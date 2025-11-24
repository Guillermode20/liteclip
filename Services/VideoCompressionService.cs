using System.Collections.Concurrent;
using System.Diagnostics;
using System.Globalization;
using System.Linq;
using System.Text;
using liteclip.Models;
using liteclip.CompressionStrategies;
using System.Text.RegularExpressions;

namespace liteclip.Services;

public class VideoCompressionService : IVideoCompressionService
{
    private readonly ConcurrentDictionary<string, JobMetadata> _jobs = new();
    private readonly ConcurrentQueue<string> _jobQueue = new();
    private readonly SemaphoreSlim _concurrencyLimiter;
    private readonly Task? _startupCleanupTask;
    private readonly string _tempUploadPath;
    private readonly string _tempOutputPath;
    private readonly ILogger<VideoCompressionService> _logger;
    private readonly FfmpegPathResolver _ffmpegResolver;
    private readonly ICompressionStrategyFactory _strategyFactory;
    private readonly int _maxConcurrentJobs;
    private readonly int _maxQueueSize;

    public VideoCompressionService(IConfiguration configuration, ILogger<VideoCompressionService> logger, FfmpegPathResolver ffmpegResolver, ICompressionStrategyFactory strategyFactory)
    {
        _logger = logger;
        _ffmpegResolver = ffmpegResolver;
        _strategyFactory = strategyFactory;
        
        // Use AppData for temp directories to avoid permission issues in Program Files
        var appDataDirectory = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
        var baseDir = Path.Combine(appDataDirectory, "LiteClip");
        
        _tempUploadPath = configuration["TempPaths:Uploads"] ?? Path.Combine(baseDir, "temp", "uploads");

        if (!Path.IsPathRooted(_tempUploadPath))
        {
          _tempUploadPath = Path.Combine(baseDir, _tempUploadPath.TrimStart(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar));
        }

        _tempOutputPath = configuration["TempPaths:Outputs"] ?? Path.Combine(baseDir, "temp", "outputs");

        if (!Path.IsPathRooted(_tempOutputPath))
        {
          _tempOutputPath = Path.Combine(baseDir, _tempOutputPath.TrimStart(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar));
        }

        if (!int.TryParse(configuration["Compression:MaxConcurrentJobs"], out var maxConcurrent))
        {
            maxConcurrent = 2;
        }
        if (!int.TryParse(configuration["Compression:MaxQueueSize"], out var maxQueue))
        {
            maxQueue = 10;
        }
        _maxConcurrentJobs = maxConcurrent;
        _maxQueueSize = maxQueue;

        _concurrencyLimiter = new SemaphoreSlim(_maxConcurrentJobs, _maxConcurrentJobs);

        Directory.CreateDirectory(_tempUploadPath);
        Directory.CreateDirectory(_tempOutputPath);

        // Schedule temp cleanup to run in background to avoid blocking the DI constructor during host startup.
        // Store the Task so tests or other code can wait for completion if necessary.
        _startupCleanupTask = Task.Run(() => CleanStartupTempFiles());
    }

    private void CleanStartupTempFiles()
    {
        try
        {
            _logger.LogInformation("Cleaning up temporary files from previous sessions...");
            if (Directory.Exists(_tempUploadPath))
            {
                foreach (var file in Directory.GetFiles(_tempUploadPath))
                {
                    try { File.Delete(file); } catch { }
                }
            }
            if (Directory.Exists(_tempOutputPath))
            {
                foreach (var file in Directory.GetFiles(_tempOutputPath))
                {
                    try { File.Delete(file); } catch { }
                }
            }
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "Failed to clean temp files on startup");
        }
    }

    public async Task<string> CompressVideoAsync(IFormFile videoFile, CompressionRequest request)
    {
        var jobId = Guid.NewGuid().ToString();
        
        // Log job header and request details
        _logger.LogJobHeader("RECEIVED", jobId, videoFile.FileName);

        var normalizedRequest = NormalizeRequest(request);
        _logger.LogCompressionRequest(jobId, 
            normalizedRequest.Mode.ToString(),
            normalizedRequest.TargetSizeMb, 
            normalizedRequest.SourceDuration,
            normalizedRequest.Segments?.Count);
        _logger.LogInformation("🎛️  Normalized codec: {Mode} → {Codec}", normalizedRequest.Mode, normalizedRequest.Codec);
        
        var codecConfig = GetCodecConfig(normalizedRequest.Codec);
        var artifacts = await PrepareJobArtifactsAsync(jobId, videoFile, normalizedRequest, codecConfig);

        normalizedRequest.SourceDuration = artifacts.EffectiveDuration ?? normalizedRequest.SourceDuration;

        var skipCompression = ShouldSkipCompression(normalizedRequest, artifacts.EffectiveMaxSizeMb, artifacts.JobId);

        // Calculate bitrates for compression (only if we're actually compressing)
        double? computedTargetKbps = null;
        double? computedVideoKbps = null;
        BitratePlan? bitratePlan = null;
        // Cache ffprobe dimensions once per job to avoid spinning up duplicate processes later in the pipeline.
        VideoDimensions? probedDims = null;

        if (!skipCompression)
        {
            // Even if we end up skipping adaptive scaling, downstream encoding still needs the source dimensions.
            probedDims = await ProbeVideoDimensionsAsync(artifacts.PreparedInputPath);
        }

        if (!skipCompression &&
            normalizedRequest.TargetSizeMb.HasValue &&
            normalizedRequest.SourceDuration.HasValue &&
            normalizedRequest.SourceDuration.Value > 0)
        {
            bitratePlan = CalculateBitratePlan(normalizedRequest, codecConfig);

            if (bitratePlan != null)
            {
                computedTargetKbps = bitratePlan.TotalKbps;
                computedVideoKbps = bitratePlan.VideoKbps;

                _logger.LogBitratePlan(artifacts.JobId, 
                    normalizedRequest.TargetSizeMb.Value,
                    normalizedRequest.SourceDuration.Value,
                    bitratePlan.VideoKbps,
                    codecConfig.AudioBitrateKbps,
                    bitratePlan.TotalKbps);

                // Auto-resolution adjustment: Downscale if bitrate is too low for the resolution
                // But only if the user hasn't explicitly requested "original" resolution (100% scale)
                if (normalizedRequest.ScalePercent != 100 && probedDims != null)
                {
                    var optimalScale = CalculateOptimalScale(probedDims.Width, probedDims.Height, bitratePlan.VideoKbps, normalizedRequest.TargetFps ?? 30, normalizedRequest.Codec);
                    var currentScale = normalizedRequest.ScalePercent ?? 100;
                    
                    if (optimalScale < currentScale)
                    {
                        _logger.LogInformation("Auto-scaling triggered for job {JobId}: Reducing resolution to {Scale}% to maintain quality (Bitrate: {Bitrate}kbps, Input: {W}x{H})", 
                            artifacts.JobId, optimalScale, bitratePlan.VideoKbps, probedDims.Width, probedDims.Height);
                        normalizedRequest.ScalePercent = optimalScale;
                    }
                }
            }
        }

        if (skipCompression)
        {
            return CompleteSkippedJob(artifacts, codecConfig, normalizedRequest);
        }

        EnsureQueueCapacity();

        // ALWAYS enable two-pass encoding for best quality and bitrate accuracy.
        var enableTwoPass = true;

        var requestSnapshot = CloneRequest(normalizedRequest);
        var compressionJob = BuildQueuedJob(artifacts, requestSnapshot, codecConfig, computedTargetKbps, computedVideoKbps, enableTwoPass);

        _jobs[jobId] = compressionJob;
        _jobQueue.Enqueue(jobId);

        _ = Task.Run(async () => await ProcessQueueAsync(jobId, normalizedRequest, codecConfig, probedDims));

        return jobId;
    }

    private async Task<JobPreparationArtifacts> PrepareJobArtifactsAsync(string jobId, IFormFile videoFile, CompressionRequest request, CodecConfig codecConfig)
    {
        var originalFilename = videoFile.FileName;
        var sanitizedFilename = string.IsNullOrWhiteSpace(originalFilename)
            ? $"{jobId}.bin"
            : Path.GetFileName(originalFilename);

        var safeStem = Path.GetFileNameWithoutExtension(string.IsNullOrWhiteSpace(sanitizedFilename) ? jobId : sanitizedFilename);
        if (string.IsNullOrWhiteSpace(safeStem))
        {
            safeStem = jobId;
        }

        var uploadPath = Path.Combine(_tempUploadPath, $"{jobId}_{sanitizedFilename}");
        await using (var stream = new FileStream(uploadPath, FileMode.Create))
        {
            await videoFile.CopyToAsync(stream);
        }

        var targetSizePrefix = request.TargetSizeMb.HasValue
            ? $"{Math.Round(request.TargetSizeMb.Value, MidpointRounding.AwayFromZero)}MB"
            : "auto";
        var outputFilename = $"{targetSizePrefix}_compressed_{safeStem}{codecConfig.FileExtension}";
        var outputPath = Path.Combine(_tempOutputPath, outputFilename);

        var originalSizeMb = videoFile.Length / (1024.0 * 1024.0);
        var originalDuration = request.SourceDuration;

        var segmentResult = await ProcessSegmentsAsync(jobId, uploadPath, request.Segments, originalDuration);
        var effectiveMaxSizeMb = originalSizeMb;

        if (segmentResult.SegmentsApplied &&
            originalDuration.HasValue && originalDuration.Value > 0 &&
            segmentResult.EffectiveDuration.HasValue)
        {
            var durationRatio = segmentResult.EffectiveDuration.Value / originalDuration.Value;
            effectiveMaxSizeMb = originalSizeMb * durationRatio;
            _logger.LogInformation("Effective max size adjusted for edited duration: {EffectiveMaxMb}MB (original: {OriginalMb}MB, ratio: {Ratio:F2})",
                effectiveMaxSizeMb, originalSizeMb, durationRatio);
        }

        return new JobPreparationArtifacts(
            jobId,
            originalFilename,
            segmentResult.PreparedInputPath,
            outputFilename,
            outputPath,
            segmentResult.EffectiveDuration ?? originalDuration,
            effectiveMaxSizeMb,
            segmentResult.SegmentsApplied);
    }

    private async Task<SegmentProcessingResult> ProcessSegmentsAsync(string jobId, string inputPath, List<VideoSegment>? segments, double? originalDuration)
    {
        if (segments == null || segments.Count == 0)
        {
            _logger.LogInformation(" No segments provided - using full video for job {JobId}", jobId);
            return new SegmentProcessingResult(false, inputPath, originalDuration);
        }

        var isFullVideo = segments.Count == 1 &&
                          segments[0].Start == 0 &&
                          originalDuration.HasValue &&
                          Math.Abs(segments[0].End - originalDuration.Value) < 0.1;

        if (isFullVideo)
        {
            _logger.LogInformation(" Segments represent full video - using full video for job {JobId}", jobId);
            return new SegmentProcessingResult(false, inputPath, originalDuration);
        }

        _logger.LogSection("SEGMENT PROCESSING");
        
        for (int i = 0; i < segments.Count; i++)
        {
            var seg = segments[i];
            _logger.LogInformation("   Segment {Index}: {Start}s - {End}s (duration: {Duration}s)",
                i + 1, seg.Start, seg.End, seg.End - seg.Start);
        }

        var mergedPath = await MergeVideoSegmentsAsync(jobId, inputPath, segments);
        var totalEditedDuration = segments.Sum(s => s.End - s.Start);
        
        _logger.LogSegmentProcessing(jobId, segments.Count, totalEditedDuration, originalDuration ?? 0);
        
        return new SegmentProcessingResult(true, mergedPath, totalEditedDuration);
    }

    private bool ShouldSkipCompression(CompressionRequest request, double effectiveMaxSizeMb, string jobId)
    {
        if (request.MuteAudio)
        {
            return false;
        }

        if (request.SkipCompression)
        {
            _logger.LogInformation(" Skip compression flag set for job {JobId} - user requested no compression", jobId);
            return true;
        }

        if (request.TargetSizeMb.HasValue && request.TargetSizeMb.Value >= (effectiveMaxSizeMb - 0.01))
        {
            _logger.LogInformation(" Target size ({TargetMb}MB) >= effective max size ({EffectiveMaxMb}MB) - skipping compression for job {JobId}",
                request.TargetSizeMb.Value, effectiveMaxSizeMb, jobId);
            return true;
        }

        return false;
    }

    private string CompleteSkippedJob(JobPreparationArtifacts artifacts, CodecConfig codecConfig, CompressionRequest request)
    {
        _logger.LogInformation(" Skipping compression for job {JobId} - copying file directly", artifacts.JobId);

        File.Copy(artifacts.PreparedInputPath, artifacts.OutputPath, overwrite: true);
        _logger.LogFileOperation("Copied", artifacts.OutputPath, new FileInfo(artifacts.OutputPath).Length);

        var job = new JobMetadata
        {
            JobId = artifacts.JobId,
            OriginalFilename = artifacts.OriginalFilename,
            InputPath = artifacts.PreparedInputPath,
            OutputPath = artifacts.OutputPath,
            OutputFilename = artifacts.OutputFilename,
            OutputMimeType = codecConfig.MimeType,
            Status = "completed",
            Codec = codecConfig.Key,
            ScalePercent = 100,
            TargetSizeMb = request.TargetSizeMb,
            TargetBitrateKbps = null,
            VideoBitrateKbps = null,
            SourceDuration = request.SourceDuration,
            TwoPass = false,
            CompressionSkipped = true,
            MuteAudio = request.MuteAudio,
            CreatedAt = DateTime.UtcNow,
            StartedAt = DateTime.UtcNow,
            CompletedAt = DateTime.UtcNow,
            Progress = 100,
            RequestSnapshot = CloneRequest(request)
        };

        if (File.Exists(artifacts.OutputPath))
        {
            job.OutputSizeBytes = new FileInfo(artifacts.OutputPath).Length;
        }

        _jobs[artifacts.JobId] = job;
        _logger.LogJobCompletion(artifacts.JobId, true, "COMPLETED (NO COMPRESSION)", job.OutputSizeBytes / (1024.0 * 1024.0), true);

        return artifacts.JobId;
    }

    private static JobMetadata BuildQueuedJob(JobPreparationArtifacts artifacts, CompressionRequest requestSnapshot, CodecConfig codecConfig, double? computedTargetKbps, double? computedVideoKbps, bool enableTwoPass)
    {
        return new JobMetadata
        {
            JobId = artifacts.JobId,
            OriginalFilename = artifacts.OriginalFilename,
            InputPath = artifacts.PreparedInputPath,
            OutputPath = artifacts.OutputPath,
            OutputFilename = artifacts.OutputFilename,
            OutputMimeType = codecConfig.MimeType,
            Status = "queued",
            Codec = codecConfig.Key,
            ScalePercent = requestSnapshot.ScalePercent,
            TargetSizeMb = requestSnapshot.TargetSizeMb,
            TargetBitrateKbps = computedTargetKbps,
            VideoBitrateKbps = computedVideoKbps,
            SourceDuration = requestSnapshot.SourceDuration,
            TwoPass = enableTwoPass,
            CreatedAt = DateTime.UtcNow,
            MuteAudio = requestSnapshot.MuteAudio,
            RequestSnapshot = CloneRequest(requestSnapshot)
        };
    }

    private void EnsureQueueCapacity()
    {
        if (_jobQueue.Count >= _maxQueueSize)
        {
            throw new InvalidOperationException($"Queue is full. Maximum queue size is {_maxQueueSize}. Please try again later.");
        }
    }

    private async Task ProcessQueueAsync(string jobId, CompressionRequest request, CodecConfig codecConfig, VideoDimensions? preProbedDimensions = null)
    {
        try
        {
            // Wait for our turn
            await _concurrencyLimiter.WaitAsync();

            // Check if job was cancelled while waiting
            if (!_jobs.TryGetValue(jobId, out var job) || job.Status == "cancelled")
            {
                _logger.LogInformation("Job {JobId} was cancelled before processing started", jobId);
                return;
            }

            // Update status from queued to processing
            job.Status = "processing";
            job.StartedAt = DateTime.UtcNow;
            _logger.LogInformation("Starting compression for job {JobId} (waited {WaitTime:F1}s in queue)", 
                jobId, (job.StartedAt.Value - job.CreatedAt).TotalSeconds);

            await RunFfmpegCompressionAsync(jobId, job, request, codecConfig, preProbedDimensions);
        }
        finally
        {
            _concurrencyLimiter.Release();
        }
    }

    public bool CancelJob(string jobId)
    {
        if (!_jobs.TryGetValue(jobId, out var job))
        {
            return false;
        }

        if (job.Status == "completed" || job.Status == "failed" || job.Status == "cancelled")
        {
            return false;
        }

        job.Status = "cancelled";
        
        // Properly terminate the FFmpeg process
        if (job.Process != null)
        {
            try
            {
                // Try graceful termination first
                if (!job.Process.HasExited)
                {
                    job.Process.Kill(entireProcessTree: true);
                    // Wait for process to actually terminate with timeout
                    if (!job.Process.WaitForExit(5000))
                    {
                        _logger.LogWarning("Process for job {JobId} did not terminate after 5s", jobId);
                    }
                }
            }
            catch (Exception ex)
            {
                _logger.LogWarning(ex, "Error while terminating process for job {JobId}", jobId);
            }
            finally
            {
                try
                {
                    job.Process.Dispose();
                }
                catch
                {
                    // Ignore disposal errors
                }
            }
        }
        
        _logger.LogInformation("Job {JobId} cancelled", jobId);
        return true;
    }

    public (bool Success, string? Error) RetryJob(string jobId)
    {
        if (!_jobs.TryGetValue(jobId, out var job))
        {
            return (false, "Job not found");
        }

        if (job.Status != "failed" && job.Status != "cancelled")
        {
            return (false, "Only failed or cancelled jobs can be retried");
        }

        if (job.CompressionSkipped)
        {
            return (false, "Jobs that skipped compression cannot be retried");
        }

        if (string.IsNullOrWhiteSpace(job.InputPath) || !File.Exists(job.InputPath))
        {
            return (false, "Original upload has been cleaned up");
        }

        var sourceRequest = job.RequestSnapshot ?? new CompressionRequest
        {
            Codec = job.Codec,
            ScalePercent = job.ScalePercent,
            TargetFps = job.RequestSnapshot?.TargetFps ?? 30,
            TargetSizeMb = job.TargetSizeMb,
            SourceDuration = job.SourceDuration,
            SkipCompression = false,
            MuteAudio = job.MuteAudio,
            UseQualityMode = string.Equals(job.Codec, "h265", StringComparison.OrdinalIgnoreCase),
            Mode = job.RequestSnapshot?.Mode ?? EncodingMode.Fast
        };

        var replayRequest = CloneRequest(sourceRequest);
        job.RequestSnapshot = CloneRequest(replayRequest);

        job.Status = "queued";
        job.Progress = 0;
        job.ErrorMessage = null;
        job.CompletedAt = null;
        job.StartedAt = null;
        job.EstimatedSecondsRemaining = null;
        job.OutputSizeBytes = null;
        job.CreatedAt = DateTime.UtcNow;
        job.CompressionSkipped = false;

        try
        {
            if (File.Exists(job.OutputPath))
            {
                File.Delete(job.OutputPath);
            }
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "Failed to delete previous output for job {JobId}", jobId);
        }

        _jobQueue.Enqueue(jobId);
        var codecConfig = GetCodecConfig(job.Codec);
        _ = Task.Run(async () => await ProcessQueueAsync(jobId, replayRequest, codecConfig, null));

        return (true, null);
    }

    public int GetQueuePosition(string jobId)
    {
        if (!_jobs.TryGetValue(jobId, out var job) || job.Status != "queued")
        {
            return 0;
        }

        var position = 1;
        foreach (var queuedJobId in _jobQueue)
        {
            if (queuedJobId == jobId)
            {
                return position;
            }
            if (_jobs.TryGetValue(queuedJobId, out var queuedJob) && queuedJob.Status == "queued")
            {
                position++;
            }
        }

        return 0;
    }

        /// <summary>
        /// Builds adaptive video filters that are always applied to improve perceived quality.
        /// These filters are especially beneficial at lower bitrates.
        /// Filters are automatically tuned based on the compression level and target size.
        /// </summary>
        private static List<string> BuildAdaptiveFilters(int scalePercent, int targetFps, double? targetSizeMb, double? sourceDuration)
    {
        var filters = new List<string>();
        
        // Calculate compression intensity (higher = more aggressive compression)
        var isHeavyCompression = false;
        if (targetSizeMb.HasValue && sourceDuration.HasValue && sourceDuration.Value > 0)
        {
            var bitrateKbps = (targetSizeMb.Value * 8 * 1024) / sourceDuration.Value;
            isHeavyCompression = bitrateKbps < 1000; // < 1 Mbps is heavy compression
        }
        
        // 1. Temporal denoising - ALWAYS apply before scaling
        // Removes noise/grain that's difficult to compress efficiently
        // More aggressive denoising for heavy compression to maximize efficiency
        if (isHeavyCompression)
        {
            // Stronger denoising for heavy compression
            filters.Add("hqdn3d=2.5:2.0:4.0:4.0");
        }
        else
        {
            // Moderate denoising for lighter compression
            filters.Add("hqdn3d=1.5:1.0:3.0:3.0");
        }
        
        // 2. Scaling (if needed)
        if (scalePercent < 100)
        {
            var factor = scalePercent / 100.0;
            var factorStr = factor.ToString(CultureInfo.InvariantCulture);
            filters.Add($"scale=trunc(iw*{factorStr}/2)*2:trunc(ih*{factorStr}/2)*2:flags=lanczos");
        }
        
        // 3. Debanding - ALWAYS apply after scaling
        // Prevents banding in gradients (skies, sunsets, dark scenes)
        // This is critical for compressed video quality
        // Parameters: range=16 pixels, threshold1/2/3/4 for each plane
        filters.Add("deband=1thr=0.02:2thr=0.02:3thr=0.02:range=16:blur=0");
        
        // 4. Contrast-adaptive sharpening - ALWAYS apply
        // Compensates for softness from denoising and enhances perceived sharpness
        if (scalePercent < 100)
        {
            // Adaptive sharpening based on downscaling amount
            var downscaleFactor = 1.0 - (scalePercent / 100.0);
            var unsharpStrength = Math.Round(0.5 + (downscaleFactor * 1.5), 2);
            var unsharpStrengthStr = unsharpStrength.ToString(CultureInfo.InvariantCulture);
            filters.Add($"unsharp=3:3:{unsharpStrengthStr}");
        }
        else
        {
            // Light sharpening even without downscaling
            filters.Add("unsharp=3:3:0.3");
        }
        
        // 5. FPS limiting (if specified)
        if (targetFps > 0)
        {
            filters.Add($"fps={targetFps}");
        }
        
        return filters;
    }

    private async Task RunFfmpegCompressionAsync(string jobId, JobMetadata job, CompressionRequest request, CodecConfig codec, VideoDimensions? preProbedDimensions)
    {
        try
        {
            // Always use bitrate-based encoding (required by frontend)
            if (!job.TargetSizeMb.HasValue || 
                !job.SourceDuration.HasValue || 
                job.SourceDuration.Value <= 0 ||
                !job.VideoBitrateKbps.HasValue)
            {
                throw new InvalidOperationException("Target size, duration, and video bitrate are required for compression");
            }

            // Apply scaling
            int scalePercent = Math.Clamp(request.ScalePercent ?? 100, 10, 100);

            // Ensure we never scale to an output height below 480px (unless source is smaller)
            try
            {
                var srcDims = preProbedDimensions;
                if (srcDims == null)
                {
                    srcDims = await ProbeVideoDimensionsAsync(job.InputPath);
                }

                if (srcDims != null && srcDims.Height >= 480)
                {
                    var minPercent = (int)Math.Ceiling(480.0 * 100 / srcDims.Height);
                    if (minPercent > 100) minPercent = 100;
                    scalePercent = Math.Max(scalePercent, minPercent);
                }
            }
            catch
            {
                // If probing fails, fall back to previous scalePercent; don't crash
            }
            job.ScalePercent = scalePercent;

            var videoBitrateKbps = job.VideoBitrateKbps.Value;

            job.TargetSizeMb = Math.Round(job.TargetSizeMb.Value, 2);
            job.VideoBitrateKbps = videoBitrateKbps;

            // Build adaptive filter chain - always applied for best quality
            var fpsToUse = request.TargetFps ?? 30;
            var filters = BuildAdaptiveFilters(scalePercent, fpsToUse, job.TargetSizeMb, job.SourceDuration);
            
            _logger.LogInformation("Applying {Count} adaptive quality filters for job {JobId} (scale={Scale}%, targetSize={TargetMb}MB)", 
                filters.Count, jobId, scalePercent, job.TargetSizeMb);

            // Get a registered compression strategy if available; fall back to legacy builders.
            ICompressionStrategy? strategy = null;
            try
            {
                strategy = _strategyFactory?.GetStrategy(codec.Key);
            }
            catch
            {
                // Ignore factory errors and fall back
                strategy = null;
            }

            if (strategy != null)
            {
                try
                {
                    var encoderName = strategy.VideoCodec;
                    job.EncoderName = encoderName;
                    // determine whether the encoder is hardware-backed - prefer probe info when available
                    var lower = encoderName?.ToLowerInvariant() ?? string.Empty;
                    var hardwareEncoders = new[] { "nvenc", "qsv", "amf", "vaapi", "v4l2m2m" };

                    var isHardwareName = hardwareEncoders.Any(h => lower.Contains(h));
                    // ensure probe says it's supported
                    job.EncoderIsHardware = isHardwareName && !string.IsNullOrWhiteSpace(encoderName);
                }
                catch
                {
                    // ignore
                }
            }
            else
            {
                try
                {
                    job.EncoderName = codec.VideoCodec;
                    job.EncoderIsHardware = false;
                }
                catch
                {
                    // ignore
                }
            }

            var containerArgs = (strategy?.BuildContainerArgs() ?? BuildContainerArgs(codec)).ToList();

            List<string> BuildArguments(double videoBitrate, AudioPlan audioPlan)
            {
                var args = new List<string> { "-y", "-i", job.InputPath };
                if (filters.Count > 0)
                {
                    args.AddRange(new[] { "-vf", string.Join(",", filters) });
                }

                if (strategy != null)
                {
                    args.AddRange(strategy.BuildVideoArgs(videoBitrate, request.Mode));
                }
                else
                {
                    args.AddRange(BuildSimpleVideoArgs(codec, videoBitrate, fpsToUse, request.Mode));
                }

                args.AddRange(BuildAudioArgsWithPlan(strategy, codec, audioPlan));
                args.AddRange(containerArgs);
                return args;
            }

            var useTwoPass = job.TwoPass;

            // Disable two-pass for hardware encoders as they typically use internal rate control
            // and running them twice with ffmpeg -pass flags often fails or is redundant.
            if (job.EncoderIsHardware == true)
            {
                _logger.LogInformation("Disabling two-pass encoding for hardware encoder {Encoder}", job.EncoderName);
                useTwoPass = false;
            }

            var enableFeedback = ShouldEnableFeedbackRetry(request.TargetSizeMb);
            var maxAttempts = enableFeedback ? 2 : 1;
            var targetSizeForFeedback = (request.TargetSizeMb ?? 0) * 0.92;
            var currentVideoKbps = videoBitrateKbps;

            if (useTwoPass)
            {
                _logger.LogInformation("Using two-pass encoding for job {JobId} with encoder {Encoder}", jobId, job.EncoderName);
                var attempts = 0;
                while (attempts < maxAttempts)
                {
                    attempts++;
                    job.VideoBitrateKbps = currentVideoKbps;
                    var audioPlan = CalculateAudioPlan(job, codec);
                    job.TargetBitrateKbps = Math.Round(currentVideoKbps + audioPlan.BitrateKbps, 2);

                    var baseArgs = BuildArguments(currentVideoKbps, audioPlan);
                    bool success;
                    try
                    {
                        success = await RunTwoPassEncodingAsync(jobId, job, baseArgs, codec, job.SourceDuration, strategy);
                    }
                    finally
                    {
                        CleanupPassLogs(jobId);
                    }
                    if (!success)
                    {
                        return;
                    }

                    var actualSizeMb = job.OutputSizeBytes.HasValue ? job.OutputSizeBytes.Value / (1024.0 * 1024.0) : 0;
                    if (attempts < maxAttempts && ShouldRetryFeedback(targetSizeForFeedback, actualSizeMb))
                    {
                        PrepareRetry(job);
                        currentVideoKbps = ComputeFeedbackBitrate(currentVideoKbps, targetSizeForFeedback, actualSizeMb);
                        continue;
                    }

                    FinalizeSuccessfulJob(job, codec);
                    break;
                }
            }
            else
            {
                var attempts = 0;
                while (attempts < maxAttempts)
                {
                    attempts++;
                    job.VideoBitrateKbps = currentVideoKbps;
                    var audioPlan = CalculateAudioPlan(job, codec);
                    job.TargetBitrateKbps = Math.Round(currentVideoKbps + audioPlan.BitrateKbps, 2);

                    var attemptArgs = BuildArguments(currentVideoKbps, audioPlan);
                    attemptArgs.Add(job.OutputPath);
                    var success = await RunSinglePassEncodingAsync(jobId, job, attemptArgs, job.SourceDuration);
                    if (!success)
                    {
                        return;
                    }

                    var actualSizeMb = job.OutputSizeBytes.HasValue ? job.OutputSizeBytes.Value / (1024.0 * 1024.0) : 0;
                    if (attempts < maxAttempts && ShouldRetryFeedback(targetSizeForFeedback, actualSizeMb))
                    {
                        PrepareRetry(job);
                        currentVideoKbps = ComputeFeedbackBitrate(currentVideoKbps, targetSizeForFeedback, actualSizeMb);
                        continue;
                    }

                    FinalizeSuccessfulJob(job, codec);
                    break;
                }
            }
        }
        catch (Exception ex)
        {
            job.Status = "failed";
            job.ErrorMessage = ex.Message;
            _logger.LogError(ex, "Exception during video compression for job {JobId}", jobId);
        }
        finally
        {
            job.CompletedAt = DateTime.UtcNow;
            
            // Ensure process is properly disposed
            if (job.Process != null)
            {
                try
                {
                    if (!job.Process.HasExited)
                    {
                        job.Process.Kill(entireProcessTree: true);
                        job.Process.WaitForExit(3000);
                    }
                }
                catch (Exception ex)
                {
                    _logger.LogWarning(ex, "Error disposing process for job {JobId}", jobId);
                }
                finally
                {
                    try
                    {
                        job.Process.Dispose();
                    }
                    catch
                    {
                        // Ignore disposal errors
                    }
                }
            }
            
            job.Process = null;
        }
    }

    private async Task<bool> RunSinglePassEncodingAsync(string jobId, JobMetadata job, List<string> arguments, double? totalDuration)
    {
        var processStartInfo = BuildFfmpegProcessStartInfo(arguments);
        var commandLine = FormatFfmpegCommand(processStartInfo.FileName, arguments);
        _logger.LogInformation("Executing FFmpeg command for job {JobId}: {Command}", jobId, commandLine);

        using var process = new Process { StartInfo = processStartInfo };
        job.Process = process;

        var errorBuilder = new StringBuilder();
        var startTime = DateTime.UtcNow;
        var lastProgressUpdate = startTime;

        process.OutputDataReceived += (_, e) =>
        {
            if (!string.IsNullOrEmpty(e.Data))
            {
                // Log output if needed
            }
        };

        process.ErrorDataReceived += (_, e) =>
        {
            if (!string.IsNullOrEmpty(e.Data))
            {
                errorBuilder.AppendLine(e.Data);

                // Parse FFmpeg progress output for real-time progress
                string line = e.Data.Trim();
                if (line.StartsWith("frame=") || line.Contains("time="))
                {
                    try
                    {
                        var now = DateTime.UtcNow;
                        var progress = ParseFfmpegProgress(line, totalDuration, out var currentTimeSeconds);
                        if (progress.HasValue)
                        {
                            job.Progress = Math.Clamp(progress.Value, 0, 100);
                            
                            // Calculate ETA every 2 seconds
                            if ((now - lastProgressUpdate).TotalSeconds >= 2 && currentTimeSeconds.HasValue && totalDuration.HasValue)
                            {
                                var elapsed = (now - startTime).TotalSeconds;
                                var speed = currentTimeSeconds.Value / elapsed; // x speed
                                if (speed > 0)
                                {
                                    var remainingSeconds = (totalDuration.Value - currentTimeSeconds.Value) / speed;
                                    job.EstimatedSecondsRemaining = (int)Math.Ceiling(remainingSeconds);
                                }
                                lastProgressUpdate = now;
                            }
                        }
                    }
                    catch
                    {
                        // Ignore parsing errors, continue with compression
                    }
                }
            }
        };

        process.Start();
        process.BeginOutputReadLine();
        process.BeginErrorReadLine();

        await process.WaitForExitAsync();

        if (job.Status == "cancelled")
        {
            _logger.LogInformation("Job {JobId} was cancelled", jobId);
            return false;
        }

        if (process.ExitCode == 0)
        {
            if (File.Exists(job.OutputPath))
            {
                var outputSize = new FileInfo(job.OutputPath).Length;
                job.OutputSizeBytes = outputSize;
            }
            return true;
        }
        else
        {
            job.Status = "failed";
            job.ErrorMessage = errorBuilder.ToString();
            _logger.LogError("Video compression failed for job {JobId}. Exit code {ExitCode}. Error: {Error}", jobId, process.ExitCode, errorBuilder.ToString());
            return false;
        }
    }

    private void FinalizeSuccessfulJob(JobMetadata job, CodecConfig codec)
    {
        job.Status = "completed";
        job.Progress = 100;
        job.EstimatedSecondsRemaining = 0;
        job.CompletedAt = DateTime.UtcNow;

        // Cleanup input file immediately to save space
        try
        {
            if (!string.IsNullOrWhiteSpace(job.InputPath) && File.Exists(job.InputPath))
            {
                File.Delete(job.InputPath);
                job.InputPath = string.Empty; // Clear path so we know it's gone
                _logger.LogInformation("Cleaned up input file for job {JobId}", job.JobId);
            }
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "Failed to delete input file for job {JobId}", job.JobId);
        }

        if (job.OutputSizeBytes.HasValue)
        {
            var outputSizeMb = job.OutputSizeBytes.Value / (1024.0 * 1024.0);
            _logger.LogInformation("Video compression completed for job {JobId} using {Codec}. Output size: {OutputSizeMb:F2} MB (Target: {TargetSizeMb} MB)",
                job.JobId, job.Codec, outputSizeMb, job.TargetSizeMb?.ToString("F2") ?? "N/A");
        }
        else
        {
            _logger.LogInformation("Video compression completed for job {JobId} using {Codec}.", job.JobId, job.Codec);
        }
    }

    private void PrepareRetry(JobMetadata job)
    {
        job.Status = "processing";
        job.Progress = 0;
        job.EstimatedSecondsRemaining = null;
        job.CompletedAt = null;
        job.OutputSizeBytes = null;
        job.ErrorMessage = null;
        job.StartedAt = DateTime.UtcNow;

        try
        {
            if (File.Exists(job.OutputPath))
            {
                File.Delete(job.OutputPath);
            }
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "Failed to delete oversized output for job {JobId} before retry", job.JobId);
        }
    }

    private async Task<bool> RunTwoPassEncodingAsync(string jobId, JobMetadata job, List<string> baseArguments, CodecConfig codec, double? totalDuration, ICompressionStrategy? strategy)
    {
        var passLogFile = Path.Combine(_tempOutputPath, $"{jobId}_ffmpeg2pass");

        // First pass
        _logger.LogInformation("Starting first pass for job {JobId}", jobId);
        var pass1Args = new List<string>(baseArguments);

        // Ask strategy for pass-specific extras when available
        if (strategy != null)
        {
            pass1Args.AddRange(strategy.GetPassExtras(1, passLogFile));
        }
        else
        {
            // Legacy fallback
            if (codec.Key == "h264" || codec.Key == "h265")
            {
                pass1Args.AddRange(new[] { "-pass", "1", "-passlogfile", passLogFile, "-f", codec.Key == "h264" ? "mp4" : "mp4" });
            }
            else if (codec.Key == "vp9")
            {
                pass1Args.AddRange(new[] { "-pass", "1", "-passlogfile", passLogFile, "-f", "webm" });
            }
            else if (codec.Key == "av1")
            {
                pass1Args.AddRange(new[] { "-pass", "1", "-passlogfile", passLogFile, "-f", "webm" });
            }
        }

        // Ensure pass1 writes to the null muxer - avoid writing mp4/webm to NUL because
        // the mp4/webm muxers may require seekable outputs (fail on NUL). Use "-f null -".
        void EnsurePass1NullOutput(List<string> passtArgs)
        {
            if (passtArgs == null) return;
            // Remove explicit -f <format> values
            for (int i = passtArgs.Count - 1; i >= 0; i--)
            {
                if (string.Equals(passtArgs[i], "-f", StringComparison.OrdinalIgnoreCase) && i + 1 < passtArgs.Count)
                {
                    passtArgs.RemoveAt(i + 1);
                    passtArgs.RemoveAt(i);
                }
            }
            // Also remove any platform-specific discard outputs
            passtArgs.RemoveAll(a => string.Equals(a, "NUL", StringComparison.OrdinalIgnoreCase) || string.Equals(a, "/dev/null", StringComparison.OrdinalIgnoreCase) || string.Equals(a, "-", StringComparison.Ordinal));

            // Add -f null - to discard the output in a platform-agnostic way
            passtArgs.Add("-f");
            passtArgs.Add("null");
            passtArgs.Add("-");
        }

        EnsurePass1NullOutput(pass1Args);
        var success = await RunPassAsync(jobId, job, pass1Args, totalDuration, 1, 2);
        if (!success)
        {
            // Attempt to degrade gracefully: sanitize x265 params or remove suspect switches
            var err = job.ErrorMessage ?? string.Empty;

            // If x265-specific params might be the root cause, try safer x265 params (cap 'subme' and similar)
            var sanitizedBase = new List<string>(baseArguments);
            var sanitized = SanitizeX265Params(sanitizedBase);
                if (sanitized)
            {
                _logger.LogWarning("Pass 1 failed for job {JobId} — trying sanitized x265 parameters", jobId);
                pass1Args = new List<string>(sanitizedBase);
                if (strategy != null)
                {
                    pass1Args.AddRange(strategy.GetPassExtras(1, passLogFile));
                }
                else
                {
                    // Legacy fallback for mp4/webm
                    if (codec.Key == "h264" || codec.Key == "h265")
                    {
                        pass1Args.AddRange(new[] { "-pass", "1", "-passlogfile", passLogFile, "-f", codec.Key == "h264" ? "mp4" : "mp4" });
                    }
                    else if (codec.Key == "vp9" || codec.Key == "av1")
                    {
                        pass1Args.AddRange(new[] { "-pass", "1", "-passlogfile", passLogFile, "-f", "webm" });
                    }
                }
                EnsurePass1NullOutput(pass1Args);

                // Retry with sanitized params
                success = await RunPassAsync(jobId, job, pass1Args, totalDuration, 1, 2);
            }

            // If the sanitized attempt still didn't work, try removing complex x265 params entirely
            if (!success)
            {
                var simplified = TryRemoveX265Params(sanitizedBase = new List<string>(baseArguments));
                    if (simplified)
                {
                    _logger.LogWarning("Pass 1 failed for job {JobId} — retrying after removing x265 params", jobId);
                    pass1Args = new List<string>(sanitizedBase);
                    if (strategy != null) pass1Args.AddRange(strategy.GetPassExtras(1, passLogFile));
                    else pass1Args.AddRange(new[] { "-pass", "1", "-passlogfile", passLogFile, "-f", codec.Key == "h264" ? "mp4" : "mp4" });
                    EnsurePass1NullOutput(pass1Args);

                    success = await RunPassAsync(jobId, job, pass1Args, totalDuration, 1, 2);
                }
            }

            if (!success)
            {
                _logger.LogError("Pass 1 failed for job {JobId} after sanitization attempts. Error: {Error}", jobId, err);
                return false;
            }
        }

        // Second pass
        _logger.LogInformation("Starting second pass for job {JobId}", jobId);
        var pass2Args = new List<string>(baseArguments);

        if (strategy != null)
        {
            pass2Args.AddRange(strategy.GetPassExtras(2, passLogFile));
        }
        else
        {
            // Legacy fallback
            if (codec.Key == "h264" || codec.Key == "h265")
            {
                pass2Args.AddRange(new[] { "-pass", "2", "-passlogfile", passLogFile });
            }
            else if (codec.Key == "vp9" || codec.Key == "av1")
            {
                pass2Args.AddRange(new[] { "-pass", "2", "-passlogfile", passLogFile });
            }
        }

        pass2Args.Add(job.OutputPath);

        success = await RunPassAsync(jobId, job, pass2Args, totalDuration, 2, 2);
        if (!success)
        {
            // If we've reached pass2 failure after a sanitation attempt, attempt a second sanitize/remove only for pass2
            var attemptAgain = false;
            var sanitizedBase2 = new List<string>(baseArguments);
            if (SanitizeX265Params(sanitizedBase2))
            {
                attemptAgain = true;
                _logger.LogWarning("Pass 2 failed for job {JobId} — retrying with sanitized params", jobId);
                pass2Args = new List<string>(sanitizedBase2);
                if (strategy != null) pass2Args.AddRange(strategy.GetPassExtras(2, passLogFile));
                else pass2Args.AddRange(new[] { "-pass", "2", "-passlogfile", passLogFile });
                pass2Args.Add(job.OutputPath);
                success = await RunPassAsync(jobId, job, pass2Args, totalDuration, 2, 2);
            }

            if (!success && !attemptAgain)
            {
                var sanitizedBase3 = new List<string>(baseArguments);
                if (TryRemoveX265Params(sanitizedBase3))
                {
                    attemptAgain = true;
                    _logger.LogWarning("Pass 2 failed for job {JobId} — retrying after removing x265 params", jobId);
                    pass2Args = new List<string>(sanitizedBase3);
                    if (strategy != null) pass2Args.AddRange(strategy.GetPassExtras(2, passLogFile));
                    else pass2Args.AddRange(new[] { "-pass", "2", "-passlogfile", passLogFile });
                    pass2Args.Add(job.OutputPath);
                    success = await RunPassAsync(jobId, job, pass2Args, totalDuration, 2, 2);
                }
            }

            if (!success)
            {
                _logger.LogError("Pass 2 failed for job {JobId} after sanitization attempts.", jobId);
                return false;
            }
        }
        if (!success) return false;

        if (File.Exists(job.OutputPath))
        {
            var outputSize = new FileInfo(job.OutputPath).Length;
            job.OutputSizeBytes = outputSize;
            var outputSizeMb = outputSize / (1024.0 * 1024.0);
            _logger.LogInformation("Two-pass encoding produced {OutputSizeMb:F2} MB for job {JobId} (Target {TargetSizeMb} MB)",
                outputSizeMb, jobId, job.TargetSizeMb?.ToString("F2") ?? "N/A");
        }

        return true;
    }

    private void CleanupPassLogs(string jobId)
    {
        try
        {
            foreach (var file in Directory.GetFiles(_tempOutputPath, $"{jobId}_ffmpeg2pass*"))
            {
                File.Delete(file);
            }
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "Failed to cleanup pass log files for job {JobId}", jobId);
        }
    }

    private async Task<bool> RunPassAsync(string jobId, JobMetadata job, List<string> arguments, double? totalDuration, int passNumber, int totalPasses)
    {
        var processStartInfo = BuildFfmpegProcessStartInfo(arguments);

        using var process = new Process { StartInfo = processStartInfo };
        job.Process = process;

        var errorBuilder = new StringBuilder();
        var startTime = DateTime.UtcNow;
        var lastProgressUpdate = startTime;

        process.ErrorDataReceived += (_, e) =>
        {
            if (!string.IsNullOrEmpty(e.Data))
            {
                errorBuilder.AppendLine(e.Data);

                string line = e.Data.Trim();
                if (line.StartsWith("frame=") || line.Contains("time="))
                {
                    try
                    {
                        var now = DateTime.UtcNow;
                        var progress = ParseFfmpegProgress(line, totalDuration, out var currentTimeSeconds);
                        if (progress.HasValue)
                        {
                            // Adjust progress based on pass number
                            var adjustedProgress = ((passNumber - 1) * 100.0 / totalPasses) + (progress.Value / totalPasses);
                            job.Progress = Math.Clamp(adjustedProgress, 0, 100);
                            
                            // Calculate ETA
                            if ((now - lastProgressUpdate).TotalSeconds >= 2 && currentTimeSeconds.HasValue && totalDuration.HasValue)
                            {
                                var elapsed = (now - startTime).TotalSeconds;
                                var speed = currentTimeSeconds.Value / elapsed;
                                if (speed > 0)
                                {
                                    var remainingThisPass = (totalDuration.Value - currentTimeSeconds.Value) / speed;
                                    var remainingPasses = (totalPasses - passNumber) * (totalDuration.Value / speed);
                                    job.EstimatedSecondsRemaining = (int)Math.Ceiling(remainingThisPass + remainingPasses);
                                }
                                lastProgressUpdate = now;
                            }
                        }
                    }
                    catch
                    {
                        // Ignore parsing errors
                    }
                }
            }
        };

        process.Start();
        process.BeginOutputReadLine();
        process.BeginErrorReadLine();

        await process.WaitForExitAsync();

        if (job.Status == "cancelled")
        {
            _logger.LogInformation("Job {JobId} was cancelled during pass {Pass}", jobId, passNumber);
            return false;
        }

        if (process.ExitCode != 0)
        {
            job.Status = "failed";
            job.ErrorMessage = $"Pass {passNumber} failed: {errorBuilder}";
            _logger.LogError("Pass {Pass} failed for job {JobId}. Exit code {ExitCode}", passNumber, jobId, process.ExitCode);
            return false;
        }

        return true;
    }

    private static double? ParseFfmpegProgress(string line, double? totalDuration, out double? currentTimeSeconds)
    {
        currentTimeSeconds = null;
        
        if (string.IsNullOrEmpty(line) || !totalDuration.HasValue || totalDuration.Value <= 0)
        {
            return null;
        }

        try
        {
            // Look for time= pattern like "time=00:01:23.45"
            var timeMatch = System.Text.RegularExpressions.Regex.Match(line, @"time=(\d{2}):(\d{2}):(\d{2}(?:\.\d+)?)");
            if (timeMatch.Success)
            {
                var hours = double.Parse(timeMatch.Groups[1].Value);
                var minutes = double.Parse(timeMatch.Groups[2].Value);
                var seconds = double.Parse(timeMatch.Groups[3].Value);

                var currentTime = hours * 3600 + minutes * 60 + seconds;
                currentTimeSeconds = currentTime;
                var progress = (currentTime / totalDuration.Value) * 100;

                return Math.Clamp(progress, 0, 100);
            }
        }
        catch
        {
            // Parsing failed, return null
        }

        return null;
    }

    public JobMetadata? GetJob(string jobId)
    {
        _jobs.TryGetValue(jobId, out var job);
        return job;
    }

    public IEnumerable<JobMetadata> GetAllJobsInternal()
    {
        return _jobs.Values.ToList();
    }

    /// <summary>
    /// Public accessor to obtain all tracked jobs.
    /// Implemented to satisfy <see cref="IVideoCompressionService"/>.
    /// </summary>
    public IEnumerable<JobMetadata> GetAllJobs()
    {
        return GetAllJobsInternal();
    }

    public void CleanupJob(string jobId)
    {
        if (_jobs.TryRemove(jobId, out var job))
        {
            // Ensure process is terminated first
            TerminateJobProcess(job);
            
            try
            {
                if (File.Exists(job.InputPath))
                {
                    File.Delete(job.InputPath);
                }

                if (File.Exists(job.OutputPath))
                {
                    File.Delete(job.OutputPath);
                }
            }
            catch (Exception ex)
            {
                _logger.LogError(ex, "Error cleaning up files for job {JobId}", jobId);
            }
        }
    }

    /// <summary>
    /// Cancels all active jobs and terminates all FFmpeg processes.
    /// Called during application shutdown to ensure clean exit.
    /// </summary>
    public void CancelAllJobs()
    {
        _logger.LogInformation("Cancelling all active jobs...");
        
        var jobsSnapshot = _jobs.Values.ToList();
        foreach (var job in jobsSnapshot)
        {
            if (job.Status == "queued" || job.Status == "processing")
            {
                job.Status = "cancelled";
                TerminateJobProcess(job);
                _logger.LogInformation("Cancelled job {JobId}", job.JobId);
            }
        }
        
        _logger.LogInformation("All active jobs have been cancelled");
    }

    /// <summary>
    /// Safely terminates and disposes the FFmpeg process associated with a job.
    /// </summary>
    private void TerminateJobProcess(JobMetadata job)
    {
        if (job.Process != null)
        {
            try
            {
                if (!job.Process.HasExited)
                {
                    _logger.LogDebug("Terminating FFmpeg process for job {JobId}", job.JobId);
                    job.Process.Kill(entireProcessTree: true);
                    
                    // Wait for process to terminate with a reasonable timeout
                    if (!job.Process.WaitForExit(5000))
                    {
                        _logger.LogWarning("FFmpeg process for job {JobId} did not terminate within 5 seconds", job.JobId);
                    }
                }
            }
            catch (Exception ex)
            {
                _logger.LogWarning(ex, "Error terminating FFmpeg process for job {JobId}", job.JobId);
            }
            finally
            {
                try
                {
                    job.Process.Dispose();
                }
                catch (Exception ex)
                {
                    _logger.LogWarning(ex, "Error disposing FFmpeg process for job {JobId}", job.JobId);
                }
                finally
                {
                    job.Process = null;
                }
            }
        }
    }

        private static CompressionRequest NormalizeRequest(CompressionRequest request)
    {
        // Derive unified encoding mode from flags first
        var mode = DeriveEncodingMode(request.UseQualityMode);
        
        // Determine codec based on the derived encoding mode
        // Fast → H.264, Quality → H.265
        var codec = mode switch
        {
            EncodingMode.Quality => "h265",
            _ => "h264"  // Default to Fast (H.264)
        };

        var normalized = new CompressionRequest
        {
            Codec = codec,
            ScalePercent = request.ScalePercent,
            TargetFps = request.TargetFps,
            TargetSizeMb = request.TargetSizeMb,
            SourceDuration = request.SourceDuration,
            Segments = NormalizeSegments(request.Segments, request.SourceDuration),
            SkipCompression = request.SkipCompression,
            MuteAudio = request.MuteAudio,
            UseQualityMode = request.UseQualityMode,
            Mode = mode
        };

        if (normalized.ScalePercent.HasValue)
        {
            normalized.ScalePercent = Math.Clamp(normalized.ScalePercent.Value, 10, 100);
        }

        if (normalized.TargetSizeMb.HasValue && normalized.TargetSizeMb.Value <= 0)
        {
            normalized.TargetSizeMb = null;
        }

        if (normalized.SourceDuration.HasValue && normalized.SourceDuration.Value <= 0)
        {
            normalized.SourceDuration = null;
        }

        // Normalize target framerate: clamp to reasonable range and default to 30 if not provided
        if (normalized.TargetFps.HasValue)
        {
            normalized.TargetFps = Math.Clamp(normalized.TargetFps.Value, 1, 240);
        }
        else
        {
            normalized.TargetFps = 30;
        }

        return normalized;
    }

    private static CodecConfig GetCodecConfig(string codec)
    {
        return codec switch
        {
            "h265" or "hevc" => new CodecConfig
            {
                Key = "h265",
                VideoCodec = "libx265",
                AudioCodec = "aac",
                FileExtension = ".mp4",
                MimeType = "video/mp4",
                AudioBitrateKbps = 192
            },
            "vp9" => new CodecConfig
            {
                Key = "vp9",
                VideoCodec = "libvpx-vp9",
                AudioCodec = "libopus",
                FileExtension = ".webm",
                MimeType = "video/webm",
                AudioBitrateKbps = 128
            },
            "av1" => new CodecConfig
            {
                Key = "av1",
                VideoCodec = "libaom-av1",
                AudioCodec = "libopus",
                FileExtension = ".webm",
                MimeType = "video/webm",
                AudioBitrateKbps = 128
            },
            _ => new CodecConfig
            {
                Key = "h264",
                VideoCodec = "libx264",
                AudioCodec = "aac",
                FileExtension = ".mp4",
                MimeType = "video/mp4",
                AudioBitrateKbps = 192
            }
        };
    }



    private static List<string> BuildSimpleVideoArgs(CodecConfig codec, double videoBitrateKbps, int fps, EncodingMode mode)
    {
        var targetBitrate = Math.Max(100, Math.Round(videoBitrateKbps));
        
        // Strict adherence to target size
        var maxRate = Math.Round(targetBitrate * 1.0);
        var buffer = Math.Round(targetBitrate * 1.0);

        // Adjust preset based on mode
        var preset = mode == EncodingMode.Fast ? "fast" : "slow";

        var args = new List<string>();

        switch (codec.Key)
        {
            case "h265":
                args.AddRange(new[] { "-c:v", codec.VideoCodec, "-preset", preset, "-pix_fmt", "yuv420p", "-tag:v", "hvc1", "-g", (fps * 2).ToString(), "-sc_threshold", "0", "-bf", "4", "-refs", "5", "-x265-params", "vbv-bufsize=" + buffer + ":vbv-maxrate=" + maxRate + ":aq-mode=3:aq-strength=1.0:psy-rd=2.0:rc-lookahead=60" });
                break;
            case "vp9":
                args.AddRange(new[] { "-c:v", codec.VideoCodec, "-deadline", "good", "-cpu-used", "1", "-row-mt", "1", "-tile-columns", "1", "-g", (fps * 2).ToString(), "-sc_threshold", "0" });
                break;
            case "av1":
                args.AddRange(new[] { "-c:v", codec.VideoCodec, "-cpu-used", "0", "-row-mt", "1", "-g", (fps * 2).ToString(), "-sc_threshold", "0" });
                break;
            default:
                args.AddRange(new[] { "-c:v", codec.VideoCodec, "-preset", preset, "-pix_fmt", "yuv420p", "-g", (fps * 2).ToString(), "-sc_threshold", "0", "-bf", "4", "-refs", "5" });
                break;
        }

        args.AddRange(new[]
        {
            "-b:v", $"{targetBitrate}k",
            "-maxrate", $"{maxRate}k",
            "-bufsize", $"{buffer}k"
        });

        return args;
    }

    private static IEnumerable<string> BuildAudioArgs(CodecConfig codec)
    {
        var args = new List<string> { "-c:a", codec.AudioCodec, "-b:a", $"{codec.AudioBitrateKbps}k" };

        if (codec.AudioCodec.Equals("libopus", StringComparison.OrdinalIgnoreCase))
        {
            args.AddRange(new[] { "-ac", "2" });
        }

        return args;
    }

    private AudioPlan CalculateAudioPlan(JobMetadata job, CodecConfig codec)
    {
        if (job.MuteAudio)
        {
            return new AudioPlan(0, 0, true);
        }

        var defaultBitrate = codec.AudioBitrateKbps;
        var defaultPlan = new AudioPlan(defaultBitrate, 2);

        if (!job.TargetSizeMb.HasValue ||
            !job.SourceDuration.HasValue || job.SourceDuration.Value <= 0 ||
            !job.VideoBitrateKbps.HasValue)
        {
            return defaultPlan;
        }

        var totalKbps = (job.TargetSizeMb.Value * 8 * 1024) / job.SourceDuration.Value;
        var residualKbps = Math.Max(32, totalKbps - job.VideoBitrateKbps.Value);
        var planned = Math.Clamp(residualKbps * 0.85, 48, defaultBitrate);
        var channels = planned <= 80 ? 1 : 2;
        if (channels == 1)
        {
            planned = Math.Min(planned, 72);
        }

        return new AudioPlan((int)Math.Round(planned, MidpointRounding.AwayFromZero), channels);
    }

    private static List<string> BuildAudioArgsWithPlan(ICompressionStrategy? strategy, CodecConfig codec, AudioPlan plan)
    {
        if (plan.Muted)
        {
            return new List<string> { "-an" };
        }

        var args = (strategy?.BuildAudioArgs() ?? BuildAudioArgs(codec)).ToList();

        ApplyOrAppend(args, "-b:a", $"{plan.BitrateKbps}k");
        ApplyOrAppend(args, "-ac", plan.Channels.ToString(CultureInfo.InvariantCulture));

        return args;
    }

    private static void ApplyOrAppend(List<string> args, string flag, string value)
    {
        var index = args.IndexOf(flag);
        if (index >= 0 && index + 1 < args.Count)
        {
            args[index + 1] = value;
            return;
        }

        args.Add(flag);
        args.Add(value);
    }

    private static IEnumerable<string> BuildContainerArgs(CodecConfig codec)
    {
        if (codec.FileExtension.Equals(".mp4", StringComparison.OrdinalIgnoreCase))
        {
            return new[] { "-movflags", "+faststart" };
        }

        return Array.Empty<string>();
    }

    private static bool ShouldEnableFeedbackRetry(double? targetSizeMb)
    {
        return targetSizeMb.HasValue && targetSizeMb.Value > 0;
    }

    private static bool ShouldRetryFeedback(double targetSizeMb, double actualSizeMb)
    {
        if (targetSizeMb <= 0 || actualSizeMb <= 0)
        {
            return false;
        }

        var overshoot = actualSizeMb / targetSizeMb;
        return overshoot > 1.05;
    }

    private static double ComputeFeedbackBitrate(double currentVideoKbps, double targetSizeMb, double actualSizeMb)
    {
        if (actualSizeMb <= 0)
        {
            return Math.Max(60, currentVideoKbps * 0.9);
        }

        var correction = targetSizeMb / actualSizeMb;
        return Math.Max(60, currentVideoKbps * correction * 0.98);
    }

    private BitratePlan? CalculateBitratePlan(CompressionRequest request, CodecConfig codecConfig)
    {
        if (!request.TargetSizeMb.HasValue || request.TargetSizeMb.Value <= 0 ||
            !request.SourceDuration.HasValue || request.SourceDuration.Value <= 0)
        {
            return null;
        }

        // TikTok‑style tuning: be more aggressive on size safety, but
        // free up as much payload as possible for texture/edges.
        // We still err on the side of staying under target, but bias
        // budgets to video instead of container overhead.

        // Slightly stronger under‑target to give the rate control room
        // to breathe while we push psychovisual quality.
        var targetSizeMb = request.TargetSizeMb.Value * 0.90;
        var durationSeconds = request.SourceDuration.Value;

        var reserveBudgetMb = CalculateReserveBudget(targetSizeMb, durationSeconds, codecConfig);
        var containerShare = GetContainerShare(codecConfig);

        // Bias reserves towards safety rather than container – containers
        // are cheap, bits are precious.
        var containerReserveMb = reserveBudgetMb * (containerShare * 0.7);
        var safetyMarginMb = reserveBudgetMb - containerReserveMb;

        // Ensure we always have a positive payload budget to hand to the encoder
        var payloadBudgetMb = targetSizeMb - reserveBudgetMb;
        if (payloadBudgetMb <= 0)
        {
            payloadBudgetMb = Math.Max(targetSizeMb * 0.1, 0.05);
            var adjustedReserve = Math.Max(targetSizeMb - payloadBudgetMb, 0);
            if (reserveBudgetMb > 0)
            {
                var scale = adjustedReserve / reserveBudgetMb;
                containerReserveMb *= scale;
                safetyMarginMb *= scale;
                reserveBudgetMb = adjustedReserve;
            }
        }

        var payloadBits = payloadBudgetMb * 1024 * 1024 * 8;
        var totalKbps = payloadBits / durationSeconds / 1000d;

        // Add a slight buffer to the reserved audio bitrate so container muxing remains under target
        // Keep audio lean so more bits land on video. Modern
        // platforms are very forgiving to slightly lower audio.
        var audioBudgetKbps = request.MuteAudio ? 0 : codecConfig.AudioBitrateKbps * 0.9;
        var videoKbps = Math.Max(80, totalKbps - audioBudgetKbps);

        return new BitratePlan(
            Math.Round(totalKbps, 2),
            Math.Round(videoKbps, 2),
            Math.Round(payloadBudgetMb, 3),
            Math.Round(containerReserveMb, 3),
            Math.Round(safetyMarginMb, 3));
    }

    private static double CalculateReserveBudget(double targetSizeMb, double durationSeconds, CodecConfig codecConfig)
    {
        // Conservative reserves to ensure we stay under target with two-pass encoding
        var baseReserve = 0.20;
        var linearComponent = targetSizeMb * (codecConfig.FileExtension.Equals(".mp4", StringComparison.OrdinalIgnoreCase) ? 0.004 : 0.0032);
        var reserve = baseReserve + linearComponent;

        if (durationSeconds >= 1800)
        {
            reserve += 0.14;
        }
        else if (durationSeconds >= 900)
        {
            reserve += 0.07;
        }
        else if (durationSeconds >= 300)
        {
            reserve += 0.035;
        }

        var maxReserve = codecConfig.FileExtension.Equals(".mp4", StringComparison.OrdinalIgnoreCase) ? 1.1 : 0.85;
        var minReserve = 0.28;
        reserve = Math.Clamp(reserve, minReserve, maxReserve);

        var maxAllowed = targetSizeMb * 0.82;
        if (reserve > maxAllowed)
        {
            reserve = maxAllowed;
        }

        return Math.Max(reserve, 0);
    }

    private static double GetContainerShare(CodecConfig codecConfig)
    {
        return codecConfig.FileExtension.Equals(".mp4", StringComparison.OrdinalIgnoreCase) ? 0.68 : 0.48;
    }

    // Attempts to sanitize libx265 parameters to avoid unsupported options on some builds.
    // Returns true if we actually changed anything.
    private static bool SanitizeX265Params(List<string> args)
    {
        if (args == null) return false;

        var changed = false;

        for (int i = 0; i < args.Count; i++)
        {
            if (string.Equals(args[i], "-x265-params", StringComparison.OrdinalIgnoreCase) && i + 1 < args.Count)
            {
                var original = args[i + 1];
                // cap subme to 7 (some libx265 builds report max supported level <= 7)
                var sanitized = Regex.Replace(original, @"subme=(\d+)", match =>
                {
                    try
                    {
                        var v = int.Parse(match.Groups[1].Value);
                        var capped = Math.Min(v, 7);
                        if (capped != v)
                        {
                            changed = true;
                        }
                        return $"subme={capped}";
                    }
                    catch
                    {
                        return match.Value;
                    }
                });

                // Also cap 'rd' to a safe value in case it's too high for some encoders
                sanitized = Regex.Replace(sanitized, @"\brd=(\d+)\b", match =>
                {
                    try
                    {
                        var v = int.Parse(match.Groups[1].Value);
                        var capped = Math.Min(v, 6);
                        if (capped != v)
                        {
                            changed = true;
                        }
                        return $"rd={capped}";
                    }
                    catch
                    {
                        return match.Value;
                    }
                });

                if (sanitized != original)
                {
                    args[i + 1] = sanitized;
                }
            }
        }

        return changed;
    }

    private static bool TryRemoveX265Params(List<string> args)
    {
        if (args == null) return false;

        var changed = false;
        for (int i = args.Count - 1; i >= 0; i--)
        {
            if (string.Equals(args[i], "-x265-params", StringComparison.OrdinalIgnoreCase) && i + 1 < args.Count)
            {
                args.RemoveAt(i + 1);
                args.RemoveAt(i);
                changed = true;
                continue;
            }

            if (string.Equals(args[i], "-x264-params", StringComparison.OrdinalIgnoreCase) && i + 1 < args.Count)
            {
                args.RemoveAt(i + 1);
                args.RemoveAt(i);
                changed = true;
            }
        }

        return changed;
    }



    private async Task<string> MergeVideoSegmentsAsync(string jobId, string inputPath, List<VideoSegment> segments)
    {
        var segmentFiles = new List<string>();
        var mergedOutputPath = Path.Combine(_tempOutputPath, $"{jobId}_merged.mp4");
        
        try
        {
            // Prepare segment extraction plan and run a single ffmpeg process to extract all segments via stream copy
            var extractionArguments = new List<string> { "-y", "-i", inputPath };

            for (int i = 0; i < segments.Count; i++)
            {
                var segment = segments[i];
                var segmentPath = Path.Combine(_tempOutputPath, $"{jobId}_segment_{i}.mp4");
                segmentFiles.Add(segmentPath);

                var duration = segment.End - segment.Start;
                var startText = segment.Start.ToString("F3", CultureInfo.InvariantCulture);
                var durationText = duration.ToString("F3", CultureInfo.InvariantCulture);

                _logger.LogInformation("Scheduled segment {Index}/{Total}: {Start}s to {End}s (duration: {Duration}s)",
                    i + 1, segments.Count, segment.Start, segment.End, duration);

                extractionArguments.AddRange(new[]
                {
                    "-ss", startText,
                    "-t", durationText,
                    "-avoid_negative_ts", "make_zero",
                    "-c", "copy",
                    segmentPath
                });
            }

            var extractionStartInfo = BuildFfmpegProcessStartInfo(extractionArguments);
            var extractionCommand = FormatFfmpegCommand(extractionStartInfo.FileName, extractionArguments);
            _logger.LogInformation("Extracting {Count} segments in a single ffmpeg invocation: {Command}", segments.Count, extractionCommand);

            using (var extractionProcess = new Process { StartInfo = extractionStartInfo })
            {
                extractionProcess.Start();

                var extractionStdErr = await extractionProcess.StandardError.ReadToEndAsync();
                await extractionProcess.WaitForExitAsync();

                if (extractionProcess.ExitCode != 0)
                {
                    _logger.LogError("Failed to extract segments. Exit code: {ExitCode}. Error: {Error}", extractionProcess.ExitCode, extractionStdErr);
                    throw new InvalidOperationException($"Failed to extract segments: ffmpeg exited with code {extractionProcess.ExitCode}. Error: {extractionStdErr}");
                }
            }
            
            // Create concat demuxer file with proper Windows path handling
            var concatFilePath = Path.Combine(_tempOutputPath, $"{jobId}_concat.txt");
            var concatContent = new StringBuilder();
            foreach (var segmentFile in segmentFiles)
            {
                // Use absolute paths and convert to forward slashes for ffmpeg compatibility
                var absolutePath = Path.GetFullPath(segmentFile);
                var normalizedPath = absolutePath.Replace("\\", "/");
                concatContent.AppendLine($"file '{normalizedPath}'");
            }
            await File.WriteAllTextAsync(concatFilePath, concatContent.ToString());
            
            _logger.LogInformation("Created concat file at {Path} with {Count} segments", concatFilePath, segmentFiles.Count);
            _logger.LogInformation("Concat file contents:\n{Contents}", await File.ReadAllTextAsync(concatFilePath));
            
            // Verify all segment files exist before merging
            foreach (var segFile in segmentFiles)
            {
                if (!File.Exists(segFile))
                {
                    _logger.LogError("Segment file does not exist: {Path}", segFile);
                    throw new InvalidOperationException($"Segment file not found: {segFile}");
                }
                _logger.LogInformation("Verified segment exists: {Path} ({Size} bytes)", segFile, new FileInfo(segFile).Length);
            }
            
            // Merge segments using concat demuxer with stream copy
            var mergeArguments = new List<string>
            {
                "-y",
                "-f", "concat",
                "-safe", "0",
                "-i", concatFilePath,
                "-c", "copy",
                mergedOutputPath
            };
            
            var mergeProcessStartInfo = BuildFfmpegProcessStartInfo(mergeArguments);
            var mergeCommand = FormatFfmpegCommand(mergeProcessStartInfo.FileName, mergeArguments);

            _logger.LogInformation("Merging {Count} segments into single file using command: {Command}", segmentFiles.Count, mergeCommand);
            
            using var mergeProcess = new Process { StartInfo = mergeProcessStartInfo };
            mergeProcess.Start();
            
            var mergeStdOutput = await mergeProcess.StandardOutput.ReadToEndAsync();
            var mergeErrorOutput = await mergeProcess.StandardError.ReadToEndAsync();
            await mergeProcess.WaitForExitAsync();
            
            if (mergeProcess.ExitCode != 0)
            {
                _logger.LogError("Failed to merge segments. Exit code: {ExitCode}", mergeProcess.ExitCode);
                _logger.LogError("FFmpeg stdout: {StdOut}", mergeStdOutput);
                _logger.LogError("FFmpeg stderr: {StdErr}", mergeErrorOutput);
                throw new InvalidOperationException($"Failed to merge segments: ffmpeg exited with code {mergeProcess.ExitCode}. Error: {mergeErrorOutput}");
            }
            
            _logger.LogInformation("Segments merged successfully to {Path}", mergedOutputPath);
            
            // Clean up segment files and concat file
            foreach (var segmentFile in segmentFiles)
            {
                try
                {
                    if (File.Exists(segmentFile))
                    {
                        File.Delete(segmentFile);
                    }
                }
                catch (Exception ex)
                {
                    _logger.LogWarning(ex, "Failed to delete segment file {Path}", segmentFile);
                }
            }
            
            try
            {
                if (File.Exists(concatFilePath))
                {
                    File.Delete(concatFilePath);
                }
            }
            catch (Exception ex)
            {
                _logger.LogWarning(ex, "Failed to delete concat file {Path}", concatFilePath);
            }
            
            return mergedOutputPath;
        }
        catch (Exception ex)
        {
            // Clean up on error
            _logger.LogError(ex, "Error during segment merging");
            
            foreach (var segmentFile in segmentFiles)
            {
                try
                {
                    if (File.Exists(segmentFile))
                    {
                        File.Delete(segmentFile);
                    }
                }
                catch
                {
                    // Ignore cleanup errors
                }
            }
            
            throw;
        }
    }

    private static List<VideoSegment>? NormalizeSegments(List<VideoSegment>? segments, double? sourceDuration)
    {
        if (segments == null || segments.Count == 0)
        {
            return null;
        }

        var normalized = new List<VideoSegment>();
        foreach (var segment in segments)
        {
            if (segment == null)
            {
                continue;
            }

            var start = double.IsFinite(segment.Start) ? Math.Max(0, segment.Start) : 0;
            var end = double.IsFinite(segment.End) ? Math.Max(0, segment.End) : 0;

            if (sourceDuration.HasValue)
            {
                start = Math.Min(start, sourceDuration.Value);
                end = Math.Min(end, sourceDuration.Value);
            }

            if (end - start < 0.05)
            {
                continue;
            }

            normalized.Add(new VideoSegment
            {
                Start = start,
                End = end
            });
        }

        if (normalized.Count == 0)
        {
            return null;
        }

        normalized = normalized
            .OrderBy(s => s.Start)
            .ThenBy(s => s.End)
            .ToList();

        var merged = new List<VideoSegment>();
        foreach (var segment in normalized)
        {
            if (merged.Count == 0)
            {
                merged.Add(new VideoSegment { Start = segment.Start, End = segment.End });
                continue;
            }

            var last = merged[^1];
            if (segment.Start <= last.End + 0.01)
            {
                last.End = Math.Max(last.End, segment.End);
            }
            else
            {
                merged.Add(new VideoSegment { Start = segment.Start, End = segment.End });
            }
        }

        return merged;
    }

    private static CompressionRequest CloneRequest(CompressionRequest source)
    {
        return new CompressionRequest
        {
            Codec = source.Codec,
            ScalePercent = source.ScalePercent,
            TargetFps = source.TargetFps,
            TargetSizeMb = source.TargetSizeMb,
            SkipCompression = source.SkipCompression,
            MuteAudio = source.MuteAudio,
            SourceDuration = source.SourceDuration,
            Segments = source.Segments?.Select(segment => new VideoSegment
            {
                Start = segment.Start,
                End = segment.End
            }).ToList(),
            UseQualityMode = source.UseQualityMode,
            Mode = source.Mode
        };
    }



    private static EncodingMode DeriveEncodingMode(bool useQualityMode)
    {
        if (useQualityMode)
        {
            return EncodingMode.Quality;
        }

        return EncodingMode.Fast;
    }

    private sealed record BitratePlan(double TotalKbps, double VideoKbps, double PayloadBudgetMb, double ContainerReserveMb, double SafetyMarginMb);
    private sealed record AudioPlan(int BitrateKbps, int Channels, bool Muted = false);

    private sealed class CodecConfig
    {
        public string Key { get; init; } = "h264";
        public string VideoCodec { get; init; } = "libx264";
        public string AudioCodec { get; init; } = "aac";
        public string FileExtension { get; init; } = ".mp4";
        public string MimeType { get; init; } = "video/mp4";
        public int AudioBitrateKbps { get; init; } = 128;
    }

    private sealed record JobPreparationArtifacts(
        string JobId,
        string OriginalFilename,
        string PreparedInputPath,
        string OutputFilename,
        string OutputPath,
        double? EffectiveDuration,
        double EffectiveMaxSizeMb,
        bool SegmentsApplied);

    private sealed record SegmentProcessingResult(bool SegmentsApplied, string PreparedInputPath, double? EffectiveDuration);

    private ProcessStartInfo BuildFfmpegProcessStartInfo(IEnumerable<string> arguments)
    {
        var ffmpegPath = _ffmpegResolver.GetFfmpegPath();
        var startInfo = new ProcessStartInfo
        {
            FileName = ffmpegPath,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true
        };

        foreach (var arg in arguments)
        {
            startInfo.ArgumentList.Add(arg);
        }

        return startInfo;
    }

    private static string FormatFfmpegCommand(string executablePath, IEnumerable<string> arguments)
    {
        var quotedExe = executablePath.Contains(' ') ? $"\"{executablePath}\"" : executablePath;
        var args = arguments.Select(arg => arg.Contains(' ') ? $"\"{arg}\"" : arg);
        return string.Join(" ", new[] { quotedExe }.Concat(args));
    }

    private string GetFfprobePath()
    {
        var ffmpegPath = _ffmpegResolver.GetFfmpegPath();
        var directory = Path.GetDirectoryName(ffmpegPath);
        var extension = Path.GetExtension(ffmpegPath);
        var ffprobeName = "ffprobe" + extension;
        
        if (string.IsNullOrEmpty(directory)) return ffprobeName;
        
        var probePath = Path.Combine(directory, ffprobeName);
        if (File.Exists(probePath)) return probePath;

        // Fallback: try to find in PATH if not next to ffmpeg
        return "ffprobe";
    }

    private async Task<VideoDimensions?> ProbeVideoDimensionsAsync(string filePath)
    {
        try
        {
            var startInfo = new ProcessStartInfo
            {
                FileName = GetFfprobePath(),
                Arguments = $"-v error -select_streams v:0 -show_entries stream=width,height -of csv=s=x:p=0 \"{filePath}\"",
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                UseShellExecute = false,
                CreateNoWindow = true
            };

            using var process = new Process { StartInfo = startInfo };
            process.Start();
            
            var output = await process.StandardOutput.ReadToEndAsync();
            await process.WaitForExitAsync();

            if (process.ExitCode == 0 && !string.IsNullOrWhiteSpace(output))
            {
                var parts = output.Trim().Split('x');
                if (parts.Length == 2 && int.TryParse(parts[0], out var w) && int.TryParse(parts[1], out var h))
                {
                    return new VideoDimensions(w, h);
                }
            }
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "Failed to probe video dimensions for {Path}", filePath);
        }
        return null;
    }

    private int CalculateOptimalScale(int width, int height, double videoKbps, int fps, string codec)
    {
        // Target bits per pixel (BPP) for "good" quality
        // These are heuristic values:
        // H.264 needs ~0.1 bpp for complex scenes, 0.05 for simple
        // H.265/VP9 are ~30-40% more efficient
        // AV1 is ~50% more efficient
        
        double targetBpp = codec.ToLowerInvariant() switch
        {
            "h265" or "hevc" => 0.065,
            "vp9" => 0.07,
            "av1" => 0.055,
            _ => 0.095 // H.264 and others
        };

        var pixels = width * height;
        if (pixels <= 0) return 100;

        // Calculate max pixels we can support at this bitrate with acceptable quality
        // videoKbps * 1000 = bits per second
        // bits per frame = bits per second / fps
        // max pixels = bits per frame / targetBpp
        var maxPixels = (videoKbps * 1000) / (fps * targetBpp);

        if (maxPixels >= pixels) return 100;

        // Calculate scale factor
        // new_pixels = scale^2 * old_pixels
        // scale = sqrt(new_pixels / old_pixels)
        var scale = Math.Sqrt(maxPixels / pixels);
        
        // Convert to percentage and round down to nearest 5%
        var percent = (int)(scale * 100);

        // If the source height is at least 480p, do not allow scaling
        // that reduces the output height below 480 pixels. This prevents
        // extremely small outputs that the UI or users don't expect.
        var minOutputHeight = 480;
        if (height >= minOutputHeight)
        {
            var minPercent = (int)Math.Ceiling(minOutputHeight * 100.0 / height);
            if (minPercent > 100) minPercent = 100;
            // Round up to nearest 5% to keep UI increments consistent and
            // ensure we don't round down below the minimum height.
            var minPercentRounded = ((minPercent + 4) / 5) * 5;
            percent = Math.Max(percent, minPercentRounded);
        }

        // Don't go below 25% or above 100%
        var finalPercent = Math.Clamp(((percent + 4) / 5) * 5, 25, 100);
        return finalPercent;
    }

    private sealed record VideoDimensions(int Width, int Height);
}

public class JobMetadata
{
    public string JobId { get; set; } = string.Empty;
    public string OriginalFilename { get; set; } = string.Empty;
    public string InputPath { get; set; } = string.Empty;
    public string OutputPath { get; set; } = string.Empty;
    public string OutputFilename { get; set; } = string.Empty;
    public string OutputMimeType { get; set; } = "video/mp4";
    public long? OutputSizeBytes { get; set; }
    public bool CompressionSkipped { get; set; } = false;
    public string Status { get; set; } = string.Empty;
    public string Codec { get; set; } = "h264";
    public int? ScalePercent { get; set; }
    public double? TargetSizeMb { get; set; }
    public double? TargetBitrateKbps { get; set; }
    public double? VideoBitrateKbps { get; set; }
    public double? SourceDuration { get; set; }
    public double Progress { get; set; } = 0;
    public string? ErrorMessage { get; set; }
    public bool TwoPass { get; set; } = false;
    public bool MuteAudio { get; set; }
    public DateTime CreatedAt { get; set; }
    public DateTime? StartedAt { get; set; }
    public DateTime? CompletedAt { get; set; }
    public int? EstimatedSecondsRemaining { get; set; }
    public Process? Process { get; set; }
        // Encoder metadata
        public string? EncoderName { get; set; }
        public bool? EncoderIsHardware { get; set; }
        public CompressionRequest? RequestSnapshot { get; set; }
}


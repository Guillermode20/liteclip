using System.Collections.Concurrent;
using System.Diagnostics;
using System.Globalization;
using System.Linq;
using System.Text;
using liteclip.Models;
using liteclip.CompressionStrategies;

namespace liteclip.Services;

public class VideoCompressionService : IVideoCompressionService
{
    private readonly ConcurrentDictionary<string, JobMetadata> _jobs = new();
    private readonly ConcurrentQueue<string> _jobQueue = new();
    private readonly SemaphoreSlim _concurrencyLimiter;
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
        _tempUploadPath = configuration["TempPaths:Uploads"] ?? Path.Combine(Path.GetTempPath(), "video-uploads");

        if (!Path.IsPathRooted(_tempUploadPath))
        {
          _tempUploadPath = Path.Combine(Path.GetTempPath(), _tempUploadPath.TrimStart(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar));
        }

        _tempOutputPath = configuration["TempPaths:Outputs"] ?? Path.Combine(Path.GetTempPath(), "video-outputs");

        if (!Path.IsPathRooted(_tempOutputPath))
        {
          _tempOutputPath = Path.Combine(Path.GetTempPath(), _tempOutputPath.TrimStart(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar));
        }

        _maxConcurrentJobs = configuration.GetValue<int>("Compression:MaxConcurrentJobs", 2);
        _maxQueueSize = configuration.GetValue<int>("Compression:MaxQueueSize", 10);

        _concurrencyLimiter = new SemaphoreSlim(_maxConcurrentJobs, _maxConcurrentJobs);

        Directory.CreateDirectory(_tempUploadPath);
        Directory.CreateDirectory(_tempOutputPath);
    }

    public async Task<string> CompressVideoAsync(IFormFile videoFile, CompressionRequest request)
    {
        _logger.LogInformation("Compression request received - Mode: {Mode}, TargetSizeMb: {TargetSizeMb}, SourceDuration: {SourceDuration}",
            $"{(request.UseUltraMode ? "Ultra" : request.UseQualityMode ? "Quality" : "Fast")}", request.TargetSizeMb, request.SourceDuration);

        var normalizedRequest = NormalizeRequest(request);
        _logger.LogInformation("Normalized codec from mode: {Mode} → {Codec}",
            normalizedRequest.Mode, normalizedRequest.Codec);
        var codecConfig = GetCodecConfig(normalizedRequest.Codec);

        var jobId = Guid.NewGuid().ToString();
        var artifacts = await PrepareJobArtifactsAsync(jobId, videoFile, normalizedRequest, codecConfig);

        normalizedRequest.SourceDuration = artifacts.EffectiveDuration ?? normalizedRequest.SourceDuration;

        var skipCompression = ShouldSkipCompression(normalizedRequest, artifacts.EffectiveMaxSizeMb, artifacts.JobId);

        // Calculate bitrates for compression (only if we're actually compressing)
        double? computedTargetKbps = null;
        double? computedVideoKbps = null;
        BitratePlan? bitratePlan = null;

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

                _logger.LogInformation(
                    "Bitrate plan for job {JobId}: TargetSize={TargetMb}MB, Duration={Duration}s, PayloadBudget={PayloadMb}MB, ContainerReserve={ContainerMb}MB, SafetyMargin={SafetyMb}MB, TotalKbps={TotalKbps}, VideoKbps={VideoKbps}, AudioKbps={AudioKbps}",
                    artifacts.JobId,
                    normalizedRequest.TargetSizeMb.Value,
                    normalizedRequest.SourceDuration.Value,
                    bitratePlan.PayloadBudgetMb,
                    bitratePlan.ContainerReserveMb,
                    bitratePlan.SafetyMarginMb,
                    bitratePlan.TotalKbps,
                    bitratePlan.VideoKbps,
                    codecConfig.AudioBitrateKbps);
            }
        }

        if (skipCompression)
        {
            return CompleteSkippedJob(artifacts, codecConfig, normalizedRequest);
        }

        EnsureQueueCapacity();

        // ALWAYS enable two-pass encoding for best quality and bitrate accuracy.
        var enableTwoPass = true;

        var compressionJob = BuildQueuedJob(artifacts, normalizedRequest, codecConfig, computedTargetKbps, computedVideoKbps, enableTwoPass);

        _jobs[jobId] = compressionJob;
        _jobQueue.Enqueue(jobId);

        _ = Task.Run(async () => await ProcessQueueAsync(jobId, normalizedRequest, codecConfig));

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
            _logger.LogInformation("No segments provided - using full video for job {JobId}", jobId);
            return new SegmentProcessingResult(false, inputPath, originalDuration);
        }

        var isFullVideo = segments.Count == 1 &&
                          segments[0].Start == 0 &&
                          originalDuration.HasValue &&
                          Math.Abs(segments[0].End - originalDuration.Value) < 0.1;

        if (isFullVideo)
        {
            _logger.LogInformation("Segments represent full video - not processing segments for job {JobId}", jobId);
            return new SegmentProcessingResult(false, inputPath, originalDuration);
        }

        _logger.LogInformation("Processing {Count} video segments for job {JobId}", segments.Count, jobId);

        for (int i = 0; i < segments.Count; i++)
        {
            var seg = segments[i];
            _logger.LogInformation("Segment {Index}: {Start}s - {End}s (duration: {Duration}s)",
                i + 1, seg.Start, seg.End, seg.End - seg.Start);
        }

        var mergedPath = await MergeVideoSegmentsAsync(jobId, inputPath, segments);
        var totalEditedDuration = segments.Sum(s => s.End - s.Start);
        _logger.LogInformation("Updated source duration from segments: {Duration}s (original was {OriginalDuration}s)",
            totalEditedDuration, originalDuration);

        return new SegmentProcessingResult(true, mergedPath, totalEditedDuration);
    }

    private bool ShouldSkipCompression(CompressionRequest request, double effectiveMaxSizeMb, string jobId)
    {
        if (request.SkipCompression)
        {
            _logger.LogInformation("Skip compression flag is set for job {JobId} - user requested no compression", jobId);
            return true;
        }

        if (request.TargetSizeMb.HasValue && request.TargetSizeMb.Value >= (effectiveMaxSizeMb - 0.01))
        {
            _logger.LogInformation("Target size ({TargetMb}MB) is >= effective max size ({EffectiveMaxMb}MB) - skipping compression for job {JobId}",
                request.TargetSizeMb.Value, effectiveMaxSizeMb, jobId);
            return true;
        }

        return false;
    }

    private string CompleteSkippedJob(JobPreparationArtifacts artifacts, CodecConfig codecConfig, CompressionRequest request)
    {
        _logger.LogInformation("Skipping compression for job {JobId} - copying file directly", artifacts.JobId);

        File.Copy(artifacts.PreparedInputPath, artifacts.OutputPath, overwrite: true);

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
            CreatedAt = DateTime.UtcNow,
            StartedAt = DateTime.UtcNow,
            CompletedAt = DateTime.UtcNow,
            Progress = 100
        };

        if (File.Exists(artifacts.OutputPath))
        {
            job.OutputSizeBytes = new FileInfo(artifacts.OutputPath).Length;
        }

        _jobs[artifacts.JobId] = job;
        _logger.LogInformation("Job {JobId} completed immediately (no compression)", artifacts.JobId);

        return artifacts.JobId;
    }

    private static JobMetadata BuildQueuedJob(JobPreparationArtifacts artifacts, CompressionRequest request, CodecConfig codecConfig, double? computedTargetKbps, double? computedVideoKbps, bool enableTwoPass)
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
            ScalePercent = request.ScalePercent,
            TargetSizeMb = request.TargetSizeMb,
            TargetBitrateKbps = computedTargetKbps,
            VideoBitrateKbps = computedVideoKbps,
            SourceDuration = request.SourceDuration,
            TwoPass = enableTwoPass,
            CreatedAt = DateTime.UtcNow
        };
    }

    private void EnsureQueueCapacity()
    {
        if (_jobQueue.Count >= _maxQueueSize)
        {
            throw new InvalidOperationException($"Queue is full. Maximum queue size is {_maxQueueSize}. Please try again later.");
        }
    }

    private async Task ProcessQueueAsync(string jobId, CompressionRequest request, CodecConfig codecConfig)
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

            await RunFfmpegCompressionAsync(jobId, job, request, codecConfig);
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
            
            // 5. Grain synthesis - apply only for heavy compression
            // Adds subtle film grain to mask compression artifacts
            // Only needed when bitrate is very low
            if (isHeavyCompression)
            {
                filters.Add("noise=alls=6:allf=t");
            }
            
            // 6. FPS limiting (if specified)
            if (targetFps > 0)
            {
                filters.Add($"fps={targetFps}");
            }
            
            return filters;
        }

        private async Task RunFfmpegCompressionAsync(string jobId, JobMetadata job, CompressionRequest request, CodecConfig codec)
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
            job.ScalePercent = scalePercent;

            var targetBitrateKbps = job.TargetBitrateKbps ?? 0;
            var videoBitrateKbps = job.VideoBitrateKbps.Value;

            job.TargetSizeMb = Math.Round(job.TargetSizeMb.Value, 2);
            job.TargetBitrateKbps = targetBitrateKbps;
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
                    job.EncoderIsHardware = !(encoderName == "libx264" || encoderName == "libx265" || encoderName == "libvpx-vp9" || encoderName == "libaom-av1");
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

            var audioArgs = (strategy?.BuildAudioArgs() ?? BuildAudioArgs(codec)).ToList();
            var containerArgs = (strategy?.BuildContainerArgs() ?? BuildContainerArgs(codec)).ToList();

            List<string> BuildArguments(double videoBitrate)
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

                args.AddRange(audioArgs);
                args.AddRange(containerArgs);
                return args;
            }

            var useTwoPass = job.TwoPass;
            if (useTwoPass)
            {
                _logger.LogInformation("Using two-pass encoding for job {JobId} with encoder {Encoder}", jobId, job.EncoderName);
                var baseArgs = BuildArguments(videoBitrateKbps); // without output
                await RunTwoPassEncodingAsync(jobId, job, baseArgs, codec, job.SourceDuration, strategy);
            }
            else
            {
                var attempts = 0;
                var maxAttempts = ShouldEnableFeedbackRetry(strategy, request.TargetSizeMb) ? 2 : 1;
                var currentVideoKbps = videoBitrateKbps;

                while (attempts < maxAttempts)
                {
                    attempts++;
                    var attemptArgs = BuildArguments(currentVideoKbps);
                    attemptArgs.Add(job.OutputPath);
                    var success = await RunSinglePassEncodingAsync(jobId, job, attemptArgs, job.SourceDuration);
                    if (!success)
                    {
                    return;
                }

                // Use 92% of target size for feedback correction to match initial bitrate calculation
                var targetSizeMb = (request.TargetSizeMb ?? 0) * 0.92;
                var actualSizeMb = job.OutputSizeBytes.HasValue ? job.OutputSizeBytes.Value / (1024.0 * 1024.0) : 0;                    if (attempts == maxAttempts || !ShouldRetryFeedback(targetSizeMb, actualSizeMb))
                    {
                        FinalizeSinglePassJob(job, codec);
                        break;
                    }

                    currentVideoKbps = ComputeFeedbackBitrate(currentVideoKbps, targetSizeMb, actualSizeMb);
                    job.VideoBitrateKbps = currentVideoKbps;
                    job.TargetBitrateKbps = Math.Round(currentVideoKbps + codec.AudioBitrateKbps, 2);
                    job.Progress = 0;
                    job.EstimatedSecondsRemaining = null;
                    job.Status = "processing";
                    job.CompletedAt = null;
                    job.StartedAt = DateTime.UtcNow;
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
        var commandLine = $"ffmpeg {string.Join(" ", arguments.Select(a => a.Contains(" ") ? $"\"{a}\"" : a))}";
        _logger.LogInformation("Executing FFmpeg command for job {JobId}: {Command}", jobId, commandLine);

        var processStartInfo = new ProcessStartInfo
        {
            FileName = _ffmpegResolver.GetFfmpegPath(),
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true
        };

        foreach (var arg in arguments)
        {
            processStartInfo.ArgumentList.Add(arg);
        }

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

    private void FinalizeSinglePassJob(JobMetadata job, CodecConfig codec)
    {
        job.Status = "completed";
        job.Progress = 100;
        job.EstimatedSecondsRemaining = 0;
        job.CompletedAt = DateTime.UtcNow;

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

    private async Task RunTwoPassEncodingAsync(string jobId, JobMetadata job, List<string> baseArguments, CodecConfig codec, double? totalDuration, ICompressionStrategy? strategy)
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

        // Use null output for first pass
        if (OperatingSystem.IsWindows())
        {
            pass1Args.Add("NUL");
        }
        else
        {
            pass1Args.Add("/dev/null");
        }

        var success = await RunPassAsync(jobId, job, pass1Args, totalDuration, 1, 2);
        if (!success) return;

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

        await RunPassAsync(jobId, job, pass2Args, totalDuration, 2, 2);

        // Cleanup pass log files
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
        var processStartInfo = new ProcessStartInfo
        {
            FileName = _ffmpegResolver.GetFfmpegPath(),
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true
        };

        foreach (var arg in arguments)
        {
            processStartInfo.ArgumentList.Add(arg);
        }

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

        if (passNumber == totalPasses)
        {
            job.Status = "completed";
            job.Progress = 100;
            job.EstimatedSecondsRemaining = 0;
            
            if (File.Exists(job.OutputPath))
            {
                var outputSize = new FileInfo(job.OutputPath).Length;
                    job.OutputSizeBytes = outputSize;
                var outputSizeMb = outputSize / (1024.0 * 1024.0);
                _logger.LogInformation("Two-pass compression completed for job {JobId}. Output size: {OutputSizeMb:F2} MB (Target: {TargetSizeMb} MB)", 
                    jobId, outputSizeMb, job.TargetSizeMb?.ToString("F2") ?? "N/A");
            }
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

        private static CompressionRequest NormalizeRequest(CompressionRequest request)
    {
        // Derive unified encoding mode from flags first
        var mode = DeriveEncodingMode(request.UseQualityMode, request.UseUltraMode);
        
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
            UseQualityMode = request.UseQualityMode,
            UseUltraMode = request.UseUltraMode,
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
                AudioBitrateKbps = 128
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
                AudioBitrateKbps = 128
            }
        };
    }



    private static List<string> BuildSimpleVideoArgs(CodecConfig codec, double videoBitrateKbps, int fps, EncodingMode mode)
    {
        _ = mode;
        var targetBitrate = Math.Max(100, Math.Round(videoBitrateKbps));
        // Tighter bitrate control for more accurate file sizes
        // maxrate: 3% variance (reduced from 5%)
        // minrate: 97% of target (increased from 95%)
        // bufsize: 1.0x for very tight control (reduced from 1.5x)
        var maxRate = Math.Round(targetBitrate * 1.03);
        var minRate = Math.Round(targetBitrate * 0.97);
        var buffer = Math.Round(targetBitrate * 1.0);

        var args = new List<string>();

        switch (codec.Key)
        {
            case "h265":
                args.AddRange(new[] { "-c:v", codec.VideoCodec, "-preset", "slower", "-pix_fmt", "yuv420p", "-tag:v", "hvc1", "-g", (fps * 2).ToString(), "-sc_threshold", "0", "-bf", "4", "-refs", "5", "-x265-params", "vbv-bufsize=" + buffer + ":vbv-maxrate=" + maxRate + ":aq-mode=3:aq-strength=1.0:psy-rd=2.0:rc-lookahead=60" });
                break;
            case "vp9":
                args.AddRange(new[] { "-c:v", codec.VideoCodec, "-deadline", "good", "-cpu-used", "1", "-row-mt", "1", "-tile-columns", "1", "-g", (fps * 2).ToString(), "-sc_threshold", "0" });
                break;
            case "av1":
                args.AddRange(new[] { "-c:v", codec.VideoCodec, "-cpu-used", "0", "-row-mt", "1", "-g", (fps * 2).ToString(), "-sc_threshold", "0" });
                break;
            default:
                args.AddRange(new[] { "-c:v", codec.VideoCodec, "-preset", "slower", "-pix_fmt", "yuv420p", "-g", (fps * 2).ToString(), "-sc_threshold", "0", "-bf", "4", "-refs", "5" });
                break;
        }

        args.AddRange(new[]
        {
            "-b:v", $"{targetBitrate}k",
            "-maxrate", $"{maxRate}k",
            "-bufsize", $"{buffer}k",
            "-minrate", $"{minRate}k"
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

    private static IEnumerable<string> BuildContainerArgs(CodecConfig codec)
    {
        if (codec.FileExtension.Equals(".mp4", StringComparison.OrdinalIgnoreCase))
        {
            return new[] { "-movflags", "+faststart" };
        }

        return Array.Empty<string>();
    }

    private static bool ShouldEnableFeedbackRetry(ICompressionStrategy? strategy, double? targetSizeMb)
    {
        if (!targetSizeMb.HasValue || targetSizeMb.Value <= 0)
        {
            return false;
        }

        if (strategy == null)
        {
            return false;
        }

        var encoderName = strategy.VideoCodec ?? string.Empty;
        return encoderName.Contains("amf", StringComparison.OrdinalIgnoreCase);
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

        // Target 8% lower to account for container overhead and ensure we stay under target with two-pass
        var targetSizeMb = request.TargetSizeMb.Value * 0.92;
        var durationSeconds = request.SourceDuration.Value;

        var reserveBudgetMb = CalculateReserveBudget(targetSizeMb, durationSeconds, codecConfig);
        var containerShare = GetContainerShare(codecConfig);

        var containerReserveMb = reserveBudgetMb * containerShare;
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
        var audioBudgetKbps = codecConfig.AudioBitrateKbps * 1.04;
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



    private async Task<string> MergeVideoSegmentsAsync(string jobId, string inputPath, List<VideoSegment> segments)
    {
        var segmentFiles = new List<string>();
        var mergedOutputPath = Path.Combine(_tempOutputPath, $"{jobId}_merged.mp4");
        
        try
        {
            // Extract each segment to a temporary file
            for (int i = 0; i < segments.Count; i++)
            {
                var segment = segments[i];
                var segmentPath = Path.Combine(_tempOutputPath, $"{jobId}_segment_{i}.mp4");
                segmentFiles.Add(segmentPath);
                
                var duration = segment.End - segment.Start;
                
                // Use ffmpeg to extract the segment via stream copy to keep quality
                var arguments = new List<string>
                {
                    "-y", // Overwrite output file
                    "-ss", segment.Start.ToString("F3", System.Globalization.CultureInfo.InvariantCulture),
                    "-i", inputPath,
                    "-t", duration.ToString("F3", System.Globalization.CultureInfo.InvariantCulture),
                    "-c", "copy", // Stream copy for speed
                    "-avoid_negative_ts", "make_zero", // Ensure proper timestamps
                    segmentPath
                };
                
                var processStartInfo = new ProcessStartInfo
                {
                    FileName = _ffmpegResolver.GetFfmpegPath(),
                    RedirectStandardOutput = true,
                    RedirectStandardError = true,
                    UseShellExecute = false,
                    CreateNoWindow = true
                };
                
                foreach (var arg in arguments)
                {
                    processStartInfo.ArgumentList.Add(arg);
                }
                
                _logger.LogInformation("Extracting segment {Index}/{Total}: {Start}s to {End}s (duration: {Duration}s)", 
                    i + 1, segments.Count, segment.Start, segment.End, duration);
                
                using var process = new Process { StartInfo = processStartInfo };
                process.Start();
                
                var errorOutput = await process.StandardError.ReadToEndAsync();
                await process.WaitForExitAsync();
                
                if (process.ExitCode != 0)
                {
                    _logger.LogError("Failed to extract segment {Index}: {Error}", i, errorOutput);
                    throw new InvalidOperationException($"Failed to extract segment {i + 1}: ffmpeg exited with code {process.ExitCode}");
                }
                
                _logger.LogInformation("Segment {Index} extracted successfully", i + 1);
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
            
            var mergeProcessStartInfo = new ProcessStartInfo
            {
                FileName = _ffmpegResolver.GetFfmpegPath(),
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                UseShellExecute = false,
                CreateNoWindow = true
            };
            
            foreach (var arg in mergeArguments)
            {
                mergeProcessStartInfo.ArgumentList.Add(arg);
            }
            
            _logger.LogInformation("Merging {Count} segments into single file", segmentFiles.Count);
            
            var ffmpegCommand = $"{_ffmpegResolver.GetFfmpegPath()} {string.Join(" ", mergeArguments.Select(a => a.Contains(" ") ? $"\"{a}\"" : a))}";
            _logger.LogInformation("Running ffmpeg command: {Command}", ffmpegCommand);
            
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



    private static EncodingMode DeriveEncodingMode(bool useQualityMode, bool useUltraMode)
    {
        // useUltraMode is ignored; we only support Fast and Quality modes
        if (useQualityMode)
        {
            return EncodingMode.Quality;
        }

        return EncodingMode.Fast;
    }

    private sealed record BitratePlan(double TotalKbps, double VideoKbps, double PayloadBudgetMb, double ContainerReserveMb, double SafetyMarginMb);

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
    public DateTime CreatedAt { get; set; }
    public DateTime? StartedAt { get; set; }
    public DateTime? CompletedAt { get; set; }
    public int? EstimatedSecondsRemaining { get; set; }
    public Process? Process { get; set; }
        // Encoder metadata
        public string? EncoderName { get; set; }
        public bool? EncoderIsHardware { get; set; }
}


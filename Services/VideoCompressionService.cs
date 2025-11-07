using System.Collections.Concurrent;
using System.Diagnostics;
using System.Globalization;
using System.Linq;
using System.Text;
using smart_compressor.Models;
using smart_compressor.CompressionStrategies;

namespace smart_compressor.Services;

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
        _tempOutputPath = configuration["TempPaths:Outputs"] ?? Path.Combine(Path.GetTempPath(), "video-outputs");
        _maxConcurrentJobs = configuration.GetValue<int>("Compression:MaxConcurrentJobs", 2);
        _maxQueueSize = configuration.GetValue<int>("Compression:MaxQueueSize", 10);

        _concurrencyLimiter = new SemaphoreSlim(_maxConcurrentJobs, _maxConcurrentJobs);

        Directory.CreateDirectory(_tempUploadPath);
        Directory.CreateDirectory(_tempOutputPath);
    }

    public async Task<string> CompressVideoAsync(IFormFile videoFile, CompressionRequest request)
    {
        _logger.LogInformation("Compression request received - Codec: {Codec}, TargetSizeMb: {TargetSizeMb}, SourceDuration: {SourceDuration}", 
            request.Codec, request.TargetSizeMb, request.SourceDuration);
        
        var normalizedRequest = NormalizeRequest(request);
        var codecConfig = GetCodecConfig(normalizedRequest.Codec);

        // Calculate bitrates for simple mode with target size
        double? computedTargetKbps = null;
        double? computedVideoKbps = null;
        if (normalizedRequest.TargetSizeMb.HasValue && 
            normalizedRequest.SourceDuration.HasValue && 
            normalizedRequest.SourceDuration.Value > 0)
        {
            var targetSize = normalizedRequest.TargetSizeMb.Value;
            var duration = normalizedRequest.SourceDuration.Value;

            // Calculate target bitrate in bits per second, then convert to kbps
            var targetBitsTotal = (targetSize * 1024 * 1024 * 8);
            var durationSec = duration;
            var totalKbps = targetBitsTotal / durationSec / 1000;

            // Reserve audio bitrate, use remaining for video
            var containerOverheadFactor = codecConfig.FileExtension.Equals(".webm", StringComparison.OrdinalIgnoreCase) ? 0.98 : 0.97;
            var effectiveTargetKbps = totalKbps * containerOverheadFactor;
            var videoKbps = Math.Max(100, effectiveTargetKbps - codecConfig.AudioBitrateKbps);

            _logger.LogInformation("Bitrate calculation: TargetSize={TargetMb}MB, Duration={Duration}s, TotalKbps={TotalKbps}, ContainerOverhead={ContainerOverheadPct}%, EffectiveKbps={EffectiveKbps}, VideoKbps={VideoKbps}, AudioKbps={AudioKbps}",
                targetSize, durationSec, Math.Round(totalKbps, 2), Math.Round((1 - containerOverheadFactor) * 100, 1), effectiveTargetKbps, videoKbps, codecConfig.AudioBitrateKbps);

            // Store for job creation below
            computedTargetKbps = Math.Round(effectiveTargetKbps, 2);
            computedVideoKbps = Math.Round(videoKbps, 2);
        }

        var jobId = Guid.NewGuid().ToString();
        var originalFilename = videoFile.FileName;
        var safeStem = Path.GetFileNameWithoutExtension(string.IsNullOrWhiteSpace(originalFilename) ? jobId : originalFilename);
        var inputPath = Path.Combine(_tempUploadPath, $"{jobId}_{originalFilename}");
        var targetSizePrefix = normalizedRequest.TargetSizeMb.HasValue
            ? $"{normalizedRequest.TargetSizeMb.Value:F0}MB"
            : "auto";
        var outputFilename = $"{targetSizePrefix}_compressed_{safeStem}{codecConfig.FileExtension}";
        var outputPath = Path.Combine(_tempOutputPath, outputFilename);

        await using (var stream = new FileStream(inputPath, FileMode.Create))
        {
            await videoFile.CopyToAsync(stream);
        }

        // Check queue size before accepting new job
        if (_jobQueue.Count >= _maxQueueSize)
        {
            throw new InvalidOperationException($"Queue is full. Maximum queue size is {_maxQueueSize}. Please try again later.");
        }

        // Enable two-pass by default when using target size for better accuracy
        var enableTwoPass = normalizedRequest.TargetSizeMb.HasValue;
        
        var job = new JobMetadata
        {
            JobId = jobId,
            OriginalFilename = originalFilename,
            InputPath = inputPath,
            OutputPath = outputPath,
            OutputFilename = outputFilename,
            OutputMimeType = codecConfig.MimeType,
            Status = "queued",
            Codec = codecConfig.Key,
            ScalePercent = normalizedRequest.ScalePercent,
            TargetSizeMb = normalizedRequest.TargetSizeMb,
            TargetBitrateKbps = computedTargetKbps,
            VideoBitrateKbps = computedVideoKbps,
            SourceDuration = normalizedRequest.SourceDuration,
            TwoPass = enableTwoPass,
            CreatedAt = DateTime.UtcNow
        };

        _jobs[jobId] = job;
        _jobQueue.Enqueue(jobId);

        _ = Task.Run(async () => await ProcessQueueAsync(jobId, normalizedRequest, codecConfig));

        return jobId;
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
        job.Process?.Kill();
        
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

            string? scaleFilter = null;
            if (scalePercent < 100)
            {
                var factor = scalePercent / 100.0;
                var factorStr = factor.ToString(CultureInfo.InvariantCulture);
                scaleFilter = $"scale=trunc(iw*{factorStr}/2)*2:trunc(ih*{factorStr}/2)*2:flags=lanczos";
                
                _logger.LogInformation("Applying resolution scaling for job {JobId}: {ScalePercent}%", jobId, scalePercent);
            }

            var targetBitrateKbps = job.TargetBitrateKbps ?? 0;
            var videoBitrateKbps = job.VideoBitrateKbps.Value;

            job.TargetSizeMb = Math.Round(job.TargetSizeMb.Value, 2);
            job.TargetBitrateKbps = targetBitrateKbps;
            job.VideoBitrateKbps = videoBitrateKbps;

            var arguments = new List<string> { "-y", "-i", job.InputPath };

            if (!string.IsNullOrWhiteSpace(scaleFilter))
            {
                arguments.AddRange(new[] { "-vf", scaleFilter });
            }

            // Prefer a registered compression strategy if available; fall back to legacy builders.
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
                arguments.AddRange(strategy.BuildVideoArgs(videoBitrateKbps));
                arguments.AddRange(strategy.BuildAudioArgs());
                arguments.AddRange(strategy.BuildContainerArgs());
            }
            else
            {
                var videoArgs = BuildSimpleVideoArgs(codec, videoBitrateKbps);
                arguments.AddRange(videoArgs);
                arguments.AddRange(BuildAudioArgs(codec));
                arguments.AddRange(BuildContainerArgs(codec));
            }
            
            // Two-pass encoding for accurate target size
            var useTwoPass = job.TwoPass;
            
            if (useTwoPass)
            {
                _logger.LogInformation("Using two-pass encoding for job {JobId}", jobId);
                await RunTwoPassEncodingAsync(jobId, job, arguments, codec, job.SourceDuration);
            }
            else
            {
                arguments.Add(job.OutputPath);
                await RunSinglePassEncodingAsync(jobId, job, arguments, job.SourceDuration);
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
            job.Process = null;
        }
    }

    private async Task RunSinglePassEncodingAsync(string jobId, JobMetadata job, List<string> arguments, double? totalDuration)
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

        // Check for cancellation
        if (job.Status == "cancelled")
        {
            _logger.LogInformation("Job {JobId} was cancelled", jobId);
            return;
        }

        if (process.ExitCode == 0)
        {
            job.Status = "completed";
            job.Progress = 100;
            job.EstimatedSecondsRemaining = 0;
            
            // Log output file size for verification
            if (File.Exists(job.OutputPath))
            {
                var outputSize = new FileInfo(job.OutputPath).Length;
                var outputSizeMb = outputSize / (1024.0 * 1024.0);
                _logger.LogInformation("Video compression completed for job {JobId} using {Codec}. Output size: {OutputSizeMb:F2} MB (Target: {TargetSizeMb} MB)", 
                    jobId, job.Codec, outputSizeMb, job.TargetSizeMb?.ToString("F2") ?? "N/A");
            }
            else
            {
                _logger.LogInformation("Video compression completed for job {JobId} using {Codec}.", jobId, job.Codec);
            }
        }
        else
        {
            job.Status = "failed";
            job.ErrorMessage = errorBuilder.ToString();
            _logger.LogError("Video compression failed for job {JobId}. Exit code {ExitCode}. Error: {Error}", jobId, process.ExitCode, errorBuilder.ToString());
        }
    }

    private async Task RunTwoPassEncodingAsync(string jobId, JobMetadata job, List<string> baseArguments, CodecConfig codec, double? totalDuration)
    {
        var passLogFile = Path.Combine(_tempOutputPath, $"{jobId}_ffmpeg2pass");

        // First pass
        _logger.LogInformation("Starting first pass for job {JobId}", jobId);
        var pass1Args = new List<string>(baseArguments);

        // Ask strategy for pass-specific extras when available
        var strategy = _strategyFactory?.GetStrategy(codec.Key);
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
        var normalized = new CompressionRequest
        {
            Codec = NormalizeCodec(request.Codec),
            ScalePercent = request.ScalePercent,
            TargetSizeMb = request.TargetSizeMb,
            SourceDuration = request.SourceDuration
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



    private static List<string> BuildSimpleVideoArgs(CodecConfig codec, double videoBitrateKbps)
    {
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
                args.AddRange(new[] { "-c:v", codec.VideoCodec, "-preset", "medium", "-pix_fmt", "yuv420p", "-tag:v", "hvc1", "-x265-params", "vbv-bufsize=" + buffer + ":vbv-maxrate=" + maxRate });
                break;
            case "vp9":
                args.AddRange(new[] { "-c:v", codec.VideoCodec, "-deadline", "good", "-cpu-used", "2", "-row-mt", "1", "-tile-columns", "1" });
                break;
            case "av1":
                args.AddRange(new[] { "-c:v", codec.VideoCodec, "-cpu-used", "5", "-row-mt", "1" });
                break;
            default:
                args.AddRange(new[] { "-c:v", codec.VideoCodec, "-preset", "medium", "-pix_fmt", "yuv420p" });
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



    private static string NormalizeCodec(string? codec)
    {
        return codec?.ToLowerInvariant() switch
        {
            "hevc" => "h265",
            "h265" => "h265",
            "vp9" => "vp9",
            "av1" => "av1",
            _ => "h264"
        };
    }

    private sealed class CodecConfig
    {
        public string Key { get; init; } = "h264";
        public string VideoCodec { get; init; } = "libx264";
        public string AudioCodec { get; init; } = "aac";
        public string FileExtension { get; init; } = ".mp4";
        public string MimeType { get; init; } = "video/mp4";
        public int AudioBitrateKbps { get; init; } = 128;
    }
}

public class JobMetadata
{
    public string JobId { get; set; } = string.Empty;
    public string OriginalFilename { get; set; } = string.Empty;
    public string InputPath { get; set; } = string.Empty;
    public string OutputPath { get; set; } = string.Empty;
    public string OutputFilename { get; set; } = string.Empty;
    public string OutputMimeType { get; set; } = "video/mp4";
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
}


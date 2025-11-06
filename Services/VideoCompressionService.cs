using System.Collections.Concurrent;
using System.Diagnostics;
using System.Globalization;
using System.Linq;
using System.Text;
using smart_compressor.Models;

namespace smart_compressor.Services;

public class VideoCompressionService
{
    private readonly ConcurrentDictionary<string, JobMetadata> _jobs = new();
    private readonly ConcurrentQueue<string> _jobQueue = new();
    private readonly SemaphoreSlim _concurrencyLimiter;
    private readonly string _tempUploadPath;
    private readonly string _tempOutputPath;
    private readonly ILogger<VideoCompressionService> _logger;
    private readonly FfmpegPathResolver _ffmpegResolver;
    private readonly int _maxConcurrentJobs;
    private readonly int _maxQueueSize;

    public VideoCompressionService(IConfiguration configuration, ILogger<VideoCompressionService> logger, FfmpegPathResolver ffmpegResolver)
    {
        _logger = logger;
        _ffmpegResolver = ffmpegResolver;
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
        _logger.LogInformation("Compression request received - Mode: {Mode}, Codec: {Codec}, TargetSizeMb: {TargetSizeMb}, SourceDuration: {SourceDuration}, Crf: {Crf}", 
            request.Mode, request.Codec, request.TargetSizeMb, request.SourceDuration, request.Crf);
        
        var normalizedRequest = NormalizeRequest(request);
        var codecConfig = GetCodecConfig(normalizedRequest.Codec);

        // Determine whether to treat this as simple mode. For "auto" we choose simple
        // only when a target size and source duration are available.
        var isSimpleMode = normalizedRequest.Mode == "simple" ||
            (normalizedRequest.Mode == "auto" &&
             normalizedRequest.TargetSizeMb.HasValue &&
             normalizedRequest.SourceDuration.HasValue &&
             normalizedRequest.SourceDuration.Value > 0);

        if (isSimpleMode)
        {
            // Calculate target bitrate in bits per second, then convert to kbps
            var targetBitsTotal = (normalizedRequest.TargetSizeMb.Value * 1024 * 1024 * 8);
            var durationSec = normalizedRequest.SourceDuration.Value;
            var totalKbps = targetBitsTotal / durationSec / 1000;
            
            // Reserve audio bitrate, use remaining for video
            // Container overhead varies by format:
            // MP4: ~2-3%, WebM: ~1-2%
            // Using 97% to be slightly conservative while maximizing accuracy
            var containerOverheadFactor = codecConfig.FileExtension.Equals(".webm", StringComparison.OrdinalIgnoreCase) ? 0.98 : 0.97;
            var effectiveTargetKbps = totalKbps * containerOverheadFactor;
            var videoKbps = Math.Max(100, effectiveTargetKbps - codecConfig.AudioBitrateKbps);
            
            normalizedRequest.TargetBitrateKbps = Math.Round(effectiveTargetKbps, 2);
            normalizedRequest.VideoBitrateKbps = Math.Round(videoKbps, 2);
            
            _logger.LogInformation("Simple mode bitrate calculation: TargetSize={TargetMb}MB, Duration={Duration}s, TotalKbps={TotalKbps}, ContainerOverhead={ContainerOverheadPct}%, EffectiveKbps={EffectiveKbps}, VideoKbps={VideoKbps}, AudioKbps={AudioKbps}",
                normalizedRequest.TargetSizeMb.Value, durationSec, Math.Round(totalKbps, 2), Math.Round((1 - containerOverheadFactor) * 100, 1), effectiveTargetKbps, videoKbps, codecConfig.AudioBitrateKbps);
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

        // Enable two-pass by default when using simple mode with target size for better accuracy
        var enableTwoPass = normalizedRequest.TwoPass || (isSimpleMode && normalizedRequest.TargetSizeMb.HasValue);
        
        var job = new JobMetadata
        {
            JobId = jobId,
            OriginalFilename = originalFilename,
            InputPath = inputPath,
            OutputPath = outputPath,
            OutputFilename = outputFilename,
            OutputMimeType = codecConfig.MimeType,
            Status = "queued",
            Mode = normalizedRequest.Mode,
            Codec = codecConfig.Key,
            Crf = normalizedRequest.Crf,
            ScalePercent = normalizedRequest.ScalePercent,
            TargetSizeMb = normalizedRequest.TargetSizeMb,
            TargetBitrateKbps = normalizedRequest.TargetBitrateKbps,
            VideoBitrateKbps = normalizedRequest.VideoBitrateKbps,
            SourceDuration = normalizedRequest.SourceDuration,
            SourceWidth = normalizedRequest.SourceWidth,
            SourceHeight = normalizedRequest.SourceHeight,
            OriginalSizeBytes = normalizedRequest.OriginalSizeBytes,
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
            // Decide effective mode. If request.Mode is "auto" choose simple only when
            // target size and duration are available; otherwise treat as advanced.
            var mode = request.Mode?.ToLowerInvariant() switch
            {
                "simple" => "simple",
                "auto" => (request.TargetSizeMb.HasValue && request.SourceDuration.HasValue && request.SourceDuration.Value > 0) ? "simple" : "advanced",
                _ => "advanced"
            };

            job.Mode = mode;

			// Apply scaling for both modes; in auto mode, decide scale if not provided
			int scalePercent;
			int? autoCrfOverride = null;
			if (request.Mode?.Equals("auto", StringComparison.OrdinalIgnoreCase) == true && request.ScalePercent is null)
			{
				var auto = DecideAutoScaleAndCrf(request, codec);
				scalePercent = Math.Clamp(auto.ScalePercent, 10, 100);
				autoCrfOverride = auto.CrfOverride;
				_logger.LogInformation("Auto mode decision for job {JobId}: scale={Scale}%{CrfNote}", jobId, scalePercent, autoCrfOverride.HasValue ? $", crfAdj={autoCrfOverride.Value}" : string.Empty);
			}
			else
			{
				scalePercent = Math.Clamp(request.ScalePercent ?? 100, 10, 100);
			}
            job.ScalePercent = scalePercent;

            string? scaleFilter = null;
            if (scalePercent < 100)
            {
                var factor = scalePercent / 100.0;
                var factorStr = factor.ToString(CultureInfo.InvariantCulture);
                scaleFilter = $"scale=trunc(iw*{factorStr}/2)*2:trunc(ih*{factorStr}/2)*2:flags=lanczos";
                
                _logger.LogInformation("Applying resolution scaling for job {JobId}: {ScalePercent}% (mode: {Mode})", jobId, scalePercent, mode);
            }

            double? targetBitrateKbps = null;
            double? videoBitrateKbps = null;

            if (mode == "simple")
            {
                var duration = request.SourceDuration;
                var targetSize = request.TargetSizeMb;

                _logger.LogInformation("Simple mode check for job {JobId}: Duration={Duration}, TargetSize={TargetSize}, HasBitrates={HasBitrates}", 
                    jobId, duration, targetSize, request.VideoBitrateKbps.HasValue);

                if (duration.HasValue && duration.Value > 0 && targetSize.HasValue && targetSize.Value > 0)
                {
                    // Use pre-calculated bitrates from request normalization
                    targetBitrateKbps = request.TargetBitrateKbps ?? 0;
                    videoBitrateKbps = request.VideoBitrateKbps ?? 0;

                    job.TargetSizeMb = Math.Round(targetSize.Value, 2);
                    job.TargetBitrateKbps = targetBitrateKbps;
                    job.VideoBitrateKbps = videoBitrateKbps;
                    job.Crf = null;
                }
                else
                {
                    _logger.LogWarning("Insufficient metadata for simple mode on job {JobId} (Duration={Duration}, TargetSize={TargetSize}). Falling back to advanced mode.", 
                        jobId, duration, targetSize);
                    mode = "advanced";
                    job.Mode = "advanced";
                }
            }

            var arguments = new List<string> { "-y", "-i", job.InputPath };

            if (!string.IsNullOrWhiteSpace(scaleFilter))
            {
                arguments.AddRange(new[] { "-vf", scaleFilter });
            }

			if (mode == "advanced")
            {
				var effectiveCrf = autoCrfOverride ?? request.Crf;
				var advancedResult = BuildAdvancedVideoArgs(codec, effectiveCrf);
                job.Crf = advancedResult.AppliedCrf;
                job.TargetSizeMb = null;
                job.TargetBitrateKbps = null;
                job.VideoBitrateKbps = null;
                arguments.AddRange(advancedResult.Args);
            }
            else
            {
                var simpleArgs = BuildSimpleVideoArgs(codec, videoBitrateKbps!.Value);
                arguments.AddRange(simpleArgs);
            }

            arguments.AddRange(BuildAudioArgs(codec));
            arguments.AddRange(BuildContainerArgs(codec));
            
            // Two-pass encoding for simple mode when targeting specific size
            var useTwoPass = job.TwoPass && mode == "simple" && videoBitrateKbps.HasValue;
            
            if (useTwoPass)
            {
                _logger.LogInformation("Using two-pass encoding for job {JobId}", jobId);
                await RunTwoPassEncodingAsync(jobId, job, arguments, codec, request.SourceDuration);
            }
            else
            {
                arguments.Add(job.OutputPath);
                await RunSinglePassEncodingAsync(jobId, job, arguments, request.SourceDuration);
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
                _logger.LogInformation("Video compression completed for job {JobId} using {Codec} ({Mode} mode). Output size: {OutputSizeMb:F2} MB (Target: {TargetSizeMb} MB)", 
                    jobId, job.Codec, job.Mode, outputSizeMb, job.TargetSizeMb?.ToString("F2") ?? "N/A");
            }
            else
            {
                _logger.LogInformation("Video compression completed for job {JobId} using {Codec} ({Mode} mode).", jobId, job.Codec, job.Mode);
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
        
        // Add pass-specific arguments
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
        
        if (codec.Key == "h264" || codec.Key == "h265")
        {
            pass2Args.AddRange(new[] { "-pass", "2", "-passlogfile", passLogFile });
        }
        else if (codec.Key == "vp9" || codec.Key == "av1")
        {
            pass2Args.AddRange(new[] { "-pass", "2", "-passlogfile", passLogFile });
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
            // Preserve "auto" explicitly so the pipeline can decide later whether to
            // use simple or advanced behavior based on available metadata.
            Mode = request.Mode?.ToLowerInvariant() switch
            {
                "simple" => "simple",
                "auto" => "auto",
                _ => "advanced"
            },
            Codec = NormalizeCodec(request.Codec),
            Crf = request.Crf,
            ScalePercent = request.ScalePercent,
            TargetSizeMb = request.TargetSizeMb,
            SourceDuration = request.SourceDuration,
            SourceWidth = request.SourceWidth,
            SourceHeight = request.SourceHeight,
            OriginalSizeBytes = request.OriginalSizeBytes
        };

        normalized.Crf = Math.Clamp(normalized.Crf ?? 28, 18, 45);
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

    private static (List<string> Args, int AppliedCrf) BuildAdvancedVideoArgs(CodecConfig codec, int? requestedCrf)
    {
        var args = new List<string>();
        int appliedCrf;

        switch (codec.Key)
        {
            case "h265":
                appliedCrf = MapCrf(requestedCrf, 20, 37, 28, true);
                args.AddRange(new[] { "-c:v", codec.VideoCodec, "-preset", "medium", "-crf", appliedCrf.ToString(CultureInfo.InvariantCulture), "-tag:v", "hvc1", "-pix_fmt", "yuv420p" });
                break;
            case "vp9":
                appliedCrf = MapCrf(requestedCrf, 32, 45, 36, true);
                args.AddRange(new[] { "-c:v", codec.VideoCodec, "-crf", appliedCrf.ToString(CultureInfo.InvariantCulture), "-b:v", "0", "-deadline", "good", "-cpu-used", "2", "-row-mt", "1", "-tile-columns", "1" });
                break;
            case "av1":
                appliedCrf = MapCrf(requestedCrf, 28, 45, 32, true);
                args.AddRange(new[] { "-c:v", codec.VideoCodec, "-crf", appliedCrf.ToString(CultureInfo.InvariantCulture), "-b:v", "0", "-cpu-used", "4", "-row-mt", "1" });
                break;
            default:
                appliedCrf = MapCrf(requestedCrf, 18, 45, 28, false);
                args.AddRange(new[] { "-c:v", codec.VideoCodec, "-preset", "veryfast", "-crf", appliedCrf.ToString(CultureInfo.InvariantCulture), "-pix_fmt", "yuv420p" });
                break;
        }

        return (args, appliedCrf);
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

    private static int MapCrf(int? requestedCrf, int codecMin, int codecMax, int codecDefault, bool scaleFromSlider)
    {
        if (requestedCrf is null)
        {
            return codecDefault;
        }

        var clamped = Math.Clamp(requestedCrf.Value, 18, 45);

        if (!scaleFromSlider)
        {
            return Math.Clamp(clamped, codecMin, codecMax);
        }

        var normalized = (clamped - 18d) / (45d - 18d);
        var mapped = codecMin + normalized * (codecMax - codecMin);
        return (int)Math.Round(mapped);
    }

	private static (int ScalePercent, int? CrfOverride) DecideAutoScaleAndCrf(CompressionRequest request, CodecConfig codec)
	{
		var srcW = Math.Max(16, request.SourceWidth ?? 1920);
		var srcH = Math.Max(16, request.SourceHeight ?? 1080);
		var fps = Math.Clamp((int)Math.Round((double)(request.SourceDuration.HasValue && request.SourceDuration.Value > 0 ? 30 : 30)), 10, 120);

		double? videoKbps = request.VideoBitrateKbps;
		if (!videoKbps.HasValue && request.TargetBitrateKbps.HasValue)
		{
			videoKbps = Math.Max(100, request.TargetBitrateKbps.Value - codec.AudioBitrateKbps);
		}
		if (!videoKbps.HasValue && request.OriginalSizeBytes.HasValue && request.SourceDuration.HasValue && request.SourceDuration.Value > 0)
		{
			var totalKbps = (request.OriginalSizeBytes.Value * 8.0) / 1000.0 / request.SourceDuration.Value;
			videoKbps = Math.Max(100, totalKbps - codec.AudioBitrateKbps);
		}
		videoKbps ??= 2000; // reasonable fallback

		(double targetBpp, double floorBpp) = codec.Key switch
		{
			"h265" => (0.070, 0.050),
			"vp9" => (0.060, 0.045),
			"av1" => (0.055, 0.040),
			_ => (0.100, 0.070) // h264
		};

		var candidates = new List<int>();
		var standardHeights = new[] { 2160, 1440, 1080, 900, 720, 540, 480, 360 };
		candidates.Add(srcH);
		foreach (var h in standardHeights)
		{
			if (h <= srcH && !candidates.Contains(h))
			{
				candidates.Add(h);
			}
		}
		candidates = candidates.Distinct().OrderByDescending(h => h).ToList();

		int chosenH = candidates.Last(); // default to smallest
		double chosenBpp = 0;
		foreach (var h in candidates)
		{
			var w = (int)Math.Round(srcW * (h / (double)srcH));
			var pixelsPerSecond = (double)w * h * fps;
			if (pixelsPerSecond <= 0)
			{
				continue;
			}
			var bpp = (videoKbps.Value * 1000.0) / pixelsPerSecond;
			if (bpp >= floorBpp)
			{
				chosenH = h;
				chosenBpp = bpp;
				break; // highest height that meets floor
			}
		}

		var scalePercent = (int)Math.Clamp(Math.Round((double)chosenH * 100.0 / srcH), 10, 100);

		int? crfOverride = null;
		// If advanced mode ends up being used, bias CRF toward quality when we downscale hard or bpp is tight
		if (scalePercent <= 50)
		{
			crfOverride = (request.Crf ?? 28) - 2;
		}
		else if (chosenBpp < targetBpp)
		{
			crfOverride = (request.Crf ?? 28) - 1;
		}

		return (scalePercent, crfOverride);
	}

    private static double? ParseFfmpegProgress(string line, double? totalDuration)
    {
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
    public string Mode { get; set; } = "advanced";
    public string Codec { get; set; } = "h264";
    public int? Crf { get; set; }
    public int? ScalePercent { get; set; }
    public double? TargetSizeMb { get; set; }
    public double? TargetBitrateKbps { get; set; }
    public double? VideoBitrateKbps { get; set; }
    public double? SourceDuration { get; set; }
    public int? SourceWidth { get; set; }
    public int? SourceHeight { get; set; }
    public long? OriginalSizeBytes { get; set; }
    public double Progress { get; set; } = 0;
    public string? ErrorMessage { get; set; }
    public bool TwoPass { get; set; } = false;
    public DateTime CreatedAt { get; set; }
    public DateTime? StartedAt { get; set; }
    public DateTime? CompletedAt { get; set; }
    public int? EstimatedSecondsRemaining { get; set; }
    public Process? Process { get; set; }
}


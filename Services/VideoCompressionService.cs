using System.Collections.Concurrent;
using System.Diagnostics;
using System.Globalization;
using System.Text;
using smart_compressor.Models;

namespace smart_compressor.Services;

public class VideoCompressionService
{
    private readonly ConcurrentDictionary<string, JobMetadata> _jobs = new();
    private readonly string _tempUploadPath;
    private readonly string _tempOutputPath;
    private readonly ILogger<VideoCompressionService> _logger;

    public VideoCompressionService(IConfiguration configuration, ILogger<VideoCompressionService> logger)
    {
        _logger = logger;
        _tempUploadPath = configuration["TempPaths:Uploads"] ?? Path.Combine(Path.GetTempPath(), "video-uploads");
        _tempOutputPath = configuration["TempPaths:Outputs"] ?? Path.Combine(Path.GetTempPath(), "video-outputs");

        Directory.CreateDirectory(_tempUploadPath);
        Directory.CreateDirectory(_tempOutputPath);
    }

    public async Task<string> CompressVideoAsync(IFormFile videoFile, CompressionRequest request)
    {
        _logger.LogInformation("Compression request received - Mode: {Mode}, Codec: {Codec}, TargetSizeMb: {TargetSizeMb}, SourceDuration: {SourceDuration}, Crf: {Crf}", 
            request.Mode, request.Codec, request.TargetSizeMb, request.SourceDuration, request.Crf);
        
        var normalizedRequest = NormalizeRequest(request);
        var codecConfig = GetCodecConfig(normalizedRequest.Codec);

        if (normalizedRequest.Mode == "simple" &&
            normalizedRequest.TargetSizeMb.HasValue &&
            normalizedRequest.SourceDuration.HasValue &&
            normalizedRequest.SourceDuration.Value > 0)
        {
            // Calculate target bitrate in bits per second, then convert to kbps
            var targetBitsTotal = (normalizedRequest.TargetSizeMb.Value * 1024 * 1024 * 8);
            var durationSec = normalizedRequest.SourceDuration.Value;
            var totalKbps = targetBitsTotal / durationSec / 1000;
            
            // Reserve audio bitrate, use remaining for video
            // Be conservative - aim for 90% of target to account for container overhead
            var effectiveTargetKbps = totalKbps * 0.90;
            var videoKbps = Math.Max(100, effectiveTargetKbps - codecConfig.AudioBitrateKbps);
            
            normalizedRequest.TargetBitrateKbps = Math.Round(effectiveTargetKbps, 2);
            normalizedRequest.VideoBitrateKbps = Math.Round(videoKbps, 2);
            
            _logger.LogInformation("Simple mode bitrate calculation: TargetSize={TargetMb}MB, Duration={Duration}s, TotalKbps={TotalKbps}, EffectiveKbps={EffectiveKbps}, VideoKbps={VideoKbps}, AudioKbps={AudioKbps}",
                normalizedRequest.TargetSizeMb.Value, durationSec, Math.Round(totalKbps, 2), effectiveTargetKbps, videoKbps, codecConfig.AudioBitrateKbps);
        }

        var jobId = Guid.NewGuid().ToString();
        var originalFilename = videoFile.FileName;
        var safeStem = Path.GetFileNameWithoutExtension(string.IsNullOrWhiteSpace(originalFilename) ? jobId : originalFilename);
        var inputPath = Path.Combine(_tempUploadPath, $"{jobId}_{originalFilename}");
        var outputFilename = $"{jobId}_compressed_{safeStem}{codecConfig.FileExtension}";
        var outputPath = Path.Combine(_tempOutputPath, outputFilename);

        await using (var stream = new FileStream(inputPath, FileMode.Create))
        {
            await videoFile.CopyToAsync(stream);
        }

        var job = new JobMetadata
        {
            JobId = jobId,
            OriginalFilename = originalFilename,
            InputPath = inputPath,
            OutputPath = outputPath,
            OutputFilename = outputFilename,
            OutputMimeType = codecConfig.MimeType,
            Status = "processing",
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
            OriginalSizeBytes = normalizedRequest.OriginalSizeBytes
        };

        _jobs[jobId] = job;

        _ = Task.Run(async () => await RunFfmpegCompressionAsync(jobId, job, normalizedRequest, codecConfig));

        return jobId;
    }

    private async Task RunFfmpegCompressionAsync(string jobId, JobMetadata job, CompressionRequest request, CodecConfig codec)
    {
        try
        {
            var mode = string.Equals(request.Mode, "simple", StringComparison.OrdinalIgnoreCase) ? "simple" : "advanced";
            job.Mode = mode;

            // Apply scaling for both modes if specified
            var scalePercent = Math.Clamp(request.ScalePercent ?? 100, 10, 100);
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
                var advancedResult = BuildAdvancedVideoArgs(codec, request.Crf);
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
            arguments.Add(job.OutputPath);

            var commandLine = $"ffmpeg {string.Join(" ", arguments.Select(a => a.Contains(" ") ? $"\"{a}\"" : a))}";
            _logger.LogInformation("Executing FFmpeg command for job {JobId}: {Command}", jobId, commandLine);

            var processStartInfo = new ProcessStartInfo
            {
                FileName = "ffmpeg",
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

            var outputBuilder = new StringBuilder();
            var errorBuilder = new StringBuilder();

            process.OutputDataReceived += (_, e) =>
            {
                if (!string.IsNullOrEmpty(e.Data))
                {
                    outputBuilder.AppendLine(e.Data);
                }
            };

            process.ErrorDataReceived += (_, e) =>
            {
                if (!string.IsNullOrEmpty(e.Data))
                {
                    errorBuilder.AppendLine(e.Data);
                }
            };

            process.Start();
            process.BeginOutputReadLine();
            process.BeginErrorReadLine();

            await process.WaitForExitAsync();

            if (process.ExitCode == 0)
            {
                job.Status = "completed";
                
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
        catch (Exception ex)
        {
            job.Status = "failed";
            job.ErrorMessage = ex.Message;
            _logger.LogError(ex, "Exception during video compression for job {JobId}", jobId);
        }
    }

    public JobMetadata? GetJob(string jobId)
    {
        _jobs.TryGetValue(jobId, out var job);
        return job;
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
            Mode = string.Equals(request.Mode, "simple", StringComparison.OrdinalIgnoreCase) ? "simple" : "advanced",
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
        // Constrain maxrate tightly to prevent overshoot - allow only 5% variance
        var maxRate = Math.Round(targetBitrate * 1.05);
        // Use smaller buffer for tighter control
        var buffer = Math.Round(targetBitrate * 1.5);

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
            "-minrate", $"{Math.Round(targetBitrate * 0.95)}k"
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
    public string? ErrorMessage { get; set; }
}


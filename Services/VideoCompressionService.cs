using System.Collections.Concurrent;
using System.Diagnostics;
using System.Globalization;

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

        // Ensure directories exist
        Directory.CreateDirectory(_tempUploadPath);
        Directory.CreateDirectory(_tempOutputPath);
    }

    public async Task<string> CompressVideoAsync(IFormFile videoFile, int? crf = null, int? scalePercent = null)
    {
        var jobId = Guid.NewGuid().ToString();
        var originalFilename = videoFile.FileName;
        var inputPath = Path.Combine(_tempUploadPath, $"{jobId}_{originalFilename}");
        var outputPath = Path.Combine(_tempOutputPath, $"{jobId}_compressed_{Path.GetFileNameWithoutExtension(originalFilename)}.mp4");

        // Save uploaded file
        using (var stream = new FileStream(inputPath, FileMode.Create))
        {
            await videoFile.CopyToAsync(stream);
        }

        // Store job metadata
        _jobs[jobId] = new JobMetadata
        {
            JobId = jobId,
            OriginalFilename = originalFilename,
            InputPath = inputPath,
            OutputPath = outputPath,
            Status = "processing"
        };

        // Run FFmpeg compression asynchronously
        _ = Task.Run(async () => await RunFFmpegCompressionAsync(jobId, inputPath, outputPath, crf, scalePercent));

        return jobId;
    }

    private async Task RunFFmpegCompressionAsync(string jobId, string inputPath, string outputPath, int? crf, int? scalePercent)
    {
        try
        {
            var crfValue = Math.Clamp(crf ?? 28, 18, 45);
            var scaleValue = Math.Clamp(scalePercent ?? 100, 10, 100);
            string vfArg = string.Empty;
            if (scaleValue < 100)
            {
                var factor = scaleValue / 100.0;
                var factorStr = factor.ToString(CultureInfo.InvariantCulture);
                vfArg = $" -vf \"scale=trunc(iw*{factorStr}/2)*2:trunc(ih*{factorStr}/2)*2:flags=lanczos\"";
            }

            var processStartInfo = new ProcessStartInfo
            {
                FileName = "ffmpeg",
                Arguments = $"-y -i \"{inputPath}\" -c:v libx264 -preset veryfast -crf {crfValue}{vfArg} -c:a aac -b:a 128k -movflags +faststart \"{outputPath}\"",
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                UseShellExecute = false,
                CreateNoWindow = true
            };

            using var process = new Process { StartInfo = processStartInfo };
            
            var outputBuilder = new System.Text.StringBuilder();
            var errorBuilder = new System.Text.StringBuilder();

            process.OutputDataReceived += (sender, e) =>
            {
                if (!string.IsNullOrEmpty(e.Data))
                    outputBuilder.AppendLine(e.Data);
            };

            process.ErrorDataReceived += (sender, e) =>
            {
                if (!string.IsNullOrEmpty(e.Data))
                    errorBuilder.AppendLine(e.Data);
            };

            process.Start();
            process.BeginOutputReadLine();
            process.BeginErrorReadLine();

            await process.WaitForExitAsync();

            if (process.ExitCode == 0)
            {
                if (_jobs.TryGetValue(jobId, out var job))
                {
                    job.Status = "completed";
                }
                _logger.LogInformation("Video compression completed for job {JobId}", jobId);
            }
            else
            {
                if (_jobs.TryGetValue(jobId, out var job))
                {
                    job.Status = "failed";
                    job.ErrorMessage = errorBuilder.ToString();
                }
                _logger.LogError("Video compression failed for job {JobId}. Error: {Error}", jobId, errorBuilder.ToString());
            }
        }
        catch (Exception ex)
        {
            if (_jobs.TryGetValue(jobId, out var job))
            {
                job.Status = "failed";
                job.ErrorMessage = ex.Message;
            }
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
        if (_jobs.TryGetValue(jobId, out var job))
        {
            try
            {
                if (File.Exists(job.InputPath))
                    File.Delete(job.InputPath);
                
                if (File.Exists(job.OutputPath))
                    File.Delete(job.OutputPath);

                _jobs.TryRemove(jobId, out _);
                _logger.LogInformation("Cleaned up files for job {JobId}", jobId);
            }
            catch (Exception ex)
            {
                _logger.LogError(ex, "Error cleaning up files for job {JobId}", jobId);
            }
        }
    }
}

public class JobMetadata
{
    public string JobId { get; set; } = string.Empty;
    public string OriginalFilename { get; set; } = string.Empty;
    public string InputPath { get; set; } = string.Empty;
    public string OutputPath { get; set; } = string.Empty;
    public string Status { get; set; } = string.Empty;
    public string? ErrorMessage { get; set; }
}



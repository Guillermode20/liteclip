using System;
using System.Collections.Generic;
using System.IO;
using System.Threading;
using System.Threading.Tasks;
using liteclip.CompressionStrategies;
using liteclip.Models;

namespace liteclip.Services;

public sealed class VideoEncodingPipeline : IVideoEncodingPipeline
{
    private readonly IFfmpegRunner _ffmpegRunner;
    private readonly ILogger<VideoEncodingPipeline> _logger;

    public VideoEncodingPipeline(IFfmpegRunner ffmpegRunner, ILogger<VideoEncodingPipeline> logger)
    {
        _ffmpegRunner = ffmpegRunner ?? throw new ArgumentNullException(nameof(ffmpegRunner));
        _logger = logger ?? throw new ArgumentNullException(nameof(logger));
    }

    public async Task<bool> RunSinglePassEncodingAsync(
        string jobId,
        JobMetadata job,
        IReadOnlyList<string> arguments,
        double? totalDuration,
        CancellationToken cancellationToken = default)
    {
        var result = await _ffmpegRunner.RunAsync(
            jobId,
            arguments,
            totalDuration,
            1,
            1,
            update =>
            {
                if (update.Percent.HasValue)
                {
                    job.Progress = Math.Clamp(update.Percent.Value, 0, 100);
                }

                if (update.EstimatedSecondsRemaining.HasValue)
                {
                    job.EstimatedSecondsRemaining = update.EstimatedSecondsRemaining.Value;
                }
            },
            process => job.Process = process,
            cancellationToken);

        if (job.Status == "cancelled")
        {
            _logger.LogInformation("Job {JobId} was cancelled", jobId);
            return false;
        }

        if (result.ExitCode == 0)
        {
            if (File.Exists(job.OutputPath))
            {
                var outputSize = new FileInfo(job.OutputPath).Length;
                job.OutputSizeBytes = outputSize;
            }

            return true;
        }

        job.Status = "failed";
        job.ErrorMessage = result.StandardError;
        _logger.LogError(
            "Video compression failed for job {JobId}. Exit code {ExitCode}. Error: {Error}",
            jobId,
            result.ExitCode,
            result.StandardError);

        return false;
    }

    public async Task<bool> RunTwoPassEncodingAsync(
        string jobId,
        JobMetadata job,
        IReadOnlyList<string> baseArguments,
        string codecKey,
        string fileExtension,
        double? totalDuration,
        string tempOutputPath,
        ICompressionStrategy? strategy,
        CancellationToken cancellationToken = default)
    {
        var passLogFile = Path.Combine(tempOutputPath, $"{jobId}_ffmpeg2pass");

        try
        {
            var pass1Args = BuildFirstPassArgs(baseArguments, codecKey, fileExtension, passLogFile, strategy);
            var success = await RunPassAsync(jobId, job, pass1Args, totalDuration, 1, 2, cancellationToken);
            if (!success)
            {
                return false;
            }

            var pass2Args = BuildSecondPassArgs(baseArguments, codecKey, fileExtension, passLogFile, strategy, job.OutputPath);
            success = await RunPassAsync(jobId, job, pass2Args, totalDuration, 2, 2, cancellationToken);
            if (!success)
            {
                return false;
            }

            if (File.Exists(job.OutputPath))
            {
                var outputSize = new FileInfo(job.OutputPath).Length;
                job.OutputSizeBytes = outputSize;
                var outputSizeMb = outputSize / (1024.0 * 1024.0);
                _logger.LogInformation(
                    "Two-pass encoding produced {OutputSizeMb:F2} MB for job {JobId} (Target {TargetSizeMb} MB)",
                    outputSizeMb,
                    jobId,
                    job.TargetSizeMb?.ToString("F2") ?? "N/A");
            }

            return true;
        }
        finally
        {
            CleanupPassLogs(jobId, tempOutputPath);
        }
    }

    private async Task<bool> RunPassAsync(
        string jobId,
        JobMetadata job,
        IReadOnlyList<string> arguments,
        double? totalDuration,
        int passNumber,
        int totalPasses,
        CancellationToken cancellationToken)
    {
        var result = await _ffmpegRunner.RunAsync(
            jobId,
            arguments,
            totalDuration,
            passNumber,
            totalPasses,
            update =>
            {
                if (update.Percent.HasValue)
                {
                    job.Progress = Math.Clamp(update.Percent.Value, 0, 100);
                }

                if (update.EstimatedSecondsRemaining.HasValue)
                {
                    job.EstimatedSecondsRemaining = update.EstimatedSecondsRemaining.Value;
                }
            },
            process => job.Process = process,
            cancellationToken);

        if (job.Status == "cancelled")
        {
            _logger.LogInformation("Job {JobId} was cancelled during pass {Pass}", jobId, passNumber);
            return false;
        }

        if (result.ExitCode != 0)
        {
            job.Status = "failed";
            job.ErrorMessage = $"Pass {passNumber} failed: {result.StandardError}";
            _logger.LogError(
                "Pass {Pass} failed for job {JobId}. Exit code {ExitCode}",
                passNumber,
                jobId,
                result.ExitCode);
            return false;
        }

        return true;
    }

    private static IReadOnlyList<string> BuildFirstPassArgs(
        IReadOnlyList<string> baseArguments,
        string codecKey,
        string fileExtension,
        string passLogFile,
        ICompressionStrategy? strategy)
    {
        var pass1Args = new List<string>(baseArguments);

        if (strategy != null)
        {
            pass1Args.AddRange(strategy.GetPassExtras(1, passLogFile));
        }
        else
        {
            if (codecKey == "h264" || codecKey == "h265")
            {
                pass1Args.AddRange(new[] { "-pass", "1", "-passlogfile", passLogFile, "-f", "mp4" });
            }
            else if (codecKey == "vp9" || codecKey == "av1")
            {
                pass1Args.AddRange(new[] { "-pass", "1", "-passlogfile", passLogFile, "-f", "webm" });
            }
        }

        EnsurePass1NullOutput(pass1Args);
        return pass1Args;
    }

    private static IReadOnlyList<string> BuildSecondPassArgs(
        IReadOnlyList<string> baseArguments,
        string codecKey,
        string fileExtension,
        string passLogFile,
        ICompressionStrategy? strategy,
        string outputPath)
    {
        var pass2Args = new List<string>(baseArguments);

        if (strategy != null)
        {
            pass2Args.AddRange(strategy.GetPassExtras(2, passLogFile));
        }
        else
        {
            if (codecKey == "h264" || codecKey == "h265" || codecKey == "vp9" || codecKey == "av1")
            {
                pass2Args.AddRange(new[] { "-pass", "2", "-passlogfile", passLogFile });
            }
        }

        pass2Args.Add(outputPath);
        return pass2Args;
    }

    private static void EnsurePass1NullOutput(List<string> args)
    {
        if (args == null)
        {
            return;
        }

        for (int i = args.Count - 1; i >= 0; i--)
        {
            if (string.Equals(args[i], "-f", StringComparison.OrdinalIgnoreCase) && i + 1 < args.Count)
            {
                args.RemoveAt(i + 1);
                args.RemoveAt(i);
            }
        }

        args.RemoveAll(a =>
            string.Equals(a, "NUL", StringComparison.OrdinalIgnoreCase) ||
            string.Equals(a, "/dev/null", StringComparison.OrdinalIgnoreCase) ||
            string.Equals(a, "-", StringComparison.Ordinal));

        args.Add("-f");
        args.Add("null");
        args.Add("-");
    }

    private void CleanupPassLogs(string jobId, string tempOutputPath)
    {
        try
        {
            foreach (var file in Directory.GetFiles(tempOutputPath, $"{jobId}_ffmpeg2pass*"))
            {
                File.Delete(file);
            }
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "Failed to cleanup pass log files for job {JobId}", jobId);
        }
    }
}

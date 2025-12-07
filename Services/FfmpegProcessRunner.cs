using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using System.Linq;
using Microsoft.Extensions.Logging;

namespace liteclip.Services;

public class FfmpegProcessRunner : IFfmpegRunner
{
    private readonly IFfmpegPathResolver _pathResolver;
    private readonly IProgressParser _progressParser;
    private readonly ILogger<FfmpegProcessRunner> _logger;

    public FfmpegProcessRunner(IFfmpegPathResolver pathResolver, IProgressParser progressParser, ILogger<FfmpegProcessRunner> logger)
    {
        _pathResolver = pathResolver;
        _progressParser = progressParser;
        _logger = logger;
    }

    public async Task<FfmpegRunResult> RunAsync(
        string jobId,
        IReadOnlyList<string> arguments,
        double? totalDuration,
        int passNumber,
        int totalPasses,
        Action<FfmpegProgressUpdate>? onProgress,
        Action<Process>? onProcessStarted = null,
        CancellationToken cancellationToken = default)
    {
        if (arguments == null)
        {
            throw new ArgumentNullException(nameof(arguments));
        }

        var ffmpegPath = _pathResolver.ResolveFfmpegPath();
        if (string.IsNullOrWhiteSpace(ffmpegPath))
        {
            throw new InvalidOperationException("FFmpeg executable not found");
        }

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

        var commandLine = FormatFfmpegCommand(ffmpegPath, arguments);
        _logger.LogInformation(
            "Executing FFmpeg command for job {JobId} (pass {Pass}/{TotalPasses}): {Command}",
            jobId,
            passNumber,
            totalPasses,
            commandLine);

        using var process = new Process
        {
            StartInfo = startInfo,
            EnableRaisingEvents = true
        };

        onProcessStarted?.Invoke(process);

        var errorBuilder = new StringBuilder();
        // Cap the stderr buffer to avoid unbounded memory growth for very verbose ffmpeg runs
        const int MaxErrorBufferLength = 64 * 1024; // 64 KB
        var startTime = DateTime.UtcNow;
        var lastProgressUpdate = startTime;

        process.ErrorDataReceived += (_, e) =>
        {
            if (string.IsNullOrEmpty(e.Data))
            {
                return;
            }

            errorBuilder.AppendLine(e.Data);
            // If the buffer grew too large, trim the start to keep only the most recent content.
            if (errorBuilder.Length > MaxErrorBufferLength)
            {
                var toRemove = errorBuilder.Length - MaxErrorBufferLength;
                // Remove from the start, keeping the most recent logs
                errorBuilder.Remove(0, toRemove);
            }

            var line = e.Data.Trim();
            if (!line.StartsWith("frame=", StringComparison.OrdinalIgnoreCase) &&
                !line.Contains("time=", StringComparison.Ordinal))
            {
                return;
            }

            try
            {
                var parsed = _progressParser.TryParse(line, totalDuration);
                if (!parsed.HasValue || onProgress == null)
                {
                    return;
                }

                var baseProgress = parsed.Value;
                double? percent = baseProgress.Percent;
                int? etaSeconds = null;

                var now = DateTime.UtcNow;
                if (baseProgress.CurrentTimeSeconds.HasValue && totalDuration.HasValue)
                {
                    var currentTimeSeconds = baseProgress.CurrentTimeSeconds.Value;
                    var elapsed = (now - startTime).TotalSeconds;
                    if (elapsed > 0)
                    {
                        var speed = currentTimeSeconds / elapsed;
                        if (speed > 0)
                        {
                            if ((now - lastProgressUpdate).TotalSeconds >= 2)
                            {
                                var remainingThisPass = (totalDuration.Value - currentTimeSeconds) / speed;
                                var remainingPasses = (totalPasses - passNumber) * (totalDuration.Value / speed);
                                etaSeconds = (int)Math.Ceiling(remainingThisPass + remainingPasses);
                                lastProgressUpdate = now;
                            }
                        }
                    }
                }

                if (percent.HasValue)
                {
                    var adjusted = ((passNumber - 1) * 100.0 / totalPasses) + (percent.Value / totalPasses);
                    percent = Math.Clamp(adjusted, 0.0, 100.0);
                }

                var update = new FfmpegProgressUpdate(percent, baseProgress.CurrentTimeSeconds, etaSeconds);
                onProgress(update);
            }
            catch
            {
            }
        };

        process.OutputDataReceived += (_, _) => { };

        try
        {
            process.Start();

            // Emit an initial progress update as soon as the FFmpeg process starts,
            // so the job visibly transitions from "starting" to "running" even
            // before the first time-based progress line is parsed.
            if (onProgress != null)
            {
                try
                {
                    onProgress(new FfmpegProgressUpdate(0, 0, null));
                }
                catch
                {
                    // Ignore progress callback errors
                }
            }

            process.BeginOutputReadLine();
            process.BeginErrorReadLine();

            await process.WaitForExitAsync(cancellationToken).ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
            try
            {
                if (!process.HasExited)
                {
                    process.Kill(entireProcessTree: true);
                }
            }
            catch
            {
            }

            return new FfmpegRunResult
            {
                ExitCode = -1,
                StandardError = errorBuilder.ToString()
            };
        }

        return new FfmpegRunResult
        {
            ExitCode = process.ExitCode,
            StandardError = errorBuilder.ToString()
        };
    }

    private static string FormatFfmpegCommand(string executablePath, IEnumerable<string> arguments)
    {
        var quotedExe = executablePath.Contains(' ') ? $"\"{executablePath}\"" : executablePath;
        var args = new List<string>();

        foreach (var arg in arguments)
        {
            args.Add(arg.Contains(' ') ? $"\"{arg}\"" : arg);
        }

        return string.Join(" ", new[] { quotedExe }.Concat(args));
    }
}

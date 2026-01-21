using System;
using System.Diagnostics;
using System.Globalization;
using System.Text.RegularExpressions;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.Extensions.Logging;
using liteclip.Models;

namespace liteclip.Services;

/// <summary>
/// Provides automated black bar detection using FFmpeg's cropdetect filter.
/// </summary>
public sealed class CropDetectionService
{
    private readonly IFfmpegPathResolver _pathResolver;
    private readonly ILogger<CropDetectionService> _logger;
    private static readonly TimeSpan DetectionTimeout = TimeSpan.FromSeconds(10);

    public CropDetectionService(IFfmpegPathResolver pathResolver, ILogger<CropDetectionService> logger)
    {
        _pathResolver = pathResolver;
        _logger = logger;
    }

    /// <summary>
    /// Detects the non-black area of a video file.
    /// </summary>
    /// <param name="filePath">Path to the video file</param>
    /// <param name="startTime">Where in the video to start detection (seconds)</param>
    /// <param name="cancellationToken">Cancellation token</param>
    public async Task<CropDetectionResult?> DetectAsync(string filePath, double startTime = 0, CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(filePath) || !System.IO.File.Exists(filePath))
        {
            return null;
        }

        var ffmpegPath = _pathResolver.ResolveFfmpegPath();
        if (string.IsNullOrWhiteSpace(ffmpegPath))
        {
            _logger.LogWarning("ffmpeg not found, cannot detect crop");
            return null;
        }

        // We run cropdetect for 1 second at the specified start time.
        // limit=24:16:0 -> 24 is the black threshold, 16 is the round factor, 0 is the reset limit
        var arguments = $"-ss {startTime.ToString(CultureInfo.InvariantCulture)} -i \"{filePath}\" -t 1 -vf cropdetect=24:16:0 -f null -";

        var (exitCode, _, stderr) = await RunFfmpegAsync(ffmpegPath, arguments, cancellationToken);

        if (exitCode != 0 || string.IsNullOrWhiteSpace(stderr))
        {
            return null;
        }

        // The output looks like: [Parsed_cropdetect_0 @ 00000...] x1:0 x2:1919 y1:140 y2:939 w:1920 h:800 x:0 y:140 pts:0 t:0.000000 crop=1920:800:0:140
        // We look for the last occurrence of crop=W:H:X:Y as it's the most stable estimate after 1 second of frames.
        var matches = Regex.Matches(stderr, @"crop=(\d+):(\d+):(\d+):(\d+)", RegexOptions.RightToLeft);
        
        if (matches.Count > 0)
        {
            var match = matches[0];
            if (int.TryParse(match.Groups[1].Value, out var w) &&
                int.TryParse(match.Groups[2].Value, out var h) &&
                int.TryParse(match.Groups[3].Value, out var x) &&
                int.TryParse(match.Groups[4].Value, out var y))
            {
                _logger.LogInformation("Auto-detected crop for {Path}: {W}x{H} at {X},{Y}", filePath, w, h, x, y);
                return new CropDetectionResult { Width = w, Height = h, X = x, Y = y };
            }
        }

        return null;
    }

    private async Task<(int ExitCode, string Stdout, string Stderr)> RunFfmpegAsync(
        string ffmpegPath, string arguments, CancellationToken ct)
    {
        var psi = new ProcessStartInfo
        {
            FileName = ffmpegPath,
            Arguments = arguments,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true
        };

        using var process = new Process { StartInfo = psi };
        using var cts = CancellationTokenSource.CreateLinkedTokenSource(ct);
        cts.CancelAfter(DetectionTimeout);

        try
        {
            process.Start();

            var stdoutTask = process.StandardOutput.ReadToEndAsync(cts.Token);
            var stderrTask = process.StandardError.ReadToEndAsync(cts.Token);

            await process.WaitForExitAsync(cts.Token);

            var stdout = await stdoutTask;
            var stderr = await stderrTask;

            return (process.ExitCode, stdout, stderr);
        }
        catch (OperationCanceledException)
        {
            try { process.Kill(entireProcessTree: true); } catch { }
            return (-1, string.Empty, "Timeout");
        }
        catch (Exception ex)
        {
            _logger.LogDebug(ex, "Error running ffmpeg for crop detection");
            return (-1, string.Empty, ex.Message);
        }
    }
}

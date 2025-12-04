using System;
using System.Diagnostics;
using System.Globalization;
using System.Text.Json;
using System.Text.RegularExpressions;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.Extensions.Logging;

namespace liteclip.Services;

/// <summary>
/// Video metadata extracted via ffprobe.
/// </summary>
public sealed class VideoMetadataResult
{
    public int Width { get; init; }
    public int Height { get; init; }
    public double Duration { get; init; }
    public double AspectRatio => Height > 0 ? (double)Width / Height : 0;
    public string? Codec { get; init; }
    public double? FrameRate { get; init; }
    public long? Bitrate { get; init; }
    public string? PixelFormat { get; init; }
    public bool HasAudio { get; init; }
    public string? AudioCodec { get; init; }
    public int? AudioChannels { get; init; }
    public int? AudioSampleRate { get; init; }
}

/// <summary>
/// Robust video metadata extraction using ffprobe with multiple fallback strategies.
/// </summary>
public sealed class VideoMetadataService
{
    private readonly IFfmpegPathResolver _pathResolver;
    private readonly ILogger<VideoMetadataService> _logger;
    private static readonly TimeSpan ProbeTimeout = TimeSpan.FromSeconds(30);

    public VideoMetadataService(IFfmpegPathResolver pathResolver, ILogger<VideoMetadataService> logger)
    {
        _pathResolver = pathResolver;
        _logger = logger;
    }

    /// <summary>
    /// Probes video metadata using ffprobe with multiple fallback strategies for reliability.
    /// </summary>
    public async Task<VideoMetadataResult?> ProbeAsync(string filePath, CancellationToken cancellationToken = default)
    {
        return await ProbeAsync(filePath, fullMetadata: true, cancellationToken);
    }
    
    /// <summary>
    /// Probes video metadata using ffprobe with multiple fallback strategies for reliability.
    /// </summary>
    /// <param name="filePath">Path to the video file</param>
    /// <param name="fullMetadata">If false, only probes essential fields (width, height, duration) for faster results</param>
    /// <param name="cancellationToken">Cancellation token</param>
    public async Task<VideoMetadataResult?> ProbeAsync(string filePath, bool fullMetadata, CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(filePath) || !System.IO.File.Exists(filePath))
        {
            _logger.LogWarning("Cannot probe metadata: file does not exist at {Path}", filePath);
            return null;
        }

        var ffprobePath = GetFfprobePath();
        if (string.IsNullOrWhiteSpace(ffprobePath))
        {
            _logger.LogWarning("ffprobe not found, cannot probe metadata");
            return null;
        }

        // Strategy 1: JSON output with targeted fields (most reliable, structured data)
        var result = await ProbeWithJsonAsync(ffprobePath, filePath, cancellationToken);
        if (result != null)
        {
            _logger.LogDebug("Probed metadata via JSON for {Path}: {W}x{H}, {D}s", filePath, result.Width, result.Height, result.Duration);
            return result;
        }

        // Strategy 2: CSV output - fast path for essential fields only
        result = await ProbeWithCsvFallbackAsync(ffprobePath, filePath, cancellationToken);
        if (result != null)
        {
            _logger.LogDebug("Probed metadata via CSV fallback for {Path}: {W}x{H}, {D}s", filePath, result.Width, result.Height, result.Duration);
            return result;
        }

        // Strategy 3: Parse raw ffprobe output (last resort)
        result = await ProbeWithRawOutputAsync(ffprobePath, filePath, cancellationToken);
        if (result != null)
        {
            _logger.LogDebug("Probed metadata via raw output for {Path}: {W}x{H}, {D}s", filePath, result.Width, result.Height, result.Duration);
            return result;
        }

        _logger.LogWarning("All probe strategies failed for {Path}", filePath);
        return null;
    }

    private async Task<VideoMetadataResult?> ProbeWithJsonAsync(string ffprobePath, string filePath, CancellationToken ct)
    {
        try
        {
            // Use JSON output with targeted show_entries to minimize parsing overhead
            // Only request the specific fields we need instead of full stream/format dumps
            var args = $"-v quiet -print_format json " +
                       $"-show_entries format=duration,bit_rate " +
                       $"-show_entries stream=codec_type,codec_name,width,height,r_frame_rate,avg_frame_rate,duration,pix_fmt,channels,sample_rate " +
                       $"\"{filePath}\"";
            var (exitCode, stdout, _) = await RunFfprobeAsync(ffprobePath, args, ct);

            if (exitCode != 0 || string.IsNullOrWhiteSpace(stdout))
                return null;

            using var doc = JsonDocument.Parse(stdout);
            var root = doc.RootElement;

            int width = 0, height = 0;
            double duration = 0;
            string? codec = null, pixelFormat = null, audioCodec = null;
            double? frameRate = null;
            long? bitrate = null;
            bool hasAudio = false;
            int? audioChannels = null, audioSampleRate = null;

            // Parse format-level duration (most reliable)
            if (root.TryGetProperty("format", out var format))
            {
                if (format.TryGetProperty("duration", out var durProp))
                {
                    var durStr = durProp.GetString();
                    if (!string.IsNullOrEmpty(durStr) && double.TryParse(durStr, NumberStyles.Float, CultureInfo.InvariantCulture, out var d))
                        duration = d;
                }
                if (format.TryGetProperty("bit_rate", out var brProp))
                {
                    var brStr = brProp.GetString();
                    if (!string.IsNullOrEmpty(brStr) && long.TryParse(brStr, out var br))
                        bitrate = br;
                }
            }

            // Parse streams
            if (root.TryGetProperty("streams", out var streams) && streams.ValueKind == JsonValueKind.Array)
            {
                foreach (var stream in streams.EnumerateArray())
                {
                    var codecType = stream.TryGetProperty("codec_type", out var ctProp) ? ctProp.GetString() : null;

                    if (codecType == "video" && width == 0)
                    {
                        if (stream.TryGetProperty("width", out var wProp)) width = wProp.GetInt32();
                        if (stream.TryGetProperty("height", out var hProp)) height = hProp.GetInt32();
                        if (stream.TryGetProperty("codec_name", out var cnProp)) codec = cnProp.GetString();
                        if (stream.TryGetProperty("pix_fmt", out var pfProp)) pixelFormat = pfProp.GetString();

                        // Try to get frame rate from various fields
                        frameRate = TryParseFrameRate(stream, "r_frame_rate")
                                 ?? TryParseFrameRate(stream, "avg_frame_rate");

                        // Stream-level duration as fallback
                        if (duration <= 0 && stream.TryGetProperty("duration", out var sdProp))
                        {
                            var sdStr = sdProp.GetString();
                            if (!string.IsNullOrEmpty(sdStr) && double.TryParse(sdStr, NumberStyles.Float, CultureInfo.InvariantCulture, out var sd))
                                duration = sd;
                        }
                    }
                    else if (codecType == "audio")
                    {
                        hasAudio = true;
                        if (stream.TryGetProperty("codec_name", out var acProp)) audioCodec = acProp.GetString();
                        if (stream.TryGetProperty("channels", out var chProp)) audioChannels = chProp.GetInt32();
                        if (stream.TryGetProperty("sample_rate", out var srProp))
                        {
                            var srStr = srProp.GetString();
                            if (!string.IsNullOrEmpty(srStr) && int.TryParse(srStr, out var sr))
                                audioSampleRate = sr;
                        }
                    }
                }
            }

            if (width <= 0 || height <= 0 || duration <= 0)
                return null;

            return new VideoMetadataResult
            {
                Width = width,
                Height = height,
                Duration = duration,
                Codec = codec,
                FrameRate = frameRate,
                Bitrate = bitrate,
                PixelFormat = pixelFormat,
                HasAudio = hasAudio,
                AudioCodec = audioCodec,
                AudioChannels = audioChannels,
                AudioSampleRate = audioSampleRate
            };
        }
        catch (Exception ex)
        {
            _logger.LogDebug(ex, "JSON probe failed for {Path}", filePath);
            return null;
        }
    }

    private async Task<VideoMetadataResult?> ProbeWithCsvFallbackAsync(string ffprobePath, string filePath, CancellationToken ct)
    {
        try
        {
            // Probe dimensions
            var dimsArgs = $"-v error -select_streams v:0 -show_entries stream=width,height,codec_name,r_frame_rate,pix_fmt -of csv=s=,:p=0 \"{filePath}\"";
            var (dimsExit, dimsOut, _) = await RunFfprobeAsync(ffprobePath, dimsArgs, ct);

            if (dimsExit != 0 || string.IsNullOrWhiteSpace(dimsOut))
                return null;

            var parts = dimsOut.Trim().Split(',');
            if (parts.Length < 2)
                return null;

            if (!int.TryParse(parts[0], out var width) || !int.TryParse(parts[1], out var height))
                return null;

            if (width <= 0 || height <= 0)
                return null;

            string? codec = parts.Length > 2 ? parts[2] : null;
            double? frameRate = parts.Length > 3 ? ParseFrameRateString(parts[3]) : null;
            string? pixelFormat = parts.Length > 4 ? parts[4] : null;

            // Probe duration separately (format duration is more reliable)
            var durArgs = $"-v error -show_entries format=duration -of default=noprint_wrappers=1:nokey=1 \"{filePath}\"";
            var (durExit, durOut, _) = await RunFfprobeAsync(ffprobePath, durArgs, ct);

            double duration = 0;
            if (durExit == 0 && !string.IsNullOrWhiteSpace(durOut))
            {
                double.TryParse(durOut.Trim(), NumberStyles.Float, CultureInfo.InvariantCulture, out duration);
            }

            // Fallback: try stream duration
            if (duration <= 0)
            {
                var streamDurArgs = $"-v error -select_streams v:0 -show_entries stream=duration -of default=noprint_wrappers=1:nokey=1 \"{filePath}\"";
                var (sdExit, sdOut, _) = await RunFfprobeAsync(ffprobePath, streamDurArgs, ct);
                if (sdExit == 0 && !string.IsNullOrWhiteSpace(sdOut))
                {
                    double.TryParse(sdOut.Trim(), NumberStyles.Float, CultureInfo.InvariantCulture, out duration);
                }
            }

            if (duration <= 0)
                return null;

            // Check for audio
            var audioArgs = $"-v error -select_streams a:0 -show_entries stream=codec_name -of default=noprint_wrappers=1:nokey=1 \"{filePath}\"";
            var (audioExit, audioOut, _) = await RunFfprobeAsync(ffprobePath, audioArgs, ct);
            bool hasAudio = audioExit == 0 && !string.IsNullOrWhiteSpace(audioOut);

            return new VideoMetadataResult
            {
                Width = width,
                Height = height,
                Duration = duration,
                Codec = codec,
                FrameRate = frameRate,
                PixelFormat = pixelFormat,
                HasAudio = hasAudio,
                AudioCodec = hasAudio ? audioOut?.Trim() : null
            };
        }
        catch (Exception ex)
        {
            _logger.LogDebug(ex, "CSV probe failed for {Path}", filePath);
            return null;
        }
    }

    private async Task<VideoMetadataResult?> ProbeWithRawOutputAsync(string ffprobePath, string filePath, CancellationToken ct)
    {
        try
        {
            // Get raw ffprobe output and parse with regex
            var args = $"-v error -show_streams -show_format \"{filePath}\"";
            var (exitCode, stdout, _) = await RunFfprobeAsync(ffprobePath, args, ct);

            if (exitCode != 0 || string.IsNullOrWhiteSpace(stdout))
                return null;

            int width = 0, height = 0;
            double duration = 0;
            string? codec = null;

            // Parse width/height
            var widthMatch = Regex.Match(stdout, @"^width=(\d+)", RegexOptions.Multiline);
            var heightMatch = Regex.Match(stdout, @"^height=(\d+)", RegexOptions.Multiline);
            var durationMatch = Regex.Match(stdout, @"^duration=([\d.]+)", RegexOptions.Multiline);
            var codecMatch = Regex.Match(stdout, @"^codec_name=(\w+)", RegexOptions.Multiline);

            if (widthMatch.Success) int.TryParse(widthMatch.Groups[1].Value, out width);
            if (heightMatch.Success) int.TryParse(heightMatch.Groups[1].Value, out height);
            if (durationMatch.Success) double.TryParse(durationMatch.Groups[1].Value, NumberStyles.Float, CultureInfo.InvariantCulture, out duration);
            if (codecMatch.Success) codec = codecMatch.Groups[1].Value;

            if (width <= 0 || height <= 0 || duration <= 0)
                return null;

            return new VideoMetadataResult
            {
                Width = width,
                Height = height,
                Duration = duration,
                Codec = codec
            };
        }
        catch (Exception ex)
        {
            _logger.LogDebug(ex, "Raw output probe failed for {Path}", filePath);
            return null;
        }
    }

    private async Task<(int ExitCode, string Stdout, string Stderr)> RunFfprobeAsync(
        string ffprobePath, string arguments, CancellationToken ct)
    {
        var psi = new ProcessStartInfo
        {
            FileName = ffprobePath,
            Arguments = arguments,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true
        };

        using var process = new Process { StartInfo = psi };
        using var cts = CancellationTokenSource.CreateLinkedTokenSource(ct);
        cts.CancelAfter(ProbeTimeout);

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
            _logger.LogWarning("ffprobe timed out after {Timeout}s", ProbeTimeout.TotalSeconds);
            return (-1, string.Empty, "Timeout");
        }
        catch (Exception ex)
        {
            _logger.LogDebug(ex, "Error running ffprobe");
            return (-1, string.Empty, ex.Message);
        }
    }

    private string? GetFfprobePath()
    {
        var ffmpegPath = _pathResolver.ResolveFfmpegPath();
        if (string.IsNullOrWhiteSpace(ffmpegPath))
            return null;

        var directory = System.IO.Path.GetDirectoryName(ffmpegPath);
        var extension = System.IO.Path.GetExtension(ffmpegPath);
        var ffprobeName = "ffprobe" + extension;

        if (!string.IsNullOrEmpty(directory))
        {
            var probePath = System.IO.Path.Combine(directory, ffprobeName);
            if (System.IO.File.Exists(probePath))
                return probePath;
        }

        // Fallback: try PATH
        return "ffprobe";
    }

    private static double? TryParseFrameRate(JsonElement stream, string propertyName)
    {
        if (!stream.TryGetProperty(propertyName, out var prop))
            return null;

        var str = prop.GetString();
        return ParseFrameRateString(str);
    }

    private static double? ParseFrameRateString(string? str)
    {
        if (string.IsNullOrWhiteSpace(str))
            return null;

        // Handle fraction format like "30000/1001" or "30/1"
        if (str.Contains('/'))
        {
            var frParts = str.Split('/');
            if (frParts.Length == 2 &&
                double.TryParse(frParts[0], NumberStyles.Float, CultureInfo.InvariantCulture, out var num) &&
                double.TryParse(frParts[1], NumberStyles.Float, CultureInfo.InvariantCulture, out var den) &&
                den > 0)
            {
                return num / den;
            }
        }

        // Handle direct number
        if (double.TryParse(str, NumberStyles.Float, CultureInfo.InvariantCulture, out var directFr))
            return directFr;

        return null;
    }
}

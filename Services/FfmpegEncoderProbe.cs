using System.Collections.Concurrent;
using System.Diagnostics;

namespace liteclip.Services;

/// <summary>
/// Implementation of FFmpeg encoder probing service with caching.
/// </summary>
public sealed class FfmpegEncoderProbe : IFfmpegEncoderProbe
{
    private readonly ConcurrentDictionary<string, bool> _encoderCache = new();
    private readonly ILogger<FfmpegEncoderProbe> _logger;

    public FfmpegEncoderProbe(ILogger<FfmpegEncoderProbe> logger)
    {
        _logger = logger;
    }

    public bool IsEncoderAvailable(string encoderName)
    {
        if (string.IsNullOrWhiteSpace(encoderName))
        {
            return false;
        }

        // Check cache first
        if (_encoderCache.TryGetValue(encoderName, out var cached))
        {
            return cached;
        }

        var available = TestEncoderAvailability(encoderName);
        _encoderCache.TryAdd(encoderName, available);

        _logger.LogDebug("Encoder {EncoderName} availability: {Available}", encoderName, available);
        return available;
    }

    public string GetBestEncoder(string codecKey, string[] preferredEncoders, string fallbackEncoder)
    {
        if (string.IsNullOrWhiteSpace(codecKey) || preferredEncoders == null || preferredEncoders.Length == 0)
        {
            _logger.LogWarning("Invalid parameters for GetBestEncoder: codecKey={CodecKey}, preferredEncoders={PreferredEncoders}", 
                codecKey, preferredEncoders?.Length ?? 0);
            return fallbackEncoder ?? string.Empty;
        }

        foreach (var encoder in preferredEncoders)
        {
            if (IsEncoderAvailable(encoder))
            {
                _logger.LogInformation("Selected encoder {Encoder} for codec {CodecKey}", encoder, codecKey);
                return encoder;
            }
        }

        _logger.LogInformation("No hardware encoders available for {CodecKey}, using fallback {FallbackEncoder}", 
            codecKey, fallbackEncoder);
        return fallbackEncoder;
    }

    public void ClearCache()
    {
        _encoderCache.Clear();
        _logger.LogDebug("Encoder availability cache cleared");
    }

    private bool TestEncoderAvailability(string encoderName)
    {
        try
        {
            // Try two more robust tests before falling back to a minimal one:
            // 1) testsrc with NV12 pix_fmt, reasonable resolution/frame-rate and GOP (-g)
            // 2) fallback to the older minimal color test if the first fails
            var attempts = new[]
            {
                // Use testsrc (video test signal) and set NV12 pixel format which AMF expects
                $"-loglevel error -f lavfi -i testsrc=duration=0.5:size=1280x720:rate=30 -pix_fmt nv12 -c:v {encoderName} -g 60 -b:v 2000k -bf 0 -f null -",
                // Fallback minimal test (keeps previous behavior)
                $"-f lavfi -i color=black:s=64x64:d=0.1 -c:v {encoderName} -f null -"
            };

            foreach (var args in attempts)
            {
                var psi = new ProcessStartInfo
                {
                    FileName = "ffmpeg",
                    Arguments = args,
                    RedirectStandardOutput = true,
                    RedirectStandardError = true,
                    UseShellExecute = false,
                    CreateNoWindow = true
                };

                using var process = Process.Start(psi);
                if (process == null) continue;

                // Read stderr (some encoders print diagnostics there)
                var error = process.StandardError.ReadToEnd();
                process.WaitForExit();

                // If exit code is zero, the encoder initialized successfully
                if (process.ExitCode == 0)
                {
                    return true;
                }

                // If the error clearly indicates the encoder is unavailable, break early
                var errLower = error.ToLowerInvariant();
                if (errLower.Contains("not available") || errLower.Contains("cannot load") || errLower.Contains("no nvenc"))
                {
                    return false;
                }

                // Otherwise try the next attempt (the fallback may succeed for some drivers)
            }

            return false;
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Error testing encoder availability for {EncoderName}", encoderName);
            return false;
        }
    }
}

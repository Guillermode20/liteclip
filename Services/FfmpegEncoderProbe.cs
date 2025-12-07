using System.Collections.Concurrent;
using System.Diagnostics;
using System.Linq;
using System.Threading.Tasks;

namespace liteclip.Services;

/// <summary>
/// Implementation of FFmpeg encoder probing service with caching.
/// </summary>
public class FfmpegEncoderProbe
{
    private readonly ConcurrentDictionary<string, bool> _encoderCache = new();
    private readonly IFfmpegPathResolver _pathResolver;
    private readonly ILogger<FfmpegEncoderProbe> _logger;

    public FfmpegEncoderProbe(IFfmpegPathResolver pathResolver, ILogger<FfmpegEncoderProbe> logger)
    {
        _pathResolver = pathResolver;
        _logger = logger;
    }

    public virtual bool IsEncoderAvailable(string encoderName)
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

    public virtual string GetBestEncoder(string codecKey, string[] preferredEncoders, string fallbackEncoder)
    {
        if (string.IsNullOrWhiteSpace(codecKey) || preferredEncoders == null || preferredEncoders.Length == 0)
        {
            _logger.LogWarning("Invalid parameters for GetBestEncoder: codecKey={CodecKey}, preferredEncoders={PreferredEncoders}",
                codecKey, preferredEncoders?.Length ?? 0);
            return fallbackEncoder ?? string.Empty;
        }

        // Check cache first for all encoders
        foreach (var encoder in preferredEncoders)
        {
            if (_encoderCache.TryGetValue(encoder, out var cached) && cached)
            {
                _logger.LogInformation("Selected cached encoder {Encoder} for codec {CodecKey}", encoder, codecKey);
                return encoder;
            }
        }

        // Parallel probe all preferred encoders that aren't cached yet
        var uncachedEncoders = preferredEncoders.Where(e => !_encoderCache.ContainsKey(e)).ToArray();
        if (uncachedEncoders.Length > 0)
        {
            // Probe in parallel with a degree of parallelism matching encoder count (typically 2-4)
            Parallel.ForEach(uncachedEncoders, new ParallelOptions { MaxDegreeOfParallelism = Math.Min(4, uncachedEncoders.Length) }, encoder =>
            {
                var available = TestEncoderAvailability(encoder);
                _encoderCache.TryAdd(encoder, available);
                _logger.LogDebug("Probed encoder {EncoderName} availability: {Available}", encoder, available);
            });
        }

        // Now check in preference order
        foreach (var encoder in preferredEncoders)
        {
            if (_encoderCache.TryGetValue(encoder, out var available) && available)
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
            var ffmpegPath = _pathResolver.ResolveFfmpegPath();
            if (string.IsNullOrWhiteSpace(ffmpegPath) || !File.Exists(ffmpegPath))
            {
                _logger.LogWarning("FFmpeg path could not be resolved while testing encoder {EncoderName}", encoderName);
                return false;
            }

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
                    FileName = ffmpegPath,
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

                // Use a short timeout to prevent hangs on stalled drivers while keeping
                // the overall encoder probing latency low.
                if (!process.WaitForExit(2000))
                {
                    try { process.Kill(entireProcessTree: true); } catch { }
                    _logger.LogWarning("Encoder test for {EncoderName} timed out", encoderName);
                    continue;
                }

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

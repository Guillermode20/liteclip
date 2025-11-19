using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Linq;
using liteclip.Services;

namespace liteclip.CompressionStrategies;

/// <summary>
/// Base class for compression strategies that use EncodingModeConfigs.
/// Reduces code duplication between H.264 and H.265 strategies.
/// </summary>
public abstract class BaseCompressionStrategy : ICompressionStrategy
{
    protected readonly FfmpegCapabilityProbe? _probe;
    protected string? _detectedEncoder;
    protected bool _encoderDetected = false;

    public abstract string CodecKey { get; }
    public abstract string OutputExtension { get; }
    public abstract string MimeType { get; }
    public string VideoCodec => GetBestEncoder();
    public abstract string AudioCodec { get; }
    public abstract int AudioBitrateKbps { get; }

    protected BaseCompressionStrategy(FfmpegCapabilityProbe? probe)
    {
        _probe = probe;
    }

    /// <summary>
    /// Returns the list of encoders to try in order of preference.
    /// </summary>
    protected abstract string[] GetEncodersToTry();

    /// <summary>
    /// Returns the fallback software encoder name.
    /// </summary>
    protected abstract string GetFallbackEncoder();

    protected virtual string GetBestEncoder()
    {
        if (_encoderDetected)
            return _detectedEncoder ?? GetFallbackEncoder();

        _encoderDetected = true;

        var encodersToTry = GetEncodersToTry();

        foreach (var encoder in encodersToTry)
        {
            // Check probe cache first
            if (_probe != null && _probe.SupportedEncoders.Contains(encoder))
            {
                _detectedEncoder = encoder;
                return encoder;
            }

            // Runtime check fallback
            if (IsEncoderAvailable(encoder))
            {
                _detectedEncoder = encoder;
                return encoder;
            }
        }

        // Fallback to software
        _detectedEncoder = GetFallbackEncoder();
        return _detectedEncoder;
    }

    protected virtual bool IsEncoderAvailable(string encoderName)
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
                if (errLower.Contains("not available") || errLower.Contains("cannot load") || errLower.Contains("no nvenc") )
                {
                    return false;
                }

                // Otherwise try the next attempt (the fallback may succeed for some drivers)
            }

            return false;
        }
        catch
        {
            return false;
        }
    }

    public virtual IEnumerable<string> BuildVideoArgs(double videoBitrateKbps, EncodingMode mode)
    {
        var targetBitrate = Math.Max(100, Math.Round(videoBitrateKbps));
        var encoder = GetBestEncoder();
        
        // Use the centralized config
        var config = EncodingModeConfigs.Get(CodecKey, encoder, mode);

        // Tighten VBV so the encoder cannot waste bits but keep enough
        // burst for hard scenes – this is closer to how social apps tune.
        var maxRateMultiplier = Math.Max(config.MaxRateMultiplier, 1.05);
        var bufferMultiplier = Math.Max(config.BufferMultiplier, 1.6);
        var minRateMultiplier = Math.Min(config.MinRateMultiplier, 0.35);

        var maxRate = Math.Round(targetBitrate * maxRateMultiplier);
        var minRate = Math.Round(targetBitrate * minRateMultiplier);
        var buffer = Math.Round(targetBitrate * bufferMultiplier);
        
        var args = new List<string>
        {
            "-c:v", encoder,
            "-b:v", $"{targetBitrate}k"
        };

        // Apply encoder/mode-specific tuning first
        foreach (var token in config.VideoArgs)
        {
            if (token == "{maxrate}")
            {
                args.Add($"{maxRate}k");
            }
            else if (token == "{minrate}")
            {
                args.Add($"{minRate}k");
            }
            else if (token == "{buffer}")
            {
                args.Add($"{buffer}k");
            }
            else if (token == "{target}")
            {
                args.Add($"{targetBitrate}k");
            }
            else if (token.Contains("{maxrate}", StringComparison.Ordinal) ||
                     token.Contains("{buffer}", StringComparison.Ordinal) ||
                     token.Contains("{minrate}", StringComparison.Ordinal))
            {
                var replaced = token
                    .Replace("{maxrate}", maxRate.ToString(), StringComparison.Ordinal)
                    .Replace("{buffer}", buffer.ToString(), StringComparison.Ordinal)
                    .Replace("{minrate}", minRate.ToString(), StringComparison.Ordinal);
                args.Add(replaced);
            }
            else
            {
                args.Add(token);
            }
        }

        // Finally attach standard bitrate constraints if they weren't already
        // provided via the config's VideoArgs.
        if (!args.Contains("-maxrate"))
        {
            args.AddRange(new[] { "-maxrate", $"{maxRate}k" });
        }
        if (!args.Contains("-bufsize"))
        {
            args.AddRange(new[] { "-bufsize", $"{buffer}k" });
        }

        return args;
    }

    public virtual IEnumerable<string> BuildAudioArgs()
    {
        return new List<string> { "-c:a", AudioCodec, "-b:a", $"{AudioBitrateKbps}k" };
    }

    public virtual IEnumerable<string> BuildContainerArgs()
    {
        return new[] { "-movflags", "+faststart" };
    }

    public virtual IEnumerable<string> GetPassExtras(int passNumber, string passLogFile)
    {
        // Hardware encoders typically do not support standard ffmpeg 2-pass flags.
        // They use internal rate control or specific flags (like -multipass qres for NVENC).
        // If we are using a hardware encoder, we should not return standard pass flags.
        var encoder = GetBestEncoder();
        var isHardware = encoder.Contains("nvenc") || encoder.Contains("qsv") || encoder.Contains("amf") || encoder.Contains("videotoolbox") || encoder.Contains("vaapi");
        
        if (isHardware)
        {
            return Enumerable.Empty<string>();
        }

        return new[] { "-pass", passNumber.ToString(), "-passlogfile", passLogFile, "-f", "mp4" };
    }
}

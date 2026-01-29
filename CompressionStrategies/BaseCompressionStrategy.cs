using System;
using System.Collections.Generic;
using System.Linq;
using liteclip.Services;

namespace liteclip.CompressionStrategies;

/// <summary>
/// Base class for compression strategies that use EncodingModeConfigs.
/// Reduces code duplication between H.264 and H.265 strategies.
/// </summary>
public abstract class BaseCompressionStrategy : ICompressionStrategy
{
    private readonly EncoderSelectionService _encoderSelectionService;
    private string? _cachedEncoder;

    protected BaseCompressionStrategy(EncoderSelectionService encoderSelectionService)
    {
        _encoderSelectionService = encoderSelectionService ?? throw new ArgumentNullException(nameof(encoderSelectionService));
    }

    public abstract string CodecKey { get; }
    public abstract string OutputExtension { get; }
    public abstract string MimeType { get; }
    public string VideoCodec => GetBestEncoder();
    public abstract string AudioCodec { get; }
    public abstract int AudioBitrateKbps { get; }

    protected virtual string GetBestEncoder()
    {
        if (_cachedEncoder == null)
        {
            _cachedEncoder = _encoderSelectionService.GetBestEncoder(CodecKey);
        }
        return _cachedEncoder;
    }

    public virtual IEnumerable<string> BuildVideoArgs(double videoBitrateKbps, EncodingMode mode)
    {
        var targetBitrate = Math.Max(100, Math.Round(videoBitrateKbps));
        var encoder = GetBestEncoder();

        // Ultra mode ALWAYS uses software encoding (libx265 for H.265, libx264 for H.264)
        if (mode == EncodingMode.Ultra)
        {
            encoder = CodecKey.Equals("h265", StringComparison.OrdinalIgnoreCase) ? "libx265" : "libx264";
        }

        // Use the centralized config
        var config = EncodingModeConfigs.Get(CodecKey, encoder, mode);

        // Use values from config directly to allow strict adherence to targets
        var maxRateMultiplier = config.MaxRateMultiplier;
        var bufferMultiplier = config.BufferMultiplier;
        var minRateMultiplier = config.MinRateMultiplier;

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
        var isHardware = EncoderSelectionService.IsHardwareEncoder(encoder);

        if (isHardware)
        {
            return Enumerable.Empty<string>();
        }

        return new[] { "-pass", passNumber.ToString(), "-passlogfile", passLogFile, "-f", "mp4" };
    }
}

using System;
using System.Collections.Generic;
using System.Linq;

namespace liteclip.CompressionStrategies;

/// <summary>
/// Logical encoding modes exposed to the UI.
/// These are derived from the quality flag:
/// - Fast:   qualityMode = false
/// - Quality: qualityMode = true
/// </summary>
public enum EncodingMode
{
    Fast,
    Quality
}

/// <summary>
/// Describes how a given (codec, encoder, mode) should be tuned.
/// This centralizes the mode-specific decisions so H.264/H.265 (and others)
/// can be compared side-by-side without digging through strategy code.
/// 
/// The <see cref="VideoArgs"/> collection contains encoder- and mode-specific
/// ffmpeg switches that are independent of the numeric bitrate budget
/// (GOP, presets, psychovisual tuning, etc). Bitrate-specific switches
/// (-b:v/-maxrate/-minrate/-bufsize) are applied separately using the
/// configured multipliers.
/// 
/// ALL MODES NOW STRICTLY ADHERE TO TARGET VALUES (multipliers set to 1.0).
/// </summary>
public sealed record EncodingModeConfig(
    string CodecKey,
    string EncoderKey,
    EncodingMode Mode,
    string DisplayName,
    string Description,
    double MaxRateMultiplier,
    double MinRateMultiplier,
    double BufferMultiplier,
    IReadOnlyList<string> VideoArgs)
{
    /// <summary>
    /// Whether this configuration applies to any encoder for the codec.
    /// </summary>
    public bool IsWildcardEncoder => EncoderKey == "*";
}

/// <summary>
/// Central table for encoding presets by codec / encoder / mode.
/// 
/// The goal is to mirror the existing behavior of the concrete strategies
/// while making all per-mode choices visible in one place for comparison.
/// 
/// ALL MULTIPLIERS SET TO 1.0 TO ENSURE STRICT TARGET ADHERENCE.
/// </summary>
public static class EncodingModeConfigs
{
    private static readonly IReadOnlyList<EncodingModeConfig> _configs = new[]
    {
        // --- H.264 / libx264 (software) ---
        new EncodingModeConfig(
            CodecKey: "h264",
            EncoderKey: "libx264",
            Mode: EncodingMode.Fast,
            DisplayName: "H.264 Fast",
            Description: "Speed-focused H.264 with fast preset and lighter psy optimizations.",
            MaxRateMultiplier: 1.0,
            MinRateMultiplier: 1.0,
            BufferMultiplier: 1.0,
            VideoArgs: new[]
            {
                "-preset", "fast",
                "-pix_fmt", "yuv420p",
                "-g", "60",
                "-sc_threshold", "0",
                "-bf", "2",
                "-refs", "2",
                "-x264-params",
                "aq-mode=2:aq-strength=0.8:rc_lookahead=30:psy=0:me=hex:subme=6"
            }
        ),
        new EncodingModeConfig(
            CodecKey: "h264",
            EncoderKey: "libx264",
            Mode: EncodingMode.Quality,
            DisplayName: "H.264 Quality",
            Description: "Balanced H.264 quality using medium preset and stronger AQ.",
            MaxRateMultiplier: 1.0,
            MinRateMultiplier: 1.0,
            BufferMultiplier: 1.0,
            VideoArgs: new[]
            {
                "-preset", "medium",
                "-pix_fmt", "yuv420p",
                "-g", "60",
                "-sc_threshold", "0",
                "-bf", "3",
                "-refs", "4",
                "-x264-params",
                "aq-mode=3:aq-strength=1.0:rc_lookahead=50:psy=1:psy-rd=1.0:me=umh:subme=8:ref=4:mbtree=1"
            }
        ),

        // --- H.264 / NVENC ---
        new EncodingModeConfig(
            CodecKey: "h264",
            EncoderKey: "h264_nvenc",
            Mode: EncodingMode.Fast,
            DisplayName: "H.264 NVENC Fast",
            Description: "NVENC H.264 tuned for speed with moderate quality.",
            MaxRateMultiplier: 1.0,
            MinRateMultiplier: 1.0,
            BufferMultiplier: 1.0,
            VideoArgs: new[]
            {
                "-preset", "p4",
                "-rc", "vbr",
                "-spatial-aq", "1",
                "-temporal-aq", "1",
                "-rc-lookahead", "32",
                "-g", "60",
                "-bf", "3"
            }
        ),
        new EncodingModeConfig(
            CodecKey: "h264",
            EncoderKey: "h264_nvenc",
            Mode: EncodingMode.Quality,
            DisplayName: "H.264 NVENC Quality",
            Description: "NVENC H.264 with maximum quality preset and lookahead.",
            MaxRateMultiplier: 1.0,
            MinRateMultiplier: 1.0,
            BufferMultiplier: 1.0,
            VideoArgs: new[]
            {
                "-preset", "p7",
                "-rc", "vbr",
                "-spatial-aq", "1",
                "-temporal-aq", "1",
                "-rc-lookahead", "32",
                "-g", "60",
                "-bf", "4",
                "-b_ref_mode", "middle",
                "-multipass", "qres"
            }
        ),

        // --- H.264 / QuickSync ---
        new EncodingModeConfig(
            CodecKey: "h264",
            EncoderKey: "h264_qsv",
            Mode: EncodingMode.Fast,
            DisplayName: "H.264 QSV Fast",
            Description: "QuickSync H.264 tuned for speed.",
            MaxRateMultiplier: 1.0,
            MinRateMultiplier: 1.0,
            BufferMultiplier: 1.0,
            VideoArgs: new[]
            {
                "-preset", "medium",
                "-look_ahead", "1",
                "-look_ahead_depth", "40",
                "-g", "60",
                "-bf", "3"
            }
        ),
        new EncodingModeConfig(
            CodecKey: "h264",
            EncoderKey: "h264_qsv",
            Mode: EncodingMode.Quality,
            DisplayName: "H.264 QSV Quality",
            Description: "QuickSync H.264 with deeper lookahead and adaptive quantization.",
            MaxRateMultiplier: 1.0,
            MinRateMultiplier: 1.0,
            BufferMultiplier: 1.0,
            VideoArgs: new[]
            {
                "-preset", "veryslow",
                "-look_ahead", "1",
                "-look_ahead_depth", "100",
                "-adaptive_i", "1",
                "-adaptive_b", "1",
                "-g", "60",
                "-bf", "4"
            }
        ),

        // --- H.264 / AMF ---
        new EncodingModeConfig(
            CodecKey: "h264",
            EncoderKey: "h264_amf",
            Mode: EncodingMode.Fast,
            DisplayName: "H.264 AMF Fast",
            Description: "AMD AMF H.264 tuned for speed with simpler rate control.",
            MaxRateMultiplier: 1.0,
            MinRateMultiplier: 1.0,
            BufferMultiplier: 1.0,
            VideoArgs: new[]
            {
                "-quality", "quality",
                "-rc", "cbr",
                "-qmin", "0",
                "-qmax", "51",
                "-pix_fmt", "nv12",
                "-g", "60",
                "-bf", "1",
                "-rc-lookahead", "32",
                "-temporal-aq", "1",
                "-spatial-aq", "0"
            }
        ),
        new EncodingModeConfig(
            CodecKey: "h264",
            EncoderKey: "h264_amf",
            Mode: EncodingMode.Quality,
            DisplayName: "H.264 AMF Quality",
            Description: "AMD AMF H.264 with stronger AQ and VBR peak for better quality.",
            MaxRateMultiplier: 1.0,
            MinRateMultiplier: 1.0,
            BufferMultiplier: 1.0,
            VideoArgs: new[]
            {
                "-quality", "quality",
                "-rc", "vbr_peak",
                "-qmin", "18",
                "-qmax", "45",
                "-pix_fmt", "nv12",
                "-g", "60",
                "-bf", "3",
                "-rc-lookahead", "64",
                "-temporal-aq", "2",
                "-spatial-aq", "2",
                "-high_motion_quality_boost_enable", "1"
            }
        ),

        // --- H.264 / VideoToolbox (MacOS) ---
        new EncodingModeConfig(
            CodecKey: "h264",
            EncoderKey: "h264_videotoolbox",
            Mode: EncodingMode.Fast,
            DisplayName: "H.264 VideoToolbox Fast",
            Description: "Apple Silicon H.264 tuned for speed.",
            MaxRateMultiplier: 1.0,
            MinRateMultiplier: 1.0,
            BufferMultiplier: 1.0,
            VideoArgs: new[]
            {
                "-pix_fmt", "yuv420p",
                "-g", "60",
                "-bf", "2"
            }
        ),
        new EncodingModeConfig(
            CodecKey: "h264",
            EncoderKey: "h264_videotoolbox",
            Mode: EncodingMode.Quality,
            DisplayName: "H.264 VideoToolbox Quality",
            Description: "Apple Silicon H.264 tuned for quality.",
            MaxRateMultiplier: 1.0,
            MinRateMultiplier: 1.0,
            BufferMultiplier: 1.0,
            VideoArgs: new[]
            {
                "-pix_fmt", "yuv420p",
                "-g", "60",
                "-bf", "3",
                "-profile:v", "high"
            }
        ),

        // --- H.264 / Wildcard ---
        new EncodingModeConfig(
            CodecKey: "h264",
            EncoderKey: "*",
            Mode: EncodingMode.Fast,
            DisplayName: "H.264 Generic Fast",
            Description: "Generic H.264 settings for unknown encoders.",
            MaxRateMultiplier: 1.0,
            MinRateMultiplier: 1.0,
            BufferMultiplier: 1.0,
            VideoArgs: new[]
            {
                "-pix_fmt", "yuv420p",
                "-g", "60"
            }
        ),
        new EncodingModeConfig(
            CodecKey: "h264",
            EncoderKey: "*",
            Mode: EncodingMode.Quality,
            DisplayName: "H.264 Generic Quality",
            Description: "Generic H.264 settings for unknown encoders.",
            MaxRateMultiplier: 1.0,
            MinRateMultiplier: 1.0,
            BufferMultiplier: 1.0,
            VideoArgs: new[]
            {
                "-pix_fmt", "yuv420p",
                "-g", "60"
            }
        ),

        // --- H.265 / libx265 (software) ---
        new EncodingModeConfig(
            CodecKey: "h265",
            EncoderKey: "libx265",
            Mode: EncodingMode.Fast,
            DisplayName: "H.265 Fast",
            Description: "Software H.265 with slower preset but smaller lookahead for speed.",
            MaxRateMultiplier: 1.0,
            MinRateMultiplier: 1.0,
            BufferMultiplier: 1.0,
            VideoArgs: new[]
            {
                "-preset", "slower",
                "-pix_fmt", "yuv420p",
                "-tag:v", "hvc1",
                "-g", "60",
                "-sc_threshold", "0",
                "-bf", "4",
                "-refs", "5",
                "-x265-params",
                "vbv-bufsize={buffer}:vbv-maxrate={maxrate}:aq-mode=3:aq-strength=1.0:psy-rd=2.0:rc-lookahead=60"
            }
        ),
        new EncodingModeConfig(
            CodecKey: "h265",
            EncoderKey: "libx265",
            Mode: EncodingMode.Quality,
            DisplayName: "H.265 Quality",
            Description: "Software H.265 tuned for strong perceptual quality.",
            MaxRateMultiplier: 1.0,
            MinRateMultiplier: 1.0,
            BufferMultiplier: 1.0,
            VideoArgs: new[]
            {
                "-preset", "slower",
                "-pix_fmt", "yuv420p",
                "-tag:v", "hvc1",
                "-g", "60",
                "-sc_threshold", "0",
                "-bf", "4",
                "-refs", "5",
                "-x265-params",
                "vbv-bufsize={buffer}:vbv-maxrate={maxrate}:aq-mode=3:aq-strength=1.4:psy-rd=2.5:psy-rdoq=1.5:rc-lookahead=80:me=star:subme=10:rd=6:ref=6:sao=1:deblock=-1,-1:rdoq-level=2:ctu=32:tu-intra-depth=3:tu-inter-depth=3"
            }
        ),

        // --- H.265 / NVENC ---
        new EncodingModeConfig(
            CodecKey: "h265",
            EncoderKey: "hevc_nvenc",
            Mode: EncodingMode.Fast,
            DisplayName: "H.265 NVENC Fast",
            Description: "NVENC HEVC tuned for reasonable speed and quality.",
            MaxRateMultiplier: 1.0,
            MinRateMultiplier: 1.0,
            BufferMultiplier: 1.0,
            VideoArgs: new[]
            {
                "-pix_fmt", "yuv420p",
                "-preset", "p6",
                "-rc", "vbr",
                "-spatial-aq", "1",
                "-temporal-aq", "1",
                "-rc-lookahead", "48",
                "-g", "60",
                "-bf", "4",
                "-b_ref_mode", "middle",
                "-multipass", "disabled",
                "-tag:v", "hvc1"
            }
        ),
        new EncodingModeConfig(
            CodecKey: "h265",
            EncoderKey: "hevc_nvenc",
            Mode: EncodingMode.Quality,
            DisplayName: "H.265 NVENC Quality",
            Description: "NVENC HEVC quality mode; same base tuning, two-pass handled separately.",
            MaxRateMultiplier: 1.0,
            MinRateMultiplier: 1.0,
            BufferMultiplier: 1.0,
            VideoArgs: new[]
            {
                "-pix_fmt", "yuv420p",
                "-preset", "p7",
                "-rc", "vbr",
                "-spatial-aq", "1",
                "-temporal-aq", "1",
                "-rc-lookahead", "48",
                "-g", "60",
                "-bf", "4",
                "-b_ref_mode", "middle",
                "-multipass", "disabled",
                "-tier", "high",
                "-tag:v", "hvc1"
            }
        ),

        // --- H.265 / QuickSync ---
        new EncodingModeConfig(
            CodecKey: "h265",
            EncoderKey: "hevc_qsv",
            Mode: EncodingMode.Fast,
            DisplayName: "H.265 QSV Fast",
            Description: "QuickSync HEVC tuned for speed.",
            MaxRateMultiplier: 1.0,
            MinRateMultiplier: 1.0,
            BufferMultiplier: 1.0,
            VideoArgs: new[]
            {
                "-preset", "slower",
                "-look_ahead", "1",
                "-look_ahead_depth", "60",
                "-g", "60",
                "-bf", "4",
                "-tag:v", "hvc1"
            }
        ),
        new EncodingModeConfig(
            CodecKey: "h265",
            EncoderKey: "hevc_qsv",
            Mode: EncodingMode.Quality,
            DisplayName: "H.265 QSV Quality",
            Description: "QuickSync HEVC quality; same as Fast for now.",
            MaxRateMultiplier: 1.0,
            MinRateMultiplier: 1.0,
            BufferMultiplier: 1.0,
            VideoArgs: new[]
            {
                "-preset", "veryslow",
                "-look_ahead", "1",
                "-look_ahead_depth", "100",
                "-adaptive_i", "1",
                "-adaptive_b", "1",
                "-g", "60",
                "-bf", "4",
                "-tag:v", "hvc1"
            }
        ),

        // --- H.265 / AMF ---
        new EncodingModeConfig(
            CodecKey: "h265",
            EncoderKey: "hevc_amf",
            Mode: EncodingMode.Fast,
            DisplayName: "H.265 AMF Fast",
            Description: "AMD AMF HEVC baseline quality; simpler tuning for speed.",
            MaxRateMultiplier: 1.0,
            MinRateMultiplier: 1.0,
            BufferMultiplier: 1.0,
            VideoArgs: new[]
            {
                "-quality", "quality",
                "-rc", "vbr_peak",
                "-qmin", "15",
                "-qmax", "45",
                "-tag:v", "hvc1",
                "-g", "60",
                "-bf", "1",
                "-rc-lookahead", "32",
                "-temporal-aq", "1",
                "-spatial-aq", "0",
                "-profile:v", "main"
            }
        ),
        new EncodingModeConfig(
            CodecKey: "h265",
            EncoderKey: "hevc_amf",
            Mode: EncodingMode.Quality,
            DisplayName: "H.265 AMF Quality",
            Description: "AMD AMF HEVC quality mode with strong AQ and lookahead.",
            MaxRateMultiplier: 1.0,
            MinRateMultiplier: 1.0,
            BufferMultiplier: 1.0,
            VideoArgs: new[]
            {
                "-quality", "quality",
                "-rc", "vbr_peak",
                "-qmin", "15",
                "-qmax", "45",
                "-tag:v", "hvc1",
                "-g", "120",
                "-bf", "4",
                "-rc-lookahead", "80",
                "-temporal-aq", "2",
                "-spatial-aq", "2",
                "-profile:v", "main",
                "-no-scenecut", "1"
            }
        ),

        // --- H.265 / VideoToolbox (MacOS) ---
        new EncodingModeConfig(
            CodecKey: "h265",
            EncoderKey: "hevc_videotoolbox",
            Mode: EncodingMode.Fast,
            DisplayName: "H.265 VideoToolbox Fast",
            Description: "Apple Silicon H.265 tuned for speed.",
            MaxRateMultiplier: 1.0,
            MinRateMultiplier: 1.0,
            BufferMultiplier: 1.0,
            VideoArgs: new[]
            {
                "-pix_fmt", "yuv420p",
                "-tag:v", "hvc1",
                "-g", "60"
            }
        ),
        new EncodingModeConfig(
            CodecKey: "h265",
            EncoderKey: "hevc_videotoolbox",
            Mode: EncodingMode.Quality,
            DisplayName: "H.265 VideoToolbox Quality",
            Description: "Apple Silicon H.265 tuned for quality.",
            MaxRateMultiplier: 1.0,
            MinRateMultiplier: 1.0,
            BufferMultiplier: 1.0,
            VideoArgs: new[]
            {
                "-pix_fmt", "yuv420p",
                "-tag:v", "hvc1",
                "-g", "60",
                "-preset", "quality"
            }
        ),

        // --- H.265 / Wildcard ---
        new EncodingModeConfig(
            CodecKey: "h265",
            EncoderKey: "*",
            Mode: EncodingMode.Fast,
            DisplayName: "H.265 Generic Fast",
            Description: "Generic H.265 settings for unknown encoders.",
            MaxRateMultiplier: 1.0,
            MinRateMultiplier: 1.0,
            BufferMultiplier: 1.0,
            VideoArgs: new[]
            {
                "-pix_fmt", "yuv420p",
                "-tag:v", "hvc1",
                "-g", "60"
            }
        ),
        new EncodingModeConfig(
            CodecKey: "h265",
            EncoderKey: "*",
            Mode: EncodingMode.Quality,
            DisplayName: "H.265 Generic Quality",
            Description: "Generic H.265 settings for unknown encoders.",
            MaxRateMultiplier: 1.0,
            MinRateMultiplier: 1.0,
            BufferMultiplier: 1.0,
            VideoArgs: new[]
            {
                "-pix_fmt", "yuv420p",
                "-tag:v", "hvc1",
                "-g", "60"
            }
        )
    };

    /// <summary>
    /// Returns the most specific configuration for the given codec/encoder/mode,
    /// falling back to wildcard encoder entries when necessary.
    /// </summary>
    public static EncodingModeConfig Get(string codecKey, string encoderName, EncodingMode mode)
    {
        if (codecKey == null) throw new ArgumentNullException(nameof(codecKey));
        if (encoderName == null) throw new ArgumentNullException(nameof(encoderName));

        var normalizedCodec = codecKey.ToLowerInvariant();
        var normalizedEncoder = encoderName.ToLowerInvariant();

        // Exact match by codec + encoder + mode
        var exact = _configs.FirstOrDefault(c =>
            string.Equals(c.CodecKey, normalizedCodec, StringComparison.OrdinalIgnoreCase) &&
            string.Equals(c.EncoderKey, normalizedEncoder, StringComparison.OrdinalIgnoreCase) &&
            c.Mode == mode);

        if (exact != null)
        {
            return exact;
        }

        // Fallback: wildcard encoder for codec + mode
        var wildcard = _configs.FirstOrDefault(c =>
            string.Equals(c.CodecKey, normalizedCodec, StringComparison.OrdinalIgnoreCase) &&
            c.EncoderKey == "*" &&
            c.Mode == mode);

        if (wildcard != null)
        {
            return wildcard;
        }

        throw new InvalidOperationException(
            $"No EncodingModeConfig found for codec '{codecKey}', encoder '{encoderName}', mode '{mode}'.");
    }

    /// <summary>
    /// Exposes all configs for inspection or diagnostic logging.
    /// </summary>
    public static IReadOnlyList<EncodingModeConfig> GetAll() => _configs;
}
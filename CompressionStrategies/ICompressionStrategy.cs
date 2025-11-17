using System.Collections.Generic;

namespace liteclip.CompressionStrategies;

/// <summary>
/// Strategy interface for codec-specific compression argument generation and metadata.
/// 
/// Quality mode defaults by codec:
/// - H.264: Prioritizes speed; default is lean/fast AMF settings.
/// - H.265: Prioritizes quality+speed mix; default is quality-focused with higher lookahead/AQ.
/// 
/// When useQualityMode=true, each codec respects its philosophy while adapting:
/// - H.264 can optionally enhance AQ further but remains speed-oriented.
/// - H.265 uses the quality settings by default; mode=false would scale back for speed.
/// </summary>
public interface ICompressionStrategy
{
    // Metadata
    string CodecKey { get; }
    string OutputExtension { get; }
    string MimeType { get; }
    string VideoCodec { get; }
    string AudioCodec { get; }
    int AudioBitrateKbps { get; }

    // Argument builders
    IEnumerable<string> BuildVideoArgs(double videoBitrateKbps, bool useQualityMode, bool useUltraMode = false);
    IEnumerable<string> BuildAudioArgs();
    IEnumerable<string> BuildContainerArgs();
    /// <summary>
    /// Returns extra ffmpeg arguments required for the given pass number when performing two-pass encoding.
    /// For example: "-pass 1 -passlogfile <log> -f mp4" or similar. Return empty collection when no extras are required.
    /// </summary>
    IEnumerable<string> GetPassExtras(int passNumber, string passLogFile);
}

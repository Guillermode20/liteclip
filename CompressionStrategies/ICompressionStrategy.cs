using System.Collections.Generic;

namespace liteclip.CompressionStrategies;

/// <summary>
/// Strategy interface for codec-specific compression argument generation and metadata.
/// 
/// The <see cref="EncodingMode"/> value is the single source of truth for
/// how the exposed encoding modes (for example, \"fast\" and \"quality\") are interpreted per codec.
/// Concrete strategies should use the central <see cref="EncodingModeConfigs"/>
/// table so all mode-specific behavior can be inspected in one place.
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
    IEnumerable<string> BuildVideoArgs(double videoBitrateKbps, EncodingMode mode);
    IEnumerable<string> BuildAudioArgs();
    IEnumerable<string> BuildContainerArgs();
    /// <summary>
    /// Returns extra ffmpeg arguments required for the given pass number when performing two-pass encoding.
    /// For example: "-pass 1 -passlogfile &lt;log&gt; -f mp4" or similar. Return empty collection when no extras are required.
    /// </summary>
    IEnumerable<string> GetPassExtras(int passNumber, string passLogFile);
}

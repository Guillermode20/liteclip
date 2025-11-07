using System.Collections.Generic;

namespace smart_compressor.CompressionStrategies;

/// <summary>
/// Strategy interface for codec-specific compression argument generation.
/// Implementations produce ffmpeg arguments and metadata for a codec.
/// </summary>
public interface ICompressionStrategy
{
    /// <summary>Short codec key (eg. "h264").</summary>
    string CodecKey { get; }

    /// <summary>File extension to use for output (eg. ".mp4").</summary>
    string OutputExtension { get; }

    /// <summary>Mime type to present for downloads.</summary>
    string MimeType { get; }

    /// <summary>
    /// Build ffmpeg arguments for the provided settings.
    /// Returns an ordered list of arguments (without the executable path).
    /// </summary>
    IEnumerable<string> BuildArguments(object settings);
}

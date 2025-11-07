namespace smart_compressor.Services;

/// <summary>
/// Abstraction for resolving a path to an ffmpeg executable.
/// Implementations may resolve a bundled resource, a system PATH entry or a configured location.
/// </summary>
public interface IFfmpegPathResolver
{
    /// <summary>
    /// Returns a full path to an ffmpeg executable, or null if none found.
    /// </summary>
    string? ResolveFfmpegPath();
}

namespace liteclip.Services;

/// <summary>
/// Abstraction for resolving paths to ffmpeg and ffprobe executables.
/// Implementations may resolve a bundled resource, a system PATH entry or a configured location.
/// </summary>
public interface IFfmpegPathResolver
{
    /// <summary>
    /// Returns a full path to an ffmpeg executable, or null if none found.
    /// </summary>
    string? ResolveFfmpegPath();

    /// <summary>
    /// Returns a full path to an ffprobe executable, or null if none found.
    /// </summary>
    string? ResolveFfprobePath();
}

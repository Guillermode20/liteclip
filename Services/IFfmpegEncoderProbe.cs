namespace liteclip.Services;

/// <summary>
/// Service for probing FFmpeg encoder availability with caching.
/// Centralizes hardware encoder detection logic and caches results for process lifetime.
/// </summary>
public interface IFfmpegEncoderProbe
{
    /// <summary>
    /// Checks if an encoder is available in the current FFmpeg installation.
    /// Results are cached for the process lifetime.
    /// </summary>
    /// <param name="encoderName">The encoder name to check (e.g., "h264_nvenc")</param>
    /// <returns>True if the encoder is available, false otherwise</returns>
    bool IsEncoderAvailable(string encoderName);

    /// <summary>
    /// Gets the best available encoder for a specific codec based on preference order.
    /// </summary>
    /// <param name="codecKey">The codec key ("h264" or "h265")</param>
    /// <param name="preferredEncoders">Array of encoder names in order of preference</param>
    /// <param name="fallbackEncoder">Software encoder to use if no hardware encoders are available</param>
    /// <returns>The best available encoder name</returns>
    string GetBestEncoder(string codecKey, string[] preferredEncoders, string fallbackEncoder);

    /// <summary>
    /// Clears the encoder availability cache (useful for testing or when FFmpeg path changes).
    /// </summary>
    void ClearCache();
}

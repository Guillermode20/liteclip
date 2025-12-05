namespace liteclip.Services;

/// <summary>
/// Service for selecting the best encoder based on codec and hardware availability.
/// Encapsulates the policy: NVENC → QSV → VideoToolbox → AMF → VAAPI → software.
/// </summary>
public interface IEncoderSelectionService
{
    /// <summary>
    /// Gets the best available encoder for H.264 encoding.
    /// </summary>
    /// <returns>The encoder name (e.g., "h264_nvenc", "libx264")</returns>
    string GetBestH264Encoder();

    /// <summary>
    /// Gets the best available encoder for H.265 encoding.
    /// </summary>
    /// <returns>The encoder name (e.g., "hevc_nvenc", "libx265")</returns>
    string GetBestH265Encoder();

    /// <summary>
    /// Gets the best available encoder for a specific codec key.
    /// </summary>
    /// <param name="codecKey">The codec key ("h264" or "h265")</param>
    /// <returns>The encoder name</returns>
    string GetBestEncoder(string codecKey);

    /// <summary>
    /// Checks if the given encoder is a hardware encoder.
    /// </summary>
    /// <param name="encoderName">The encoder name to check</param>
    /// <returns>True if it's a hardware encoder, false for software encoders</returns>
    bool IsHardwareEncoder(string encoderName);

    /// <summary>
    /// Gets cached encoder info (name + hardware flag) for a specific codec key.
    /// This avoids repeated probing per job.
    /// </summary>
    /// <param name="codecKey">The codec key ("h264" or "h265")</param>
    /// <returns>Cached encoder information including hardware flag</returns>
    CachedEncoderInfo GetCachedEncoderInfo(string codecKey);
}

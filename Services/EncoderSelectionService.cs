namespace liteclip.Services;

/// <summary>
/// Cached encoder information including hardware flag.
/// </summary>
public sealed record CachedEncoderInfo(string EncoderName, bool IsHardware);

/// <summary>
/// Implementation of encoder selection service with hardware encoder preference policy.
/// Policy order: NVENC → QSV → VideoToolbox → AMF → VAAPI → software.
/// Caches encoder selection at startup to avoid repeated probing per job.
/// </summary>
public class EncoderSelectionService
{
    private readonly FfmpegEncoderProbe _encoderProbe;
    private readonly ILogger<EncoderSelectionService> _logger;

    // Cached encoder info per codec - populated lazily on first access
    // Use separate locks to avoid contention between H264 and H265 lookups
    private readonly object _h264CacheLock = new();
    private readonly object _h265CacheLock = new();
    private CachedEncoderInfo? _cachedH264Encoder;
    private CachedEncoderInfo? _cachedH265Encoder;

    // Hardware encoder preference order by codec (Windows-only: NVENC, QSV, AMF)
    private static readonly string[] H264_ENCODER_PREFERENCE = new[]
    {
        "h264_nvenc",       // NVIDIA
        "h264_qsv",         // Intel QuickSync
        "h264_amf"          // AMD
    };

    private static readonly string[] H265_ENCODER_PREFERENCE = new[]
    {
        "hevc_nvenc",       // NVIDIA (Best balance of speed/quality)
        "hevc_qsv",         // Intel QuickSync (Excellent speed)
        "hevc_amf"          // AMD (Fast, requires careful tuning)
    };

    private static readonly string H264_FALLBACK = "libx264";
    private static readonly string H265_FALLBACK = "libx265";

    public EncoderSelectionService(FfmpegEncoderProbe encoderProbe, ILogger<EncoderSelectionService> logger)
    {
        _encoderProbe = encoderProbe;
        _logger = logger;
    }

    /// <summary>
    /// Gets cached encoder info for H.264, probing only once.
    /// </summary>
    public CachedEncoderInfo GetCachedH264EncoderInfo()
    {
        if (_cachedH264Encoder != null)
            return _cachedH264Encoder;

        lock (_h264CacheLock)
        {
            if (_cachedH264Encoder != null)
                return _cachedH264Encoder;

            var encoder = _encoderProbe.GetBestEncoder("h264", H264_ENCODER_PREFERENCE, H264_FALLBACK);
            var isHardware = IsHardwareEncoder(encoder);
            _cachedH264Encoder = new CachedEncoderInfo(encoder, isHardware);
            _logger.LogInformation("Cached H.264 encoder: {Encoder} (hardware: {IsHardware})", encoder, isHardware);
            return _cachedH264Encoder;
        }
    }

    /// <summary>
    /// Gets cached encoder info for H.265, probing only once.
    /// </summary>
    public CachedEncoderInfo GetCachedH265EncoderInfo()
    {
        if (_cachedH265Encoder != null)
            return _cachedH265Encoder;

        lock (_h265CacheLock)
        {
            if (_cachedH265Encoder != null)
                return _cachedH265Encoder;

            var encoder = _encoderProbe.GetBestEncoder("h265", H265_ENCODER_PREFERENCE, H265_FALLBACK);
            var isHardware = IsHardwareEncoder(encoder);
            _cachedH265Encoder = new CachedEncoderInfo(encoder, isHardware);
            _logger.LogInformation("Cached H.265 encoder: {Encoder} (hardware: {IsHardware})", encoder, isHardware);
            return _cachedH265Encoder;
        }
    }

    /// <summary>
    /// Gets cached encoder info for a specific codec key.
    /// </summary>
    public CachedEncoderInfo GetCachedEncoderInfo(string codecKey)
    {
        return codecKey.ToLowerInvariant() switch
        {
            "h264" => GetCachedH264EncoderInfo(),
            "h265" or "hevc" => GetCachedH265EncoderInfo(),
            _ => throw new ArgumentException($"Unsupported codec key: {codecKey}", nameof(codecKey))
        };
    }

    public string GetBestEncoder(string codecKey) => GetCachedEncoderInfo(codecKey).EncoderName;

    public static bool IsHardwareEncoder(string encoderName)
    {
        if (string.IsNullOrWhiteSpace(encoderName))
            return false;

        var encoderLower = encoderName.ToLowerInvariant();
        return encoderLower.Contains("nvenc") ||
               encoderLower.Contains("qsv") ||
               encoderLower.Contains("amf");
    }
}

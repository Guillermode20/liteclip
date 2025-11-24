namespace liteclip.Services;

/// <summary>
/// Implementation of encoder selection service with hardware encoder preference policy.
/// Policy order: NVENC → QSV → VideoToolbox → AMF → VAAPI → software
/// </summary>
public sealed class EncoderSelectionService : IEncoderSelectionService
{
    private readonly IFfmpegEncoderProbe _encoderProbe;
    private readonly ILogger<EncoderSelectionService> _logger;

    // Hardware encoder preference order by codec
    private static readonly string[] H264_ENCODER_PREFERENCE = new[]
    {
        "h264_nvenc",       // NVIDIA
        "h264_qsv",         // Intel QuickSync
        "h264_videotoolbox",// MacOS Apple Silicon
        "h264_amf",         // AMD
        "h264_vaapi"        // Linux Generic
    };

    private static readonly string[] H265_ENCODER_PREFERENCE = new[]
    {
        "hevc_nvenc",       // NVIDIA (Best balance of speed/quality)
        "hevc_qsv",         // Intel QuickSync (Excellent speed)
        "hevc_videotoolbox",// MacOS Apple Silicon (Fast)
        "hevc_amf",         // AMD (Fast, requires careful tuning)
        "hevc_vaapi"        // Linux Generic
    };

    private static readonly string H264_FALLBACK = "libx264";
    private static readonly string H265_FALLBACK = "libx265";

    public EncoderSelectionService(IFfmpegEncoderProbe encoderProbe, ILogger<EncoderSelectionService> logger)
    {
        _encoderProbe = encoderProbe;
        _logger = logger;
    }

    public string GetBestH264Encoder()
    {
        return GetBestEncoder("h264");
    }

    public string GetBestH265Encoder()
    {
        return GetBestEncoder("h265");
    }

    public string GetBestEncoder(string codecKey)
    {
        return codecKey.ToLowerInvariant() switch
        {
            "h264" => _encoderProbe.GetBestEncoder("h264", H264_ENCODER_PREFERENCE, H264_FALLBACK),
            "h265" or "hevc" => _encoderProbe.GetBestEncoder("h265", H265_ENCODER_PREFERENCE, H265_FALLBACK),
            _ => throw new ArgumentException($"Unsupported codec key: {codecKey}", nameof(codecKey))
        };
    }

    public bool IsHardwareEncoder(string encoderName)
    {
        if (string.IsNullOrWhiteSpace(encoderName))
            return false;

        var encoderLower = encoderName.ToLowerInvariant();
        return encoderLower.Contains("nvenc") || 
               encoderLower.Contains("qsv") || 
               encoderLower.Contains("amf") || 
               encoderLower.Contains("videotoolbox") || 
               encoderLower.Contains("vaapi");
    }
}

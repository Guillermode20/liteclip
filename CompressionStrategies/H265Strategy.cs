using System;
using System.Collections.Generic;
using liteclip.Services;

namespace liteclip.CompressionStrategies;

public class H265Strategy : BaseCompressionStrategy
{
    public override string CodecKey => "h265";
    public override string OutputExtension => ".mp4";
    public override string MimeType => "video/mp4";
    public override string AudioCodec => "aac";
    public override int AudioBitrateKbps => 128;

    public H265Strategy() : base()
    {
    }

    protected override string[] GetEncodersToTry()
    {
        return new[] 
        { 
            "hevc_nvenc",       // NVIDIA (Best balance of speed/quality)
            "hevc_qsv",         // Intel QuickSync (Excellent speed)
            "hevc_videotoolbox",// MacOS Apple Silicon (Fast)
            "hevc_amf",         // AMD (Fast, requires careful tuning)
            "hevc_vaapi"        // Linux Generic
        };
    }

    protected override string GetFallbackEncoder()
    {
        return "libx265";
    }
}
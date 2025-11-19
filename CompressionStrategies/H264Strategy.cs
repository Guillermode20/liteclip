using System;
using System.Collections.Generic;
using liteclip.Services;

namespace liteclip.CompressionStrategies;

public class H264Strategy : BaseCompressionStrategy
{
    public override string CodecKey => "h264";
    public override string OutputExtension => ".mp4";
    public override string MimeType => "video/mp4";
    public override string AudioCodec => "aac";
    public override int AudioBitrateKbps => 128;

    public H264Strategy(FfmpegCapabilityProbe? probe = null) : base(probe)
    {
    }

    protected override string[] GetEncodersToTry()
    {
        return new[] { "h264_nvenc", "h264_qsv", "h264_amf" };
    }

    protected override string GetFallbackEncoder()
    {
        return "libx264";
    }
}

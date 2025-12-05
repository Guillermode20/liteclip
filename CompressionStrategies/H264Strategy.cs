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

    public H264Strategy(EncoderSelectionService encoderSelectionService) : base(encoderSelectionService)
    {
    }
}

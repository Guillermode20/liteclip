using liteclip.CompressionStrategies;

namespace liteclip.Models;

public class VideoSegment
{
    public double Start { get; set; }
    public double End { get; set; }
}

public class CompressionRequest
{
    public string Codec { get; set; } = "h264";
    public int? ScalePercent { get; set; }
    public int? TargetFps { get; set; }
    public double? TargetSizeMb { get; set; }
    public bool SkipCompression { get; set; }
    public bool MuteAudio { get; set; }
    public double? SourceDuration { get; set; }
    public List<VideoSegment>? Segments { get; set; }
    public bool UseQualityMode { get; set; }
    public string? QualityMode { get; set; }

    public int? CropX { get; set; }
    public int? CropY { get; set; }
    public int? CropWidth { get; set; }
    public int? CropHeight { get; set; }

    /// <summary>
    /// Unified logical encoding mode derived from the quality flag.
    /// This is not supplied directly by the client; it is normalized on the server.
    /// </summary>
    public EncodingMode Mode { get; set; } = EncodingMode.Fast;
}


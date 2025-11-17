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
    public double? SourceDuration { get; set; }
    public List<VideoSegment>? Segments { get; set; }
        public bool UseQualityMode { get; set; }
        public bool UseUltraMode { get; set; }
}


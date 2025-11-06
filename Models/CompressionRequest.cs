namespace smart_compressor.Models;

public class CompressionRequest
{
    public string Mode { get; set; } = "advanced";
    public string Codec { get; set; } = "h264";
    public int? Crf { get; set; }
    public int? ScalePercent { get; set; }
    public double? TargetSizeMb { get; set; }
    public double? SourceDuration { get; set; }
    public int? SourceWidth { get; set; }
    public int? SourceHeight { get; set; }
    public long? OriginalSizeBytes { get; set; }
    public double? TargetBitrateKbps { get; set; }
    public double? VideoBitrateKbps { get; set; }
    public bool TwoPass { get; set; } = false;
}


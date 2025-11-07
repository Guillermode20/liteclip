namespace smart_compressor.Models;

public class CompressionRequest
{
    public string Codec { get; set; } = "h264";
    public int? ScalePercent { get; set; }
    public double? TargetSizeMb { get; set; }
    public double? SourceDuration { get; set; }
}


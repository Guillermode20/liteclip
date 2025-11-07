namespace smart_compressor.Models;

public class CompressionResult
{
    public string JobId { get; set; } = string.Empty;
    public string OriginalFilename { get; set; } = string.Empty;
    public string Status { get; set; } = string.Empty;
    public string? Message { get; set; }
    public string Codec { get; set; } = "h264";
    public int? Crf { get; set; }
    public int? ScalePercent { get; set; }
    public double? TargetSizeMb { get; set; }
    public double? TargetBitrateKbps { get; set; }
    public string? OutputFilename { get; set; }
    public string? OutputMimeType { get; set; }
    public double Progress { get; set; } = 0;
    public int? EstimatedSecondsRemaining { get; set; }
    public int? QueuePosition { get; set; }
}



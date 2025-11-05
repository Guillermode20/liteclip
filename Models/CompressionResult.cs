namespace smart_compressor.Models;

public class CompressionResult
{
    public string JobId { get; set; } = string.Empty;
    public string OriginalFilename { get; set; } = string.Empty;
    public string Status { get; set; } = string.Empty;
    public string? Message { get; set; }
}



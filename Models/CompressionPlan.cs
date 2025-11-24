using System.Collections.Generic;

namespace liteclip.Models;

public sealed class CompressionPlan
{
    public string JobId { get; init; } = string.Empty;
    public CompressionRequest Request { get; init; } = new();
    public double? TotalKbps { get; init; }
    public double? VideoKbps { get; init; }
}

namespace liteclip.Models;

/// <summary>
/// Results of automated crop detection.
/// </summary>
public sealed class CropDetectionResult
{
    public int X { get; init; }
    public int Y { get; init; }
    public int Width { get; init; }
    public int Height { get; init; }
}

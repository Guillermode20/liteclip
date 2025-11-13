namespace liteclip.Models;

/// <summary>
/// Represents an in-memory compression job and its runtime metadata.
/// </summary>
public class CompressionJob
{
    /// <summary>Unique identifier for the job.</summary>
    public string JobId { get; set; } = string.Empty;

    /// <summary>Original uploaded filename.</summary>
    public string OriginalFilename { get; set; } = string.Empty;

    /// <summary>Selected codec for compression (eg. "h264").</summary>
    public string Codec { get; set; } = "h264";

    /// <summary>Percent scale to apply to the source (eg. 50 for 50%).</summary>
    public int? ScalePercent { get; set; }

    /// <summary>Desired target size in megabytes.</summary>
    public double? TargetSizeMb { get; set; }

    /// <summary>Estimated or actual target bitrate in Kbps (may be null).</summary>
    public double? TargetBitrateKbps { get; set; }

    /// <summary>Path on disk to the compressed output file.</summary>
    public string? OutputPath { get; set; }

    /// <summary>Filename to present to the user for download.</summary>
    public string? OutputFilename { get; set; }

    /// <summary>Mime type of the compressed output.</summary>
    public string? OutputMimeType { get; set; }

    /// <summary>Progress percentage (0.0 - 100.0).</summary>
    public double Progress { get; set; } = 0.0;

    /// <summary>Estimated remaining seconds for the job, if available.</summary>
    public int? EstimatedSecondsRemaining { get; set; }

    /// <summary>Current queue position when queued (1-based).</summary>
    public int? QueuePosition { get; set; }

    /// <summary>Human-readable error message if job failed.</summary>
    public string? ErrorMessage { get; set; }

    /// <summary>Current job status.</summary>
    public JobStatus Status { get; set; } = JobStatus.Queued;

    /// <summary>Optional source duration in seconds (when provided).</summary>
    public double? SourceDuration { get; set; }
}

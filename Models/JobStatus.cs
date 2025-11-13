namespace liteclip.Models;

/// <summary>
/// Represents the lifecycle status of a compression job.
/// </summary>
public enum JobStatus
{
    Queued,
    Processing,
    Completed,
    Failed,
    Cancelled
}

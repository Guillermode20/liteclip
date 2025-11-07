using Microsoft.AspNetCore.Http;
using smart_compressor.Models;
using System.Collections.Generic;
using System.Threading.Tasks;

namespace smart_compressor.Services;

/// <summary>
/// Public contract for the video compression service.
/// Implementations handle job creation, cancellation and querying.
/// </summary>
public interface IVideoCompressionService
{
    /// <summary>
    /// Starts a compression job from an uploaded file and returns a job id.
    /// </summary>
    /// <param name="file">Uploaded file stream.</param>
    /// <param name="request">Compression parameters.</param>
    /// <returns>Job identifier string.</returns>
    Task<string> CompressVideoAsync(IFormFile file, CompressionRequest request);

    /// <summary>
    /// Attempts to cancel a running or queued job.
    /// </summary>
    /// <param name="jobId">Job identifier.</param>
    /// <returns>True if cancelled, false otherwise.</returns>
    bool CancelJob(string jobId);

    /// <summary>
    /// Returns the job metadata for the supplied job id, or null if not found.
    /// </summary>
    /// <summary>
    /// Returns the internal job metadata for the supplied job id, or null if not found.
    /// </summary>
    JobMetadata? GetJob(string jobId);

    /// <summary>
    /// Returns a snapshot of all tracked internal jobs.
    /// </summary>
    IEnumerable<JobMetadata> GetAllJobs();

    /// <summary>
    /// Returns the 1-based queue position for a queued job or 0 when not queued.
    /// </summary>
    int GetQueuePosition(string jobId);

    /// <summary>
    /// Cleanup a specific job's files and remove it from in-memory tracking.
    /// </summary>
    void CleanupJob(string jobId);
}

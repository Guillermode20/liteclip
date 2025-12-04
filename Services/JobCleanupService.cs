using liteclip.Services;

namespace liteclip.Services;

public class JobCleanupService : BackgroundService
{
    private readonly ILogger<JobCleanupService> _logger;
    private readonly VideoCompressionService _compressionService;
    private readonly IConfiguration _configuration;
    private readonly TimeSpan _cleanupInterval;
    private readonly TimeSpan _jobRetentionTime;

    public JobCleanupService(
        ILogger<JobCleanupService> logger,
        VideoCompressionService compressionService,
        IConfiguration configuration)
    {
        _logger = logger;
        _compressionService = compressionService;
        _configuration = configuration;
        
        if (!int.TryParse(configuration["Compression:CleanupIntervalMinutes"], out var cleanupMinutes))
        {
            cleanupMinutes = 5;
        }

        if (!int.TryParse(configuration["Compression:JobRetentionMinutes"], out var retentionMinutes))
        {
            retentionMinutes = 30;
        }

        _cleanupInterval = TimeSpan.FromMinutes(cleanupMinutes);
        _jobRetentionTime = TimeSpan.FromMinutes(retentionMinutes);
    }

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        _logger.LogServiceStartup("Job Cleanup Service", $"Cleanup interval: {_cleanupInterval}, Retention time: {_jobRetentionTime}");

        while (!stoppingToken.IsCancellationRequested)
        {
            try
            {
                await Task.Delay(_cleanupInterval, stoppingToken);
                await CleanupExpiredJobsAsync();
            }
            catch (TaskCanceledException)
            {
                // Service is stopping, this is expected
                break;
            }
            catch (Exception ex)
            {
                _logger.LogError(ex, "Error during job cleanup");
            }
        }

        _logger.LogInformation("Job Cleanup Service stopped");
    }

    private async Task CleanupExpiredJobsAsync()
    {
        try
        {
            var now = DateTime.UtcNow;
            var expiredJobs = _compressionService.GetAllJobs()
                .Where(job => ShouldCleanupJob(job, now))
                .ToList();

            if (expiredJobs.Count > 0)
            {
                _logger.LogInformation("Cleaning up {Count} expired jobs", expiredJobs.Count);

                // Process cleanup in batches to avoid blocking for too long
                foreach (var job in expiredJobs)
                {
                    try
                    {
                        _compressionService.CleanupJob(job.JobId);
                        _logger.LogDebug("Cleaned up job {JobId} (Status: {Status}, Age: {Age:F1}m)", 
                            job.JobId, job.Status, (now - job.CreatedAt).TotalMinutes);
                    }
                    catch (Exception ex)
                    {
                        _logger.LogError(ex, "Failed to cleanup job {JobId}", job.JobId);
                    }
                    
                    // Yield periodically to avoid blocking the thread pool
                    await Task.Yield();
                }
            }
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Error in CleanupExpiredJobsAsync");
        }
    }

    private bool ShouldCleanupJob(JobMetadata job, DateTime now)
    {
        // Clean up completed jobs after retention time
        if (job.Status == "completed" && job.CompletedAt.HasValue)
        {
            return (now - job.CompletedAt.Value) > _jobRetentionTime;
        }

        // Clean up failed jobs after retention time
        if (job.Status == "failed" && job.CompletedAt.HasValue)
        {
            return (now - job.CompletedAt.Value) > _jobRetentionTime;
        }

        // Clean up cancelled jobs after retention time
        if (job.Status == "cancelled")
        {
            var referenceTime = job.CompletedAt ?? job.StartedAt ?? job.CreatedAt;
            return (now - referenceTime) > _jobRetentionTime;
        }

        // Clean up stale jobs that have been processing for too long (4 hours)
        if (job.Status == "processing" && job.StartedAt.HasValue)
        {
            return (now - job.StartedAt.Value) > TimeSpan.FromHours(4);
        }

        // Clean up stale queued jobs (2 hours)
        if (job.Status == "queued")
        {
            return (now - job.CreatedAt) > TimeSpan.FromHours(2);
        }

        return false;
    }
}


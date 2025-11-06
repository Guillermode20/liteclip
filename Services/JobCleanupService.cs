using smart_compressor.Services;

namespace smart_compressor.Services;

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
        
        _cleanupInterval = TimeSpan.FromMinutes(configuration.GetValue<int>("Compression:CleanupIntervalMinutes", 5));
        _jobRetentionTime = TimeSpan.FromMinutes(configuration.GetValue<int>("Compression:JobRetentionMinutes", 30));
    }

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        _logger.LogInformation("Job Cleanup Service started. Cleanup interval: {Interval}, Retention time: {Retention}", 
            _cleanupInterval, _jobRetentionTime);

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

    private Task CleanupExpiredJobsAsync()
    {
        try
        {
            var now = DateTime.UtcNow;
            var expiredJobs = _compressionService.GetAllJobs()
                .Where(job => ShouldCleanupJob(job, now))
                .ToList();

            if (expiredJobs.Any())
            {
                _logger.LogInformation("Cleaning up {Count} expired jobs", expiredJobs.Count);

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
                }
            }
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Error in CleanupExpiredJobsAsync");
        }

        return Task.CompletedTask;
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

public static class VideoCompressionServiceExtensions
{
    public static IEnumerable<JobMetadata> GetAllJobs(this VideoCompressionService service)
    {
        // This is a helper method to access all jobs for cleanup
        // We'll need to add a public method in VideoCompressionService to support this
        return service.GetAllJobsInternal();
    }
}


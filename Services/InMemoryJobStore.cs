using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Linq;

namespace liteclip.Services;

public sealed class InMemoryJobStore : IJobStore
{
    private readonly ConcurrentDictionary<string, JobMetadata> _jobs = new();
    private readonly ConcurrentQueue<string> _queue = new();

    // Cache queue position lookups - invalidated on enqueue
    private readonly object _queueLock = new();
    private List<string>? _queueSnapshot;

    public void AddOrUpdate(JobMetadata job)
    {
        _jobs[job.JobId] = job;
    }

    public bool TryGet(string jobId, out JobMetadata? job)
    {
        var success = _jobs.TryGetValue(jobId, out var value);
        job = value;
        return success;
    }

    public IEnumerable<JobMetadata> GetAll()
    {
        return _jobs.Values.ToList();
    }

    public bool TryRemove(string jobId, out JobMetadata? job)
    {
        var success = _jobs.TryRemove(jobId, out var removed);
        job = removed;
        // Invalidate cache when job is removed
        if (success)
        {
            lock (_queueLock) { _queueSnapshot = null; }
        }
        return success;
    }

    public int GetQueueLength()
    {
        return _queue.Count;
    }

    public void Enqueue(string jobId)
    {
        _queue.Enqueue(jobId);
        // Invalidate cache when new job is enqueued
        lock (_queueLock) { _queueSnapshot = null; }
    }

    public int GetQueuePosition(string jobId)
    {
        if (!_jobs.TryGetValue(jobId, out var job) || job.Status != "queued")
        {
            return 0;
        }

        // Use cached snapshot or create new one
        List<string> snapshot;
        lock (_queueLock)
        {
            snapshot = _queueSnapshot ??= _queue.ToList();
        }

        var position = 1;
        foreach (var queuedJobId in snapshot)
        {
            if (queuedJobId == jobId)
            {
                return position;
            }

            if (_jobs.TryGetValue(queuedJobId, out var queuedJob) && queuedJob.Status == "queued")
            {
                position++;
            }
        }

        return 0;
    }
}

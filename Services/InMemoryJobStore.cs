using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Linq;

namespace liteclip.Services;

public sealed class InMemoryJobStore : IJobStore
{
    private readonly ConcurrentDictionary<string, JobMetadata> _jobs = new();
    private readonly ConcurrentQueue<string> _queue = new();

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
        return success;
    }

    public int GetQueueLength()
    {
        return _queue.Count;
    }

    public void Enqueue(string jobId)
    {
        _queue.Enqueue(jobId);
    }

    public int GetQueuePosition(string jobId)
    {
        if (!_jobs.TryGetValue(jobId, out var job) || job.Status != "queued")
        {
            return 0;
        }

        var position = 1;
        foreach (var queuedJobId in _queue)
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

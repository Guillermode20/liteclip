using System.Collections.Generic;

namespace liteclip.Services;

public interface IJobStore
{
    void AddOrUpdate(JobMetadata job);

    bool TryGet(string jobId, out JobMetadata? job);

    IEnumerable<JobMetadata> GetAll();

    bool TryRemove(string jobId, out JobMetadata? job);

    int GetQueueLength();

    void Enqueue(string jobId);

    int GetQueuePosition(string jobId);
}

using liteclip.Models;

namespace liteclip.Services;

public readonly record struct CodecPlanningContext(string Key, string FileExtension, int AudioBitrateKbps);

public interface ICompressionPlanner
{
    CompressionRequest NormalizeRequest(CompressionRequest request);

    CompressionPlan BuildPlan(
        string jobId,
        CompressionRequest normalizedRequest,
        CodecPlanningContext codecContext,
        int? sourceWidth,
        int? sourceHeight);
}

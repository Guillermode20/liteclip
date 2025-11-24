using System.Collections.Generic;
using System.Threading;
using System.Threading.Tasks;
using liteclip.CompressionStrategies;
using liteclip.Models;

namespace liteclip.Services;

public interface IVideoEncodingPipeline
{
    Task<bool> RunSinglePassEncodingAsync(
        string jobId,
        JobMetadata job,
        IReadOnlyList<string> arguments,
        double? totalDuration,
        CancellationToken cancellationToken = default);

    Task<bool> RunTwoPassEncodingAsync(
        string jobId,
        JobMetadata job,
        IReadOnlyList<string> baseArguments,
        string codecKey,
        string fileExtension,
        double? totalDuration,
        string tempOutputPath,
        ICompressionStrategy? strategy,
        CancellationToken cancellationToken = default);
}

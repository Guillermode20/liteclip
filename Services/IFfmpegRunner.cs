using System.Collections.Generic;
using System.Diagnostics;
using System.Threading;
using System.Threading.Tasks;

namespace liteclip.Services;

public readonly struct FfmpegProgressUpdate
{
    public double? Percent { get; }
    public double? CurrentTimeSeconds { get; }
    public int? EstimatedSecondsRemaining { get; }

    public FfmpegProgressUpdate(double? percent, double? currentTimeSeconds, int? estimatedSecondsRemaining)
    {
        Percent = percent;
        CurrentTimeSeconds = currentTimeSeconds;
        EstimatedSecondsRemaining = estimatedSecondsRemaining;
    }
}

public sealed class FfmpegRunResult
{
    public int ExitCode { get; init; }
    public string StandardError { get; init; } = string.Empty;
}

public interface IFfmpegRunner
{
    Task<FfmpegRunResult> RunAsync(
        string jobId,
        IReadOnlyList<string> arguments,
        double? totalDuration,
        int passNumber,
        int totalPasses,
        System.Action<FfmpegProgressUpdate>? onProgress,
        System.Action<Process>? onProcessStarted = null,
        CancellationToken cancellationToken = default);
}

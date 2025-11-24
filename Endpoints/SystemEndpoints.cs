using Microsoft.AspNetCore.Http;
using Microsoft.AspNetCore.Routing;
using Microsoft.Extensions.Logging;
using liteclip;
using liteclip.Services;

namespace liteclip.Endpoints;

public static class SystemEndpoints
{
    public static IEndpointRouteBuilder MapSystemEndpoints(this IEndpointRouteBuilder endpoints)
    {
        endpoints.MapGet("/api/update", async (UpdateCheckerService updateChecker, ILogger<Program> logger) =>
            {
                try
                {
                    var info = await updateChecker.GetUpdateInfoAsync();
                    return Results.Ok(info);
                }
                catch (Exception ex)
                {
                    logger.LogWarning(ex, "Update check failed");
                    return Results.Problem("Unable to check for updates", statusCode: 503);
                }
            })
            .WithName("GetUpdateInfo");

        endpoints.MapGet("/api/ffmpeg/status", (FfmpegBootstrapper bootstrapper) =>
            {
                var status = bootstrapper.GetStatus();
                return Results.Ok(new
                {
                    state = status.State.ToString().ToLowerInvariant(),
                    status.ProgressPercent,
                    status.DownloadedBytes,
                    status.TotalBytes,
                    status.Message,
                    status.ExecutablePath,
                    status.ErrorMessage,
                    status.Ready
                });
            })
            .WithName("GetFfmpegStatus");

        endpoints.MapPost("/api/ffmpeg/retry", async (FfmpegBootstrapper bootstrapper) =>
            {
                bootstrapper.ResetForRetry();
                await bootstrapper.EnsureReadyAsync();
                var status = bootstrapper.GetStatus();
                return Results.Ok(new
                {
                    state = status.State.ToString().ToLowerInvariant(),
                    status.Message,
                    status.Ready
                });
            })
            .WithName("RetryFfmpegDownload");

        endpoints.MapGet("/api/ffmpeg/encoders", async (HttpRequest request, FfmpegProbeService probe, FfmpegBootstrapper bootstrapper) =>
            {
                // ensure ffmpeg ready
                try
                {
                    await bootstrapper.EnsureReadyAsync();
                }
                catch (Exception ex)
                {
                    return Results.Problem(title: "FFmpeg is still preparing", detail: ex.Message, statusCode: 503);
                }

                var verify = false;
                if (request.Query.TryGetValue("verify", out var v))
                {
                    if (bool.TryParse(v.ToString(), out var b)) verify = b;
                }

                var encoders = await probe.GetEncodersAsync(verify);
                return Results.Ok(encoders);
            })
            .WithName("GetFfmpegEncoders");

        return endpoints;
    }
}

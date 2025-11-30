using System.IO;
using Microsoft.AspNetCore.Http;
using Microsoft.AspNetCore.Mvc;
using Microsoft.AspNetCore.Routing;
using Microsoft.Extensions.Logging;
using liteclip;
using liteclip.Services;

namespace liteclip.Endpoints;

public static class SystemEndpoints
{
    public static IEndpointRouteBuilder MapSystemEndpoints(this IEndpointRouteBuilder endpoints)
    {
        endpoints.MapGet("/api/version", (IAppVersionProvider versionProvider) =>
            {
                var version = versionProvider.GetCurrentVersion();
                return Results.Ok(new { version });
            })
            .WithName("GetAppVersion");

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

        endpoints.MapPost("/api/ffmpeg/start", async (FfmpegBootstrapper bootstrapper) =>
            {
                // Start bootstrap if not already started
                _ = Task.Run(async () =>
                {
                    try
                    {
                        await bootstrapper.EnsureReadyAsync();
                    }
                    catch
                    {
                        // Errors will be reflected in status endpoint
                    }
                });

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
            .WithName("StartFfmpegDownload");

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

        endpoints.MapPost("/api/app/close", () =>
            {
                Task.Run(async () =>
                {
                    // Give time for the response to be sent
                    await Task.Delay(100);
                    Environment.Exit(0);
                });
                return Results.Ok();
            })
            .WithName("CloseApp");

        // Probe video metadata using ffprobe (more reliable than browser)
        endpoints.MapPost("/api/probe-metadata", async (
            IFormFile file,
            VideoMetadataService metadataService,
            FfmpegBootstrapper bootstrapper,
            ILogger<Program> logger) =>
            {
                if (file == null || file.Length == 0)
                {
                    return Results.BadRequest(new { error = "No file uploaded" });
                }

                try
                {
                    await bootstrapper.EnsureReadyAsync();
                }
                catch (Exception ex)
                {
                    return Results.Problem(
                        title: "FFmpeg is still preparing",
                        detail: ex.Message,
                        statusCode: 503
                    );
                }

                // Save to temp file for probing
                var tempDir = Path.Combine(Path.GetTempPath(), "liteclip", "probe");
                Directory.CreateDirectory(tempDir);
                var tempFile = Path.Combine(tempDir, $"{Guid.NewGuid()}{Path.GetExtension(file.FileName)}");

                try
                {
                    await using (var stream = new FileStream(tempFile, FileMode.Create))
                    {
                        await file.CopyToAsync(stream);
                    }

                    var metadata = await metadataService.ProbeAsync(tempFile);

                    if (metadata == null)
                    {
                        return Results.Problem(
                            title: "Failed to extract metadata",
                            detail: "Could not read video metadata. The file may be corrupted or in an unsupported format.",
                            statusCode: 422
                        );
                    }

                    return Results.Ok(new
                    {
                        width = metadata.Width,
                        height = metadata.Height,
                        duration = metadata.Duration,
                        aspectRatio = metadata.AspectRatio,
                        codec = metadata.Codec,
                        frameRate = metadata.FrameRate,
                        bitrate = metadata.Bitrate,
                        pixelFormat = metadata.PixelFormat,
                        hasAudio = metadata.HasAudio,
                        audioCodec = metadata.AudioCodec,
                        audioChannels = metadata.AudioChannels,
                        audioSampleRate = metadata.AudioSampleRate
                    });
                }
                catch (Exception ex)
                {
                    logger.LogError(ex, "Error probing video metadata");
                    return Results.Problem(
                        title: "Error probing metadata",
                        detail: ex.Message,
                        statusCode: 500
                    );
                }
                finally
                {
                    // Cleanup temp file
                    try { if (File.Exists(tempFile)) File.Delete(tempFile); } catch { }
                }
            })
            .DisableAntiforgery()
            .WithName("ProbeVideoMetadata");

        return endpoints;
    }
}

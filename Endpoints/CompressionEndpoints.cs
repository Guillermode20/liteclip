using System;
using System.Collections.Generic;
using System.IO;
using Microsoft.AspNetCore.Http;
using Microsoft.AspNetCore.Mvc;
using Microsoft.AspNetCore.Routing;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.Logging;
using liteclip;
using liteclip.Models;
using liteclip.Services;

namespace liteclip.Endpoints;

public static class CompressionEndpoints
{
    public static IEndpointRouteBuilder MapCompressionEndpoints(this IEndpointRouteBuilder endpoints)
    {
        // POST endpoint to upload and compress video
        endpoints.MapPost("/api/compress", async (
                [FromForm] IFormFile file,
                [FromForm] int? scalePercent,
                [FromForm] string? codec,
                [FromForm] double? targetSizeMb,
                [FromForm] double? sourceDuration,
                [FromForm] string? segments,
                [FromForm] bool? skipCompression,
                [FromForm] bool? qualityMode,
                [FromForm] bool? muteAudio,

                VideoCompressionService compressionService,
                IConfiguration configuration,
                FfmpegBootstrapper ffmpegBootstrapper,
                ILogger<Program> logger) =>
            {
                if (file == null || file.Length == 0)
                {
                    return Results.BadRequest(new { error = "No file uploaded" });
                }

                try
                {
                    await ffmpegBootstrapper.EnsureReadyAsync();
                }
                catch (Exception ex)
                {
                    return Results.Problem(
                        title: "FFmpeg is still preparing",
                        detail: ex.Message,
                        statusCode: 503
                    );
                }

                // Validate file size
                var maxFileSize = 2_147_483_648L;
                if (!string.IsNullOrWhiteSpace(configuration["FileUpload:MaxFileSizeBytes"]) && long.TryParse(configuration["FileUpload:MaxFileSizeBytes"], out var parsedMax))
                {
                    maxFileSize = parsedMax;
                }
                if (file.Length > maxFileSize)
                {
                    var maxSizeMb = maxFileSize / (1024.0 * 1024.0);
                    return Results.BadRequest(new { error = $"File is too large. Maximum allowed size is {maxSizeMb:F0} MB." });
                }

                try
                {
                    // Parse segments if provided
                    List<VideoSegment>? videoSegments = null;
                    if (!string.IsNullOrWhiteSpace(segments))
                    {
                        try
                        {
                            var jsonOptions = new System.Text.Json.JsonSerializerOptions
                            {
                                PropertyNameCaseInsensitive = true,
                                NumberHandling = System.Text.Json.Serialization.JsonNumberHandling.AllowReadingFromString
                            };
                            // Try to deserialize as List<VideoSegment> first
                            videoSegments = System.Text.Json.JsonSerializer.Deserialize<List<VideoSegment>>(segments, jsonOptions);
                        }
                        catch (Exception ex)
                        {
                            return Results.BadRequest(new { error = $"Invalid segments format: {ex.Message}" });
                        }
                    }

                    var compressionRequest = new CompressionRequest
                    {
                        Codec = codec ?? "h264",
                        ScalePercent = scalePercent,
                        TargetSizeMb = targetSizeMb,
                        SkipCompression = skipCompression ?? false,
                        MuteAudio = muteAudio ?? false,
                        SourceDuration = sourceDuration,
                        Segments = videoSegments,
                        UseQualityMode = qualityMode ?? false
                    };

                    var jobId = await compressionService.CompressVideoAsync(file, compressionRequest);
                    var job = compressionService.GetJob(jobId);

                    return Results.Ok(new CompressionResult
                    {
                        JobId = jobId,
                        OriginalFilename = file.FileName,
                        Status = job?.Status ?? "processing",
                        Message = "Video compression started. Use the jobId to download the result.",
                        Codec = job?.Codec ?? compressionRequest.Codec,
                        ScalePercent = job?.ScalePercent,
                        TargetSizeMb = job?.TargetSizeMb,
                        TargetBitrateKbps = job?.TargetBitrateKbps,
                        OutputSizeBytes = job?.OutputSizeBytes,
                        CompressionSkipped = job?.CompressionSkipped ?? false,
                        OutputFilename = job?.OutputFilename,
                        OutputMimeType = job?.OutputMimeType,
                        Progress = job?.Progress ?? 0,
                        EncoderName = job?.EncoderName,
                        EncoderIsHardware = job?.EncoderIsHardware,
                        CreatedAt = job?.CreatedAt,
                        CompletedAt = job?.CompletedAt
                    });
                }
                catch (Exception ex)
                {
                    return Results.Problem(
                        title: "Error processing video",
                        detail: ex.Message,
                        statusCode: 500
                    );
                }
            })
            .WithName("CompressVideo")
            .DisableAntiforgery();

        // GET endpoint to check job status
        endpoints.MapGet("/api/status/{jobId}", (string jobId, VideoCompressionService compressionService, ILogger<Program> logger) =>
            {
                if (string.IsNullOrWhiteSpace(jobId))
                {
                    return Results.BadRequest(new { error = "Job ID is required" });
                }

                var job = compressionService.GetJob(jobId);

                if (job == null)
                {
                    logger.LogWarning("Job not found for status check: {JobId}", jobId);
                    return Results.NotFound(new { error = $"Job not found. The job may have expired or the application was restarted. JobId: {jobId}" });
                }

                var queuePosition = job.Status == "queued" ? compressionService.GetQueuePosition(jobId) : (int?)null;

                return Results.Ok(new CompressionResult
                {
                    JobId = jobId,
                    OriginalFilename = job.OriginalFilename,
                    Status = job.Status,
                    Message = job.Status switch
                    {
                        "queued" => queuePosition.HasValue && queuePosition.Value > 0
                            ? $"Video is queued for compression (position {queuePosition.Value})."
                            : "Video is queued for compression.",
                        "processing" => job.EstimatedSecondsRemaining.HasValue && job.EstimatedSecondsRemaining.Value > 0
                            ? $"Video compression is in progress ({job.Progress:F1}%). Estimated time remaining: {FormatTimeRemaining(job.EstimatedSecondsRemaining.Value)}"
                            : $"Video compression is in progress ({job.Progress:F1}%).",
                        "completed" => "Video compression completed successfully.",
                        "failed" => $"Video compression failed: {job.ErrorMessage}",
                        "cancelled" => "Video compression was cancelled.",
                        _ => "Unknown status"
                    },
                    Codec = job.Codec,
                    ScalePercent = job.ScalePercent,
                    TargetSizeMb = job.TargetSizeMb,
                    TargetBitrateKbps = job.TargetBitrateKbps,
                    OutputSizeBytes = job.OutputSizeBytes,
                    CompressionSkipped = job.CompressionSkipped,
                    OutputFilename = job.OutputFilename,
                    OutputMimeType = job.OutputMimeType,
                    Progress = job.Progress,
                    EstimatedSecondsRemaining = job.EstimatedSecondsRemaining,
                    QueuePosition = queuePosition,
                    EncoderName = job.EncoderName,
                    EncoderIsHardware = job.EncoderIsHardware,
                    CreatedAt = job.CreatedAt,
                    CompletedAt = job.CompletedAt
                });
            })
            .WithName("GetJobStatus");

        // POST endpoint to cancel a job
        endpoints.MapPost("/api/cancel/{jobId}", (string jobId, VideoCompressionService compressionService, ILogger<Program> logger) =>
            {
                logger.LogApiRequest("POST", $"/api/cancel/{jobId}", "Job cancellation");

                if (string.IsNullOrWhiteSpace(jobId))
                {
                    return Results.BadRequest(new { error = "Job ID is required" });
                }

                var success = compressionService.CancelJob(jobId);

                if (!success)
                {
                    var job = compressionService.GetJob(jobId);
                    if (job == null)
                    {
                        return Results.NotFound(new { error = "Job not found" });
                    }

                    if (job.Status == "completed" || job.Status == "failed" || job.Status == "cancelled")
                    {
                        return Results.BadRequest(new { error = $"Cannot cancel job with status: {job.Status}" });
                    }

                    return Results.Problem("Failed to cancel job", statusCode: 500);
                }

                logger.LogJobCompletion(jobId, true, "CANCELLED");
                return Results.Ok(new { message = "Job cancelled successfully", jobId });
            })
            .WithName("CancelJob");

        endpoints.MapPost("/api/retry/{jobId}", (string jobId, VideoCompressionService compressionService, ILogger<Program> logger) =>
            {
                logger.LogApiRequest("POST", $"/api/retry/{jobId}", "Job retry");

                if (string.IsNullOrWhiteSpace(jobId))
                {
                    return Results.BadRequest(new { error = "Job ID is required" });
                }

                var (success, error) = compressionService.RetryJob(jobId);
                if (!success)
                {
                    return Results.BadRequest(new { error = error ?? "Unable to retry job." });
                }

                logger.LogInformation("🔄 Job {JobId} re-queued for processing", jobId);
                return Results.Ok(new { message = "Job re-queued for processing", jobId });
            })
            .WithName("RetryJob");

        // GET endpoint to check status and download compressed video
        endpoints.MapGet("/api/download/{jobId}", async (string jobId, VideoCompressionService compressionService, ILogger<Program> logger) =>
            {
                logger.LogApiRequest("GET", $"/api/download/{jobId}", "File download");

                if (string.IsNullOrWhiteSpace(jobId))
                {
                    logger.LogWarning("Empty jobId provided for download");
                    return Results.BadRequest(new { error = "Job ID is required" });
                }

                var job = compressionService.GetJob(jobId);

                if (job == null)
                {
                    logger.LogWarning("Job not found for download: {JobId}", jobId);
                    return Results.NotFound(new { error = $"Job not found. The job may have expired or the application was restarted. JobId: {jobId}" });
                }

                logger.LogInformation("📁 Job found: {JobId}, Status: {Status}", jobId, job.Status);

                if (job.Status == "processing")
                {
                    return Results.Ok(new CompressionResult
                    {
                        JobId = jobId,
                        OriginalFilename = job.OriginalFilename,
                        Status = "processing",
                        Message = "Video compression is still in progress. Please try again later.",
                        Codec = job.Codec,
                        ScalePercent = job.ScalePercent,
                        TargetSizeMb = job.TargetSizeMb,
                        TargetBitrateKbps = job.TargetBitrateKbps,
                        OutputSizeBytes = job.OutputSizeBytes,
                        CompressionSkipped = job.CompressionSkipped,
                        OutputFilename = job.OutputFilename,
                        OutputMimeType = job.OutputMimeType,
                        Progress = job.Progress,
                        EncoderName = job.EncoderName,
                        EncoderIsHardware = job.EncoderIsHardware,
                        CreatedAt = job.CreatedAt,
                        CompletedAt = job.CompletedAt
                    });
                }

                if (job.Status == "failed")
                {
                    logger.LogError("Job failed: {JobId}, Error: {Error}", jobId, job.ErrorMessage);
                    return Results.Problem(
                        title: "Video compression failed",
                        detail: job.ErrorMessage ?? "Unknown error occurred",
                        statusCode: 500
                    );
                }

                if (job.Status == "completed")
                {
                    if (!File.Exists(job.OutputPath))
                    {
                        logger.LogError("Output file not found for completed job: {JobId}, Path: {Path}", jobId, job.OutputPath);
                        return Results.NotFound(new { error = "Compressed video file not found on disk" });
                    }

                    var fileName = !string.IsNullOrWhiteSpace(job.OutputFilename) ? job.OutputFilename : $"compressed_{job.OriginalFilename}";
                    var fileInfo = new FileInfo(job.OutputPath);
                    logger.LogFileOperation("Streaming", job.OutputPath, fileInfo.Length);

                    // Stream the file instead of loading into memory - prevents OOM on large files
                    var stream = new FileStream(
                        job.OutputPath,
                        FileMode.Open,
                        FileAccess.Read,
                        FileShare.Read,
                        bufferSize: 81920,
                        useAsync: true);

                    var mimeType = !string.IsNullOrWhiteSpace(job.OutputMimeType) ? job.OutputMimeType : "video/mp4";
                    return Results.File(stream, mimeType, fileName, enableRangeProcessing: true);
                }

                logger.LogWarning("Unexpected job status: {JobId}, Status: {Status}", jobId, job.Status);
                return Results.NotFound(new { error = $"Compressed video not found. Job status: {job.Status}" });
            })
            .WithName("DownloadCompressedVideo");

        return endpoints;
    }

    private static string FormatTimeRemaining(int seconds)
    {
        if (seconds < 60)
        {
            return $"{seconds}s";
        }
        else if (seconds < 3600)
        {
            var minutes = seconds / 60;
            var secs = seconds % 60;
            return secs > 0 ? $"{minutes}m {secs}s" : $"{minutes}m";
        }
        else
        {
            var hours = seconds / 3600;
            var minutes = (seconds % 3600) / 60;
            return minutes > 0 ? $"{hours}h {minutes}m" : $"{hours}h";
        }
    }
}

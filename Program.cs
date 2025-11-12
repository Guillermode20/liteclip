using Microsoft.AspNetCore.Http.Features;
using Microsoft.AspNetCore.Mvc;
using Microsoft.AspNetCore.Server.Kestrel.Core;
using smart_compressor.Models;
using smart_compressor.Services;
using smart_compressor.CompressionStrategies;

var builder = WebApplication.CreateBuilder(args);

// Configure to listen on fixed port for Tauri sidecar
builder.WebHost.UseUrls("http://localhost:5333");

// Configure Kestrel to accept large files (up to 2 GB)
builder.Services.Configure<KestrelServerOptions>(options =>
{
    options.Limits.MaxRequestBodySize = 2_147_483_648; // 2 GB
});

// Configure form options for large file uploads
builder.Services.Configure<FormOptions>(options =>
{
    options.MultipartBodyLengthLimit = 2_147_483_648; // 2 GB
    options.ValueLengthLimit = int.MaxValue;
    options.MultipartHeadersLengthLimit = int.MaxValue;
});

// Add services to the container.
// OpenAPI removed for faster startup (not needed for desktop app)
// Register concrete implementations and expose their interfaces for DI compatibility.
builder.Services.AddSingleton<FfmpegPathResolver>();
builder.Services.AddSingleton<IFfmpegPathResolver>(sp => sp.GetRequiredService<FfmpegPathResolver>());

builder.Services.AddSingleton<VideoCompressionService>();
builder.Services.AddSingleton<IVideoCompressionService>(sp => sp.GetRequiredService<VideoCompressionService>());

// Compression strategies and factory
builder.Services.AddSingleton<ICompressionStrategy, H264Strategy>();
builder.Services.AddSingleton<ICompressionStrategy, H265Strategy>();
builder.Services.AddSingleton<ICompressionStrategy, Vp9Strategy>();
builder.Services.AddSingleton<ICompressionStrategy, Av1Strategy>();
builder.Services.AddSingleton<ICompressionStrategyFactory, CompressionStrategyFactory>();

builder.Services.AddHostedService<JobCleanupService>();

var app = builder.Build();

// Configure the HTTP request pipeline.
// Static files now served by Tauri frontend
// This backend is API-only

// Add health check endpoint for Tauri to verify backend is ready
app.MapGet("/api/health", () => Results.Ok(new { status = "healthy", timestamp = DateTime.UtcNow }))
    .WithName("HealthCheck");

// POST endpoint to upload and compress video
app.MapPost("/api/compress", async (
    [FromForm] IFormFile file,
    [FromForm] int? scalePercent,
    [FromForm] string? codec,
    [FromForm] double? targetSizeMb,
    [FromForm] double? sourceDuration,
    
    VideoCompressionService compressionService,
    IConfiguration configuration) =>
{
    if (file == null || file.Length == 0)
    {
        return Results.BadRequest(new { error = "No file uploaded" });
    }

    // Validate file size
    var maxFileSize = configuration.GetValue<long>("FileUpload:MaxFileSizeBytes", 2_147_483_648);
    if (file.Length > maxFileSize)
    {
        var maxSizeMb = maxFileSize / (1024.0 * 1024.0);
        return Results.BadRequest(new { error = $"File is too large. Maximum allowed size is {maxSizeMb:F0} MB." });
    }

    try
    {
        var compressionRequest = new CompressionRequest
        {
            Codec = codec ?? "h264",
            ScalePercent = scalePercent,
            TargetSizeMb = targetSizeMb,
            SourceDuration = sourceDuration,
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
            OutputFilename = job?.OutputFilename,
            OutputMimeType = job?.OutputMimeType,
            Progress = job?.Progress ?? 0,
            EncoderName = job?.EncoderName,
            EncoderIsHardware = job?.EncoderIsHardware
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
app.MapGet("/api/status/{jobId}", (string jobId, VideoCompressionService compressionService, ILogger<Program> logger) =>
{
    logger.LogInformation("Status check request for jobId: {JobId}", jobId);

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
        OutputFilename = job.OutputFilename,
        OutputMimeType = job.OutputMimeType,
        Progress = job.Progress,
        EstimatedSecondsRemaining = job.EstimatedSecondsRemaining,
        QueuePosition = queuePosition,
        EncoderName = job.EncoderName,
        EncoderIsHardware = job.EncoderIsHardware
    });
})
.WithName("GetJobStatus");

// POST endpoint to cancel a job
app.MapPost("/api/cancel/{jobId}", (string jobId, VideoCompressionService compressionService, ILogger<Program> logger) =>
{
    logger.LogInformation("Cancel request for jobId: {JobId}", jobId);

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

    return Results.Ok(new { message = "Job cancelled successfully", jobId });
})
.WithName("CancelJob");

// GET endpoint to check status and download compressed video
app.MapGet("/api/download/{jobId}", async (string jobId, VideoCompressionService compressionService, ILogger<Program> logger) =>
{
    logger.LogInformation("Download request for jobId: {JobId}", jobId);

    if (string.IsNullOrWhiteSpace(jobId))
    {
        logger.LogWarning("Empty jobId provided");
        return Results.BadRequest(new { error = "Job ID is required" });
    }

    var job = compressionService.GetJob(jobId);

    if (job == null)
    {
        logger.LogWarning("Job not found: {JobId}", jobId);
        return Results.NotFound(new { error = $"Job not found. The job may have expired or the application was restarted. JobId: {jobId}" });
    }

    logger.LogInformation("Job found: {JobId}, Status: {Status}", jobId, job.Status);

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
            OutputFilename = job.OutputFilename,
            OutputMimeType = job.OutputMimeType
        ,
            Progress = job.Progress,
            EncoderName = job.EncoderName,
            EncoderIsHardware = job.EncoderIsHardware
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

        try
        {
            var fileBytes = await File.ReadAllBytesAsync(job.OutputPath);
            var fileName = !string.IsNullOrWhiteSpace(job.OutputFilename) ? job.OutputFilename : $"compressed_{job.OriginalFilename}";
            
            logger.LogInformation("Serving file for job: {JobId}, Size: {Size} bytes", jobId, fileBytes.Length);
            
            // Note: Cleanup is handled by JobCleanupService after the retention period
            // This allows multiple downloads and preview before automatic cleanup
            
            var mimeType = !string.IsNullOrWhiteSpace(job.OutputMimeType) ? job.OutputMimeType : "video/mp4";
            return Results.File(fileBytes, mimeType, fileName);
        }
        catch (Exception ex)
        {
            logger.LogError(ex, "Error reading file for job: {JobId}", jobId);
            return Results.Problem(
                title: "Error reading compressed video",
                detail: ex.Message,
                statusCode: 500
            );
        }
    }

    logger.LogWarning("Unexpected job status: {JobId}, Status: {Status}", jobId, job.Status);
    return Results.NotFound(new { error = $"Compressed video not found. Job status: {job.Status}" });
})
.WithName("DownloadCompressedVideo");

// Handle application startup
app.Lifetime.ApplicationStarted.Register(() =>
{
    var urls = app.Urls;
    var url = urls.FirstOrDefault();
    
    Console.WriteLine($"\n🎉 Smart Video Compressor Backend - API Server");
    Console.WriteLine($"📅 Started at {DateTime.Now:O}");
    Console.WriteLine($"📡 API server running at: {url}");
    Console.WriteLine($"🚀 Ready to accept compression requests\n");
});

// Run the application (blocks until stopped)
app.Run();

static string FormatTimeRemaining(int seconds)
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

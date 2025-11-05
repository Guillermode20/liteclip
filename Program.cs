using Microsoft.AspNetCore.Http.Features;
using Microsoft.AspNetCore.Mvc;
using Microsoft.AspNetCore.Server.Kestrel.Core;
using smart_compressor.Models;
using smart_compressor.Services;

var builder = WebApplication.CreateBuilder(args);

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
builder.Services.AddOpenApi();
builder.Services.AddSingleton<VideoCompressionService>();

var app = builder.Build();

// Configure the HTTP request pipeline.
if (app.Environment.IsDevelopment())
{
    app.MapOpenApi();
}

app.UseHttpsRedirection();
app.UseDefaultFiles(); // Enables serving index.html at root URL
app.UseStaticFiles();

// POST endpoint to upload and compress video
app.MapPost("/api/compress", async (
    [FromForm] IFormFile file,
    [FromForm] int? crf,
    [FromForm] int? scalePercent,
    [FromForm] string? mode,
    [FromForm] string? codec,
    [FromForm] double? targetSizeMb,
    [FromForm] double? sourceDuration,
    [FromForm] int? sourceWidth,
    [FromForm] int? sourceHeight,
    [FromForm] long? originalSizeBytes,
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
            Mode = mode ?? "advanced",
            Codec = codec ?? "h264",
            Crf = crf,
            ScalePercent = scalePercent,
            TargetSizeMb = targetSizeMb,
            SourceDuration = sourceDuration,
            SourceWidth = sourceWidth,
            SourceHeight = sourceHeight,
            OriginalSizeBytes = originalSizeBytes
        };

        var jobId = await compressionService.CompressVideoAsync(file, compressionRequest);
        var job = compressionService.GetJob(jobId);
        
        return Results.Ok(new CompressionResult
        {
            JobId = jobId,
            OriginalFilename = file.FileName,
            Status = job?.Status ?? "processing",
            Message = "Video compression started. Use the jobId to download the result.",
            Mode = job?.Mode ?? compressionRequest.Mode,
            Codec = job?.Codec ?? compressionRequest.Codec,
            Crf = job?.Crf,
            ScalePercent = job?.ScalePercent,
            TargetSizeMb = job?.TargetSizeMb,
            TargetBitrateKbps = job?.TargetBitrateKbps,
            OutputFilename = job?.OutputFilename,
            OutputMimeType = job?.OutputMimeType,
            Progress = job?.Progress ?? 0
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

    return Results.Ok(new CompressionResult
    {
        JobId = jobId,
        OriginalFilename = job.OriginalFilename,
        Status = job.Status,
        Message = job.Status switch
        {
            "processing" => $"Video compression is in progress ({job.Progress:F1}%).",
            "completed" => "Video compression completed successfully.",
            "failed" => $"Video compression failed: {job.ErrorMessage}",
            _ => "Unknown status"
        },
        Mode = job.Mode,
        Codec = job.Codec,
        Crf = job.Crf,
        ScalePercent = job.ScalePercent,
        TargetSizeMb = job.TargetSizeMb,
        TargetBitrateKbps = job.TargetBitrateKbps,
        OutputFilename = job.OutputFilename,
        OutputMimeType = job.OutputMimeType,
        Progress = job.Progress
    });
})
.WithName("GetJobStatus");

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
            Mode = job.Mode,
            Codec = job.Codec,
            Crf = job.Crf,
            ScalePercent = job.ScalePercent,
            TargetSizeMb = job.TargetSizeMb,
            TargetBitrateKbps = job.TargetBitrateKbps,
            OutputFilename = job.OutputFilename,
            OutputMimeType = job.OutputMimeType
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
            
            // Clean up files after reading
            compressionService.CleanupJob(jobId);
            
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

app.Run();

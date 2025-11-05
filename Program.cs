using smart_compressor.Models;
using smart_compressor.Services;

var builder = WebApplication.CreateBuilder(args);

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
app.MapPost("/api/compress", async (IFormFile file, int? crf, int? scalePercent, VideoCompressionService compressionService) =>
{
    if (file == null || file.Length == 0)
    {
        return Results.BadRequest(new { error = "No file uploaded" });
    }

    try
    {
        var jobId = await compressionService.CompressVideoAsync(file, crf, scalePercent);
        
        return Results.Ok(new CompressionResult
        {
            JobId = jobId,
            OriginalFilename = file.FileName,
            Status = "processing",
            Message = "Video compression started. Use the jobId to download the result."
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
            "processing" => "Video compression is in progress.",
            "completed" => "Video compression completed successfully.",
            "failed" => $"Video compression failed: {job.ErrorMessage}",
            _ => "Unknown status"
        }
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
            Message = "Video compression is still in progress. Please try again later."
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
            var fileName = $"compressed_{job.OriginalFilename}";
            
            logger.LogInformation("Serving file for job: {JobId}, Size: {Size} bytes", jobId, fileBytes.Length);
            
            // Clean up files after reading
            compressionService.CleanupJob(jobId);
            
            return Results.File(fileBytes, "video/mp4", fileName);
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

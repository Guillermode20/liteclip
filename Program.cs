using Microsoft.AspNetCore.Http.Features;
using Microsoft.AspNetCore.Mvc;
using Microsoft.AspNetCore.Server.Kestrel.Core;
using Microsoft.Extensions.FileProviders;
using smart_compressor.Models;
using smart_compressor.Services;
using System.Windows.Forms;

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
builder.Services.AddSingleton<FfmpegPathResolver>();
builder.Services.AddSingleton<VideoCompressionService>();
builder.Services.AddHostedService<JobCleanupService>();

var app = builder.Build();

// Configure the HTTP request pipeline.
if (app.Environment.IsDevelopment())
{
    app.MapOpenApi();
}

// Disable HTTPS redirection for local webview
// app.UseHttpsRedirection();

// Use embedded static files from the assembly
try
{
    var embeddedProvider = new ManifestEmbeddedFileProvider(typeof(Program).Assembly, "wwwroot");
    app.UseDefaultFiles(new DefaultFilesOptions { FileProvider = embeddedProvider });
    app.UseStaticFiles(new StaticFileOptions { FileProvider = embeddedProvider });
    
    Console.WriteLine("✓ Embedded static files configured");
}
catch (Exception ex)
{
    Console.WriteLine($"⚠️ Error configuring embedded files: {ex.Message}");
    Console.WriteLine("Falling back to physical file provider...");
    
    // Fallback to physical files
    app.UseDefaultFiles();
    app.UseStaticFiles();
}

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
    [FromForm] bool? twoPass,
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
            OriginalSizeBytes = originalSizeBytes,
            TwoPass = twoPass ?? false
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
        Mode = job.Mode,
        Codec = job.Codec,
        Crf = job.Crf,
        ScalePercent = job.ScalePercent,
        TargetSizeMb = job.TargetSizeMb,
        TargetBitrateKbps = job.TargetBitrateKbps,
        OutputFilename = job.OutputFilename,
        OutputMimeType = job.OutputMimeType,
        Progress = job.Progress,
        EstimatedSecondsRemaining = job.EstimatedSecondsRemaining,
        QueuePosition = queuePosition
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

// Handle application startup - open window once server is ready
app.Lifetime.ApplicationStarted.Register(() =>
{
    var urls = app.Urls;
    var url = urls.FirstOrDefault() ?? "http://localhost:5000";
    
    // Replace https with http for webview
    if (url.StartsWith("https://"))
    {
        url = url.Replace("https://", "http://");
    }
    
    Console.WriteLine($"\n🎉 Smart Video Compressor is starting...");
    Console.WriteLine($"📡 Server URL: {url}");
    Console.WriteLine($"🪟 Opening native window...\n");
    
    // Reduced delay for faster startup
    Task.Delay(500).ContinueWith(_ =>
    {
        if (OperatingSystem.IsWindows())
        {
            OpenWindowsWebView(url, app);
        }
        else
        {
            Console.WriteLine($"⚠️ This application only supports Windows.");
            Console.WriteLine($"🌐 Server is running at: {url}");
            Console.WriteLine($"Open this URL in your browser.");
        }
    });
});

// Run the application (blocks until stopped)
app.Run();

[System.Runtime.Versioning.SupportedOSPlatform("windows6.1")]
static void OpenWindowsWebView(string url, WebApplication app)
{
    try
    {
        Console.WriteLine($"Creating WebView2 window for URL: {url}");
        
        // Set the apartment state for COM (required for WebView2)
        var thread = new System.Threading.Thread(() =>
        {
            try
            {
                // Enable visual styles for WinForms
                Application.EnableVisualStyles();
                Application.SetCompatibleTextRenderingDefault(false);
                
                // Create and show the WebView2 window
                var form = new smart_compressor.WebViewWindow(url);
                
                form.FormClosed += (sender, e) =>
                {
                    Console.WriteLine("\n👋 Window closed. Shutting down...");
                    app.Lifetime.StopApplication();
                };
                
                // Run the WinForms application
                Application.Run(form);
            }
            catch (Exception ex)
            {
                Console.WriteLine($"❌ Error in WebView thread: {ex.Message}");
                Console.WriteLine($"Stack trace: {ex.StackTrace}");
            }
        });
        
        // Set thread to STA (Single-Threaded Apartment) mode required by WebView2
        thread.SetApartmentState(System.Threading.ApartmentState.STA);
        thread.Start();
        thread.Join(); // Wait for the thread to complete
    }
    catch (Exception ex)
    {
        Console.WriteLine($"❌ Error creating window: {ex.Message}");
        Console.WriteLine($"Stack trace: {ex.StackTrace}");
        Console.WriteLine($"🌐 Server is still running at: {url}");
        Console.WriteLine($"⏹️  Press Ctrl+C to stop\n");
    }
}

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

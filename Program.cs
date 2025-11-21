using Microsoft.AspNetCore.Http.Features;
using Microsoft.AspNetCore.Mvc;
using Microsoft.AspNetCore.Server.Kestrel.Core;
using Microsoft.Extensions.FileProviders;
using liteclip.Models;
using liteclip.Services;
using liteclip.CompressionStrategies;
using Photino.NET;
using System.Drawing;
// Add missing 'using' statements that were previously implicit
using Microsoft.AspNetCore.Builder;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Hosting;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.Logging;
using System;
using System.IO;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;

namespace liteclip
{
    internal class Program
    {
        [STAThread] // The most important line in the whole file
        static void Main(string[] args)
        {
            // This runs the async main method and blocks the STA thread
            // which is the correct pattern for .NET UI apps.
            RunServerAndWindow(args).GetAwaiter().GetResult();
        }

        // All of your previous Program.cs logic is now inside this async method
        static async Task RunServerAndWindow(string[] args)
        {

            var builder = WebApplication.CreateBuilder(args);

            // Configure logging for maximum server-side logging
            builder.Logging.SetMinimumLevel(LogLevel.Trace);

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
            builder.Services.AddHttpClient();
            builder.Services.AddSingleton<FfmpegPathResolver>();
            builder.Services.AddSingleton<IFfmpegPathResolver>(sp => sp.GetRequiredService<FfmpegPathResolver>());

            builder.Services.AddSingleton<VideoCompressionService>();
            builder.Services.AddSingleton<IVideoCompressionService>(sp => sp.GetRequiredService<VideoCompressionService>());

            // Compression strategies and factory
            builder.Services.AddSingleton<ICompressionStrategy, H264Strategy>();
            builder.Services.AddSingleton<ICompressionStrategy, H265Strategy>();
            builder.Services.AddSingleton<ICompressionStrategyFactory, CompressionStrategyFactory>();

            builder.Services.AddHostedService<JobCleanupService>();
            builder.Services.AddSingleton<UpdateCheckerService>();
            builder.Services.AddSingleton<UserSettingsStore>();

            var app = builder.Build();

            // NOTE: Delay FFmpeg capability probing to background after the UI is loaded.
            // Running the probe synchronously on startup makes the UI wait on long ffmpeg checks.
            // We will start the probe later (non-blocking) once the native window has been loaded.

            // Configure the HTTP request pipeline.
            // Serve static files: physical in Development, embedded in non-Development
            if (app.Environment.IsDevelopment())
            {
                app.UseDefaultFiles();
                app.UseStaticFiles();
                Console.WriteLine("✓ Using physical static files (Development)");
            }
            else
            {
                try
                {
                    var embeddedProvider = new ManifestEmbeddedFileProvider(typeof(Program).Assembly, "wwwroot");
                    app.UseDefaultFiles(new DefaultFilesOptions { FileProvider = embeddedProvider });
                    app.UseStaticFiles(new StaticFileOptions { FileProvider = embeddedProvider });
                    Console.WriteLine("✓ Using embedded static files (Non-Development)");
                }
                catch (InvalidOperationException ex) when (ex.Message.Contains("embedded file manifest"))
                {
                    var physicalProvider = new PhysicalFileProvider(Path.Combine(AppContext.BaseDirectory, "wwwroot"));
                    app.UseDefaultFiles(new DefaultFilesOptions { FileProvider = physicalProvider });
                    app.UseStaticFiles(new StaticFileOptions { FileProvider = physicalProvider });
                    Console.WriteLine("✓ Using physical static files (fallback)");
                }
            }

            // POST endpoint to upload and compress video
            app.MapPost("/api/compress", async (
                [FromForm] IFormFile file,
                [FromForm] int? scalePercent,
                [FromForm] string? codec,
                [FromForm] double? targetSizeMb,
                [FromForm] double? sourceDuration,
                [FromForm] string? segments,
                [FromForm] bool? skipCompression,
                [FromForm] bool? qualityMode,
                [FromForm] bool? ultraMode,
                [FromForm] bool? muteAudio,
                
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
                        UseQualityMode = qualityMode ?? false,
                        UseUltraMode = ultraMode ?? false
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

            app.MapPost("/api/retry/{jobId}", (string jobId, VideoCompressionService compressionService, ILogger<Program> logger) =>
            {
                logger.LogInformation("Retry request for jobId: {JobId}", jobId);

                if (string.IsNullOrWhiteSpace(jobId))
                {
                    return Results.BadRequest(new { error = "Job ID is required" });
                }

                var (success, error) = compressionService.RetryJob(jobId);
                if (!success)
                {
                    return Results.BadRequest(new { error = error ?? "Unable to retry job." });
                }

                return Results.Ok(new { message = "Job re-queued for processing", jobId });
            })
            .WithName("RetryJob");

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

                    try
                    {
                        var fileBytes = await File.ReadAllBytesAsync(job.OutputPath);
                        var fileName = !string.IsNullOrWhiteSpace(job.OutputFilename) ? job.OutputFilename : $"compressed_{job.OriginalFilename}";
                        
                        logger.LogInformation("Serving file for job: {JobId}, Size: {Size} bytes", jobId, fileBytes.Length);
                        
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

            app.MapGet("/api/update", async (UpdateCheckerService updateChecker, ILogger<Program> logger) =>
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

            app.MapGet("/api/settings", async (UserSettingsStore store) =>
            {
                var settings = await store.GetAsync();
                return Results.Ok(settings);
            })
            .WithName("GetUserSettings");

            app.MapPost("/api/settings", async ([FromBody] UserSettings settings, UserSettingsStore store) =>
            {
                var updated = await store.UpdateAsync(settings);
                return Results.Ok(updated);
            })
            .WithName("UpdateUserSettings");

            // Encoder detection endpoint removed: frontend no longer queries encoder capabilities

            // --- Robust Server Startup and Shutdown Logic ---

            var serverReadyTcs = new TaskCompletionSource<string>();
            var cts = new CancellationTokenSource();

            // Register graceful shutdown handler for Ctrl+C and other termination signals
            app.Lifetime.ApplicationStopping.Register(() =>
            {
                try
                {
                    Console.WriteLine("\n🛑 Application shutdown signal received. Cancelling active jobs...");
                    var compressionService = app.Services.GetRequiredService<VideoCompressionService>();
                    compressionService.CancelAllJobs();
                }
                catch (Exception ex)
                {
                    Console.WriteLine($"Warning: Failed to cancel jobs during graceful shutdown: {ex.Message}");
                }
            });

            app.Lifetime.ApplicationStarted.Register(() =>
            {
                try
                {
                    var urls = app.Urls;
                    var url = urls.FirstOrDefault(u => u.StartsWith("http://"));

                    if (url == null)
                    {
                        url = urls.FirstOrDefault()?.Replace("https://", "http://");
                    }
                    
                    if (url == null)
                    {
                        Console.WriteLine($"\n⚠️ Warning: No URL configured. Application may not start correctly.");
                        serverReadyTcs.TrySetException(new InvalidOperationException("No server URL found."));
                        return;
                    }
                    
                    Console.WriteLine($"\n🎉 LiteClip - Fast Video Compression");
                    Console.WriteLine($"📅 Started at {DateTime.Now:O}");
                    Console.WriteLine($"📡 Server running at: {url}");
                    Console.WriteLine($"🪟 Creating native desktop window...\n");
                    
                    serverReadyTcs.TrySetResult(url);
                }
                catch (Exception ex)
                {
                    serverReadyTcs.TrySetException(ex);
                }
            });

            // NOTE: we delay starting the HTTP server until after we show the native UI so the app feels faster

            // Create the native UI early so it appears instantly for the user.
            var window = new PhotinoWindow()
                .SetTitle("LiteClip - Fast Video Compression")
                .SetUseOsDefaultSize(false)
                .SetUseOsDefaultLocation(false)
                .SetResizable(true)
                .SetDevToolsEnabled(true)
                .SetContextMenuEnabled(true)
                .SetLogVerbosity(4);

            window.RegisterWebMessageReceivedHandler((sender, message) =>
                {
                    Console.WriteLine($"Message received from frontend: {message}");
                    if (message == "close-app")
                    {
                        window.Close();
                    }
                });

            // Load a local copy of the frontend early (fast) so UI shows while server starts
            var indexPath = Path.Combine(AppContext.BaseDirectory, "wwwroot", "index.html");
            if (File.Exists(indexPath))
            {
                window.Load(new Uri(indexPath).AbsoluteUri);
            }
            else
            {
                window.Load("about:blank");
            }

            // Server will be started after the UI is shown; we'll navigate the window to the server URL
            // once the HTTP server reports it's ready (below).


            // Skipping duplicate web handler registration — already set above.

            // The UI already loaded a local index earlier - avoid duplicate load.

            // Probe already started earlier; no need to start a second probe.

            // Now start the server in background and wait for it to bind
            var serverTask = app.RunAsync(cts.Token);

            string serverUrl;
            try
            {
                serverUrl = await serverReadyTcs.Task;
            }
            catch (Exception ex)
            {
                Console.WriteLine($"\n❌ Failed to start server: {ex.Message}");
                await serverTask; 
                return; 
            }

            // When server is ready, navigate the UI to the running server
            try
            {
                window.Load(serverUrl);
            }
            catch (Exception ex)
            {
                var logger = app.Services.GetRequiredService<ILogger<Program>>();
                logger.LogWarning(ex, "Failed to navigate to server URL - continuing with existing UI");
            }

            // FFmpeg capability probe removed: no background probing will be performed

            window.SetSize(1200, 800);
            window.Center();

            // This BLOCKS the [STAThread] (Main) until the window is closed.
            // This is the correct UI pattern.
            window.WaitForClose();

            // --- Graceful Shutdown ---
            Console.WriteLine("\n👋 Window closed. Shutting down...");

            // Cancel all active compression jobs and terminate FFmpeg processes
            try
            {
                var compressionService = app.Services.GetRequiredService<VideoCompressionService>();
                compressionService.CancelAllJobs();
            }
            catch (Exception ex)
            {
                Console.WriteLine($"Warning: Failed to cancel jobs during shutdown: {ex.Message}");
            }

            cts.Cancel();
            await serverTask;

            Console.WriteLine("Server stopped. Exiting.");
            Environment.Exit(0);
        }


        // --- Helper Function ---
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
    }
}
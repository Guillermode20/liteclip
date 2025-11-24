using Microsoft.AspNetCore.Http.Features;
using Microsoft.AspNetCore.Mvc;
using Microsoft.AspNetCore.Server.Kestrel.Core;
using Microsoft.Extensions.FileProviders;
using liteclip.Models;
using liteclip.Services;
using liteclip.CompressionStrategies;
using liteclip.Endpoints;
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
        [System.Diagnostics.CodeAnalysis.UnconditionalSuppressMessage("Trimming", "IL2026", Justification = "Endpoint mapping uses reflection; types used are preserved at runtime. This is expected for ASP.NET minimal APIs.")]
        static async Task RunServerAndWindow(string[] args)
        {

            var builder = WebApplication.CreateBuilder(args);

            // Configure logging - be verbose during development, but quieter in production to reduce startup overhead
            builder.Logging.SetMinimumLevel(builder.Environment.IsDevelopment() ? LogLevel.Trace : LogLevel.Information);

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
            builder.Services.AddSingleton<IProgressParser, FfmpegProgressParser>();
            builder.Services.AddSingleton<IFfmpegRunner, FfmpegProcessRunner>();
            builder.Services.AddSingleton<FfmpegProbeService>();

            // New encoder services
            builder.Services.AddSingleton<IFfmpegEncoderProbe, FfmpegEncoderProbe>();
            builder.Services.AddSingleton<IEncoderSelectionService, EncoderSelectionService>();

            builder.Services.AddSingleton<ICompressionPlanner, DefaultCompressionPlanner>();
            builder.Services.AddSingleton<IJobStore, InMemoryJobStore>();

            builder.Services.AddSingleton<IVideoEncodingPipeline, VideoEncodingPipeline>();
            builder.Services.AddSingleton<VideoCompressionService>();
            builder.Services.AddSingleton<IVideoCompressionService>(sp => sp.GetRequiredService<VideoCompressionService>());

            // Compression strategies and factory - now depend on encoder selection service
            builder.Services.AddSingleton<ICompressionStrategy>(sp => new H264Strategy(sp.GetRequiredService<IEncoderSelectionService>()));
            builder.Services.AddSingleton<ICompressionStrategy>(sp => new H265Strategy(sp.GetRequiredService<IEncoderSelectionService>()));
            builder.Services.AddSingleton<ICompressionStrategyFactory, CompressionStrategyFactory>();

            builder.Services.AddHostedService<JobCleanupService>();
            builder.Services.AddSingleton<UpdateCheckerService>();
            builder.Services.AddSingleton<UserSettingsStore>();
            builder.Services.AddSingleton<FfmpegBootstrapper>();

            var app = builder.Build();

            var ffmpegBootstrapper = app.Services.GetRequiredService<FfmpegBootstrapper>();
            var ffmpegProbeService = app.Services.GetRequiredService<FfmpegProbeService>();
            var programLogger = app.Services.GetRequiredService<ILogger<Program>>();
            var ffmpegStartupTask = ffmpegBootstrapper.EnsureReadyAsync();
            _ = ffmpegStartupTask.ContinueWith(t =>
            {
                // Run an async probe in the background to get version info
                _ = Task.Run(async () =>
                {
                    if (t.IsCompletedSuccessfully)
                    {
                        var executable = ffmpegBootstrapper.GetStatus().ExecutablePath;
                        try
                        {
                            var version = await ffmpegProbeService.GetFfmpegVersionAsync();
                            programLogger.LogInformation("FFmpeg ready. Executable path: {Path}, Version: {Version}", executable ?? "(unknown)", version ?? "(unknown)");
                        }
                        catch (Exception ex)
                        {
                            programLogger.LogInformation("FFmpeg ready. Executable path: {Path}", executable ?? "(unknown)");
                            programLogger.LogDebug(ex, "Failed to query FFmpeg version");
                        }

                        Console.WriteLine("✓ FFmpeg ready");
                    }
                    else
                    {
                        programLogger.LogWarning(t.Exception, "FFmpeg initialization failed");
                        Console.WriteLine($"⚠️ FFmpeg initialization failed: {t.Exception?.GetBaseException().Message}");
                    }
                });
            });

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
            
            app.MapCompressionEndpoints();
            app.MapSettingsEndpoints();
            app.MapSystemEndpoints();

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
            // Load user settings to honor StartMaximized preference
            var userSettingsStore = app.Services.GetRequiredService<UserSettingsStore>();
            var userSettings = await userSettingsStore.GetAsync();
            var window = new PhotinoWindow()
                .SetTitle("LiteClip - Fast Video Compression")
                .SetUseOsDefaultSize(false)
                .SetUseOsDefaultLocation(false)
                .SetResizable(true)
                .SetDevToolsEnabled(app.Environment.IsDevelopment())
                .SetContextMenuEnabled(true)
                .SetLogVerbosity(app.Environment.IsDevelopment() ? 4 : 0);

            // Apply user preference: start maximized if requested
            try
            {
                if (userSettings.StartMaximized)
                {
                    window.SetMaximized(true);
                    window.SetMinSize(854, 480); // set a reasonable minimum size when maximized
                }
            }
            catch
            {
                // Photino may not implement SetMaximized on some platforms/versions;
                // if so, gracefully ignore and fallback to a default size below.
            }

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
                // Load local file first to ensure webview is properly initialized on Windows
                var indexUri = new Uri(indexPath);
                window.Load(indexUri.AbsoluteUri);
                Console.WriteLine($"✓ Loaded local index.html while server starts");
            }
            else
            {
                // Fallback to blank page if local file not available
                window.Load("about:blank");
                Console.WriteLine($"⚠️ Local index.html not found at {indexPath}, using blank page");
            }

            // Server will be started after the UI is shown; we'll navigate the window to the server URL
            // once the HTTP server reports it's ready (below).


            // Skipping duplicate web handler registration — already set above.

            // The UI already loaded a local index earlier - avoid duplicate load.

            // Probe already started earlier; no need to start a second probe.

            // Now start the server in background and wait for it to bind
            var serverTask = app.RunAsync(cts.Token);

            string? serverUrl = null;
            var serverReadyTask = serverReadyTcs.Task;
            try
            {
                // Wait for the server to start up, but don't wait indefinitely.
                var completed = await Task.WhenAny(serverReadyTask, Task.Delay(TimeSpan.FromSeconds(10)));
                if (completed != serverReadyTask)
                {
                    Console.WriteLine("\n⚠️ Server start timed out after 10s. Proceeding with UI while continuing to start the server in background.");
                }

                if (serverReadyTask.IsCompletedSuccessfully)
                {
                    serverUrl = serverReadyTask.Result;
                }
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
                if (!string.IsNullOrWhiteSpace(serverUrl))
                {
                    // Warm up the HTTP server (static files, JIT) to improve perceived navigation speed.
                    try
                    {
                        var httpFactory = app.Services.GetRequiredService<IHttpClientFactory>();
                        _ = Task.Run(async () =>
                        {
                            try
                            {
                                var client = httpFactory.CreateClient();
                                client.Timeout = TimeSpan.FromMilliseconds(1000);
                                // Fire-and-forget request; don't await in startup path.
                                await client.GetAsync(serverUrl);
                            }
                            catch
                            {
                                // Ignore warmup failures
                            }
                        });
                    }
                    catch
                    {
                        // If IHttpClientFactory is not available, continue without warmup
                    }

                    window.Load(serverUrl);
                }
            }
            catch (Exception ex)
            {
                var logger = app.Services.GetRequiredService<ILogger<Program>>();
                logger.LogWarning(ex, "Failed to navigate to server URL - continuing with existing UI");
            }

            // FFmpeg capability probe removed: no background probing will be performed

            // If the user didn't request maximized, use the default size and center.
            if (!userSettings.StartMaximized)
            {
                window.SetSize(1200, 800);
                window.Center();
            }

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
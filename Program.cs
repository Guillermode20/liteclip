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

            // NOTE: Do NOT auto-start FFmpeg bootstrap here.
            // We will start it only after the user explicitly consents via /api/ffmpeg/start.

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
                    // Check if embedded files manifest is actually available
                    var assembly = typeof(Program).Assembly;
                    Console.WriteLine($"✓ Assembly: {assembly.FullName}");
                    Console.WriteLine($"✓ BaseDirectory: {AppContext.BaseDirectory}");
                    
                    // Try to get embedded files manifest - if it fails, use physical files
                    var embeddedProvider = new ManifestEmbeddedFileProvider(assembly, "wwwroot");
                    
                    // Test if we can find index.html
                    var fileInfo = embeddedProvider.GetFileInfo("index.html");
                    Console.WriteLine($"✓ Embedded index.html exists: {fileInfo.Exists}");
                    if (fileInfo.Exists)
                    {
                        Console.WriteLine($"✓ Embedded index.html length: {fileInfo.Length}");
                        app.UseDefaultFiles(new DefaultFilesOptions { FileProvider = embeddedProvider });
                        app.UseStaticFiles(new StaticFileOptions { FileProvider = embeddedProvider });
                        Console.WriteLine("✓ Using embedded static files (Non-Development)");
                    }
                    else
                    {
                        throw new InvalidOperationException("Embedded files not found");
                    }
                }
                catch (Exception ex)
                {
                    Console.WriteLine($"⚠️ Embedded files not available, using physical fallback: {ex.Message}");

                    static string? FindWwwRoot()
                    {
                        var candidates = new[]
                        {
                            Path.Combine(AppContext.BaseDirectory, "wwwroot"),
                            Environment.ProcessPath is null
                                ? null
                                : Path.Combine(Path.GetDirectoryName(Environment.ProcessPath)!, "wwwroot"),
                        };

                        foreach (var candidate in candidates.Where(c => !string.IsNullOrEmpty(c)))
                        {
                            if (Directory.Exists(candidate))
                            {
                                return candidate;
                            }
                        }

                        return null;
                    }

                    var physicalRoot = FindWwwRoot();
                    if (physicalRoot is not null)
                    {
                        var physicalProvider = new PhysicalFileProvider(physicalRoot);
                        app.UseDefaultFiles(new DefaultFilesOptions { FileProvider = physicalProvider });
                        app.UseStaticFiles(new StaticFileOptions { FileProvider = physicalProvider });
                        Console.WriteLine($"✓ Using physical static files (fallback) from {physicalRoot}");
                    }
                    else
                    {
                        Console.WriteLine("❌ Could not locate wwwroot next to the executable or extraction directory. UI will not load.");
                        app.MapGet("/", () => Results.Problem("Static assets unavailable. Please reinstall LiteClip."));
                    }
                }
            }
            
            app.MapCompressionEndpoints();
            app.MapSettingsEndpoints();
            app.MapSystemEndpoints();

            // --- Robust Server Startup and Shutdown Logic ---

            var cts = new CancellationTokenSource();
            var lifetime = app.Services.GetRequiredService<IHostApplicationLifetime>();
            var serverReadyTcs = new TaskCompletionSource<string>(TaskCreationOptions.RunContinuationsAsynchronously);

            // Register graceful shutdown handler for Ctrl+C and other termination signals
            lifetime.ApplicationStopping.Register(() =>
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

            // Load user settings to honor StartMaximized preference
            var userSettingsStore = app.Services.GetRequiredService<UserSettingsStore>();
            var userSettings = await userSettingsStore.GetAsync();
            var window = new PhotinoWindow()
                .SetTitle("LiteClip - Fast Video Compression")
                .SetUseOsDefaultSize(false)
                .SetUseOsDefaultLocation(false)
                .SetResizable(true)
                .SetDevToolsEnabled(true) // Enable in production for debugging
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
                    Console.WriteLine("Closing app due to 'close-app' message...");
                    window.Close();
                }
            });

            window.WindowClosing += (sender, e) =>
            {
                Console.WriteLine("\n👋 Window close requested. Stopping host...");
                cts.Cancel();
                lifetime.StopApplication();
                return false;
            };

            lifetime.ApplicationStarted.Register(() =>
            {
                try
                {
                    var urls = app.Urls;
                    var url = urls.FirstOrDefault(u => u.StartsWith("http://")) ??
                              urls.FirstOrDefault()?.Replace("https://", "http://");

                    if (url == null)
                    {
                        throw new InvalidOperationException("No server URL found.");
                    }

                    Console.WriteLine($"\n🎉 LiteClip - Fast Video Compression");
                    Console.WriteLine($"📅 Started at {DateTime.Now:O}");
                    Console.WriteLine($"📡 Server running at: {url}");
                    Console.WriteLine("🪟 Loading native desktop window...\n");

                    serverReadyTcs.TrySetResult(url);
                    window.Load(url);

                    if (!userSettings.StartMaximized)
                    {
                        window.SetSize(1200, 800);
                        window.Center();
                    }
                }
                catch (Exception ex)
                {
                    Console.WriteLine($"\n❌ Failed to determine server URL: {ex.Message}");
                    serverReadyTcs.TrySetException(ex);
                    window.Load("about:blank");
                }
            });

            // Start the HTTP server; Photino will load once ApplicationStarted fires.
            var serverTask = app.RunAsync(cts.Token);

            try
            {
                await serverReadyTcs.Task.WaitAsync(TimeSpan.FromSeconds(30));
            }
            catch (TimeoutException)
            {
                Console.WriteLine("\n⚠️ Server start timed out after 30s. Window will load once the host is ready.");
            }
            catch (Exception ex)
            {
                Console.WriteLine($"\n❌ Failed to start server: {ex.Message}");
                await serverTask;
                return;
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
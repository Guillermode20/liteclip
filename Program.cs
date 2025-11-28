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
            // NOTE: Main MUST stay synchronous and on the STA thread.
            // Photino/WebView2 require their message pump (WaitForClose)
            // to run on this original STA thread, so we avoid async Main
            // and any awaits that could resume on a ThreadPool thread.
            RunServerAndWindow(args);
        }

        // All of the hosting + Photino setup is inside this method.
        // It is intentionally synchronous so that:
        //   - Kestrel is started with StartAsync().GetResult() BEFORE
        //     we call window.Load(serverUrl), and
        //   - window.WaitForClose() always runs on the STA thread.
        [System.Diagnostics.CodeAnalysis.UnconditionalSuppressMessage("Trimming", "IL2026", Justification = "Endpoint mapping uses reflection; types used are preserved at runtime. This is expected for ASP.NET minimal APIs.")]
        static void RunServerAndWindow(string[] args)
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
            // Always serve static files from a physical wwwroot folder.
            // This keeps Debug and Release behavior identical and avoids
            // ManifestEmbeddedFileProvider + single-file quirks that caused
            // the published Photino window to render blank even though
            // the UI worked in a normal browser.
            app.UseDefaultFiles();
            app.UseStaticFiles();
            Console.WriteLine($"✓ Using physical static files ({app.Environment.EnvironmentName})");

            app.MapCompressionEndpoints();
            app.MapSettingsEndpoints();
            app.MapSystemEndpoints();

            // --- Robust Server Startup and Shutdown Logic ---

            var cts = new CancellationTokenSource();
            var lifetime = app.Services.GetRequiredService<IHostApplicationLifetime>();

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
            var userSettings = userSettingsStore.GetAsync().GetAwaiter().GetResult();

            // Prepare a safe WebView2 user data folder
            // This prevents "Window closed immediately" issues caused by locked files from previous zombie processes
            string webView2Path = Path.Combine(Path.GetTempPath(), "LiteClip_WebView2");
            try
            {
                if (Directory.Exists(webView2Path))
                {
                    Directory.Delete(webView2Path, true);
                }
                Directory.CreateDirectory(webView2Path);
            }
            catch
            {
                // If deletion fails, the folder is likely locked by a zombie process.
                // Use a unique path to ensure the app can still start.
                webView2Path = Path.Combine(Path.GetTempPath(), $"LiteClip_WebView2_{DateTime.Now.Ticks}");
                Directory.CreateDirectory(webView2Path);
            }

            var window = new PhotinoWindow()
                .SetTitle("LiteClip - Fast Video Compression")
                .SetUseOsDefaultSize(false)
                .SetUseOsDefaultLocation(false)
                .SetResizable(true)
                .SetDevToolsEnabled(true) // Enable in production for debugging
                .SetContextMenuEnabled(true)
                .SetTemporaryFilesPath(webView2Path) // Explicitly set the user data folder
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

            // Determine the server URL up-front. In Development this respects
            // launchSettings.json; in Release it typically resolves to http://localhost:5000.
            var urls = app.Urls;
            var serverUrl = urls.FirstOrDefault(u => u.StartsWith("http://")) ??
                            urls.FirstOrDefault()?.Replace("https://", "http://") ??
                            "http://localhost:5000";

            Console.WriteLine($"\n🎉 LiteClip - Fast Video Compression");
            Console.WriteLine($"📅 Started at {DateTime.Now:O}");
            Console.WriteLine($"📡 Server running at: {serverUrl}");
            Console.WriteLine("🪟 Loading native desktop window...\n");

            // Start the HTTP server and block until it is listening BEFORE
            // we navigate the Photino window. This fixed the intermittent
            // "blank window" issue where WebView2 never sent any HTTP
            // requests in some runs of the published app.
            app.StartAsync(cts.Token).GetAwaiter().GetResult();

            window.Load(serverUrl);

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
            app.StopAsync(cts.Token).GetAwaiter().GetResult();

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
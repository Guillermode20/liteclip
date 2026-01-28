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
using Microsoft.AspNetCore.Builder;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Hosting;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.Logging;
using System;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Net.Http;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.AspNetCore.Hosting.Server.Features;

namespace liteclip
{
    internal class Program
    {
        [STAThread]
        static void Main(string[] args)
        {
            // NOTE: Main MUST stay synchronous and on the STA thread.
            // Photino/WebView2 require their message pump (WaitForClose)
            // to run on this original STA thread.

            var builder = WebApplication.CreateBuilder(args);

            ConfigureServices(builder);

            var app = builder.Build();

            ConfigurePipeline(app);

            // Keep Development URL configuration in launchSettings/environment.
            // In non-Development, prefer a loopback-only ephemeral port.
            if (!app.Environment.IsDevelopment() && string.IsNullOrWhiteSpace(Environment.GetEnvironmentVariable("ASPNETCORE_URLS")))
            {
                app.Urls.Clear();
                app.Urls.Add("http://127.0.0.1:0");
            }

            var shutdownCts = new CancellationTokenSource();
            var serverReadyTcs = new TaskCompletionSource<string>(TaskCreationOptions.RunContinuationsAsynchronously);

            app.Lifetime.ApplicationStarted.Register(() =>
            {
                try
                {
                    var serverUrl = ResolveServerUrl(app);
                    Console.WriteLine($"\n🎉 LiteClip - Fast Video Compression");
                    Console.WriteLine($"📅 Started at {DateTime.Now:O}");
                    Console.WriteLine($"📡 Server running at: {serverUrl}");

                    serverReadyTcs.TrySetResult(serverUrl);
                }
                catch (Exception ex)
                {
                    serverReadyTcs.TrySetException(ex);
                }
            });

            var hostTask = app.RunAsync(shutdownCts.Token);

            hostTask.ContinueWith(t =>
            {
                var hostException = t.Exception?.GetBaseException() ?? new InvalidOperationException("Host terminated unexpectedly.");
                if (!serverReadyTcs.Task.IsCompleted)
                {
                    serverReadyTcs.TrySetException(hostException);
                }

                Console.WriteLine($"FATAL: Server encountered an error: {hostException}");
            }, TaskContinuationOptions.OnlyOnFaulted);

            // Kick FFmpeg bootstrapper so status endpoints immediately know if binaries already exist.
            _ = Task.Run(() =>
            {
                try
                {
                    var ffmpegBootstrapper = app.Services.GetRequiredService<FfmpegBootstrapper>();
                    ffmpegBootstrapper.PrimeExistingInstallation();
                }
                catch (Exception ex)
                {
                    Console.WriteLine($"Warning: FFmpeg initialization failed: {ex.Message}");
                }
            });

            string serverUrl;
            try
            {
                serverUrl = serverReadyTcs.Task.GetAwaiter().GetResult();
            }
            catch (Exception ex)
            {
                Console.WriteLine($"FATAL: Server failed to start: {ex}");
                shutdownCts.Cancel();

                try
                {
                    hostTask.GetAwaiter().GetResult();
                }
                catch
                {
                    // Already logged by continuation above.
                }

                return;
            }

            var uiUrl = GetUiUrl(app, serverUrl);
            var window = CreatePhotinoWindow(app, uiUrl);

            // This BLOCKS the [STAThread] (Main) until the window is closed.
            window.WaitForClose();

            // --- Graceful Shutdown ---
            Console.WriteLine("\n👋 Window closed. Shutting down...");

            // Cancel all active compression jobs
            try
            {
                var compressionService = app.Services.GetRequiredService<VideoCompressionService>();
                compressionService.CancelAllJobs();
            }
            catch (Exception ex)
            {
                Console.WriteLine($"Warning: Failed to cancel jobs during shutdown: {ex.Message}");
            }

            shutdownCts.Cancel();

            try
            {
                hostTask.GetAwaiter().GetResult();
            }
            catch (Exception ex)
            {
                Console.WriteLine($"Warning: Server shutdown encountered an error: {ex.Message}");
            }

            Console.WriteLine("Server stopped. Exiting.");
            Environment.Exit(0);
        }

        private static string GetUiUrl(WebApplication app, string serverUrl)
        {
            // In Development we typically use the Vite dev server for fast UI iteration.
            // Override with LITECLIP_UI_URL if needed.
            if (app.Environment.IsDevelopment())
            {
                var overrideUrl = Environment.GetEnvironmentVariable("LITECLIP_UI_URL");
                if (!string.IsNullOrWhiteSpace(overrideUrl))
                {
                    return overrideUrl;
                }

                const string viteUrl = "http://localhost:5173";
                if (IsUrlReachable(viteUrl))
                {
                    return viteUrl;
                }

                Console.WriteLine("\n⚠️  Vite dev server not reachable at http://localhost:5173");
                Console.WriteLine("   To use the fast dev UI, run: cd frontend; npm install; npm run dev");
                Console.WriteLine("   Falling back to backend-served static files.\n");
                return serverUrl;
            }

            return serverUrl;
        }

        private static bool IsUrlReachable(string url)
        {
            try
            {
                using var client = new HttpClient
                {
                    Timeout = TimeSpan.FromMilliseconds(300)
                };

                using var request = new HttpRequestMessage(HttpMethod.Head, url);
                using var response = client.Send(request);
                return (int)response.StatusCode >= 200 && (int)response.StatusCode < 500;
            }
            catch
            {
                return false;
            }
        }

        private static void ConfigureServices(WebApplicationBuilder builder)
        {
            // Configure logging
            builder.Logging.SetMinimumLevel(builder.Environment.IsDevelopment() ? LogLevel.Trace : LogLevel.Information);

            // Configure Kestrel limits
            builder.Services.Configure<KestrelServerOptions>(options =>
            {
                options.Limits.MaxRequestBodySize = 2_147_483_648; // 2 GB
            });

            // Configure form options
            builder.Services.Configure<FormOptions>(options =>
            {
                options.MultipartBodyLengthLimit = 2_147_483_648; // 2 GB
                options.ValueLengthLimit = int.MaxValue;
                options.MultipartHeadersLengthLimit = int.MaxValue;
            });

            // Add services
            builder.Services.AddHttpClient();
            builder.Services.AddSingleton<FfmpegPathResolver>();
            builder.Services.AddSingleton<IFfmpegPathResolver>(sp => sp.GetRequiredService<FfmpegPathResolver>());
            builder.Services.AddSingleton<IProgressParser, FfmpegProgressParser>();
            builder.Services.AddSingleton<IFfmpegRunner, FfmpegProcessRunner>();
            builder.Services.AddSingleton<FfmpegProbeService>();
            builder.Services.AddSingleton<VideoMetadataService>();
            builder.Services.AddSingleton<CropDetectionService>();

            builder.Services.AddSingleton<FfmpegEncoderProbe>();
            builder.Services.AddSingleton<EncoderSelectionService>();

            builder.Services.AddSingleton<DefaultCompressionPlanner>();
            builder.Services.AddSingleton<InMemoryJobStore>();

            builder.Services.AddSingleton<VideoCompressionService>();
            builder.Services.AddSingleton<IAppVersionProvider, AppVersionProvider>();

            builder.Services.AddSingleton<ICompressionStrategy>(sp => new H264Strategy(sp.GetRequiredService<EncoderSelectionService>()));
            builder.Services.AddSingleton<ICompressionStrategy>(sp => new H265Strategy(sp.GetRequiredService<EncoderSelectionService>()));
            builder.Services.AddSingleton<ICompressionStrategyFactory, CompressionStrategyFactory>();

            builder.Services.AddHostedService<JobCleanupService>();
            builder.Services.AddSingleton<UpdateCheckerService>();
            builder.Services.AddSingleton<UserSettingsStore>();
            builder.Services.AddSingleton<FfmpegBootstrapper>();
        }

        private static void ConfigurePipeline(WebApplication app)
        {
            if (app.Environment.IsDevelopment())
            {
                app.UseDefaultFiles();
                app.UseStaticFiles();
                Console.WriteLine("✓ Using physical static files (Development)");
            }
            else
            {
                ConfigureEmbeddedResources(app);
            }

            app.MapCompressionEndpoints();
            app.MapSettingsEndpoints();
            app.MapSystemEndpoints();
        }

        private static void ConfigureEmbeddedResources(WebApplication app)
        {
            try
            {
                var assembly = typeof(Program).Assembly;
                var embeddedProvider = new ManifestEmbeddedFileProvider(assembly, "wwwroot");

                // Verify index.html exists
                if (embeddedProvider.GetFileInfo("index.html").Exists)
                {
                    app.UseDefaultFiles(new DefaultFilesOptions { FileProvider = embeddedProvider });
                    app.UseStaticFiles(new StaticFileOptions { FileProvider = embeddedProvider });
                    Console.WriteLine("✓ Using embedded static files (Non-Development)");
                }
                else
                {
                    throw new InvalidOperationException("Embedded wwwroot not found");
                }
            }
            catch (Exception ex)
            {
                Console.WriteLine($"⚠️ Embedded wwwroot not available, using physical fallback: {ex.Message}");
                ConfigurePhysicalFallback(app);
            }
        }

        private static void ConfigurePhysicalFallback(WebApplication app)
        {
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
                Console.WriteLine("❌ Could not locate wwwroot. UI will not load.");
                app.MapGet("/", () => Results.Problem("Static assets unavailable. Please reinstall LiteClip."));
            }
        }

        private static string? FindWwwRoot()
        {
            var candidates = new[]
            {
                Path.Combine(AppContext.BaseDirectory, "wwwroot"),
                Environment.ProcessPath is null ? null : Path.Combine(Path.GetDirectoryName(Environment.ProcessPath)!, "wwwroot"),
            };

            return candidates.FirstOrDefault(c => !string.IsNullOrEmpty(c) && Directory.Exists(c));
        }

        private static PhotinoWindow CreatePhotinoWindow(WebApplication app, string uiUrl)
        {
            // Prepare WebView2 user data folder to prevent locking issues
            var webViewBasePath = Path.Combine(Path.GetTempPath(), "LiteClip_WebView2");
            var webView2Path = PrepareWebViewUserDataFolder(webViewBasePath);

            // Load user settings (best-effort)
            var userSettingsStore = app.Services.GetRequiredService<UserSettingsStore>();
            UserSettings? userSettings = null;
            try
            {
                userSettings = userSettingsStore.GetAsync().GetAwaiter().GetResult();
            }
            catch
            {
                // Ignore settings failures during startup; use defaults.
            }

            var window = new PhotinoWindow()
                .SetTitle("LiteClip - Fast Video Compression")
                .SetUseOsDefaultSize(false)
                .SetUseOsDefaultLocation(false)
                .SetLocation(new Point(-10000, -10000))
                .SetResizable(true)
                .SetDevToolsEnabled(app.Environment.IsDevelopment())
                .SetContextMenuEnabled(true)
                .SetTemporaryFilesPath(webView2Path)
                .SetLogVerbosity(app.Environment.IsDevelopment() ? 4 : 0);

            window.SetMinSize(854, 480);
            window.SetSize(1200, 800);

            var windowShown = 0;

            void RevealWindow()
            {
                if (Interlocked.Exchange(ref windowShown, 1) == 1)
                {
                    return;
                }

                if (userSettings?.StartMaximized == true)
                {
                    window.SetMaximized(true);
                }
                else
                {
                    window.Center();
                }

            }

            window.RegisterWebMessageReceivedHandler((sender, message) =>
            {
                if (message == "close-app")
                {
                    window.Close();
                }
                else if (message == "window-ready")
                {
                    RevealWindow();
                }
            });

            // Fallback in case the frontend never sends the window-ready event.
            _ = Task.Delay(TimeSpan.FromSeconds(5)).ContinueWith(_ => RevealWindow(), TaskScheduler.Default);

            window.WindowClosing += (sender, e) =>
            {
                Console.WriteLine("\n👋 Window close requested.");
                return false; // Allow close
            };

            window.Load(uiUrl);

            return window;
        }

        static string PrepareWebViewUserDataFolder(string basePath)
        {
            Directory.CreateDirectory(basePath);
            CleanupOldWebViewProfiles(basePath);

            var primaryProfilePath = Path.Combine(basePath, "Current");

            try
            {
                if (Directory.Exists(primaryProfilePath))
                {
                    Directory.Delete(primaryProfilePath, true);
                }

                Directory.CreateDirectory(primaryProfilePath);
                return primaryProfilePath;
            }
            catch
            {
                var fallbackPath = Path.Combine(basePath, $"Profile_{DateTimeOffset.UtcNow:yyyyMMdd_HHmmssfff}");
                Directory.CreateDirectory(fallbackPath);
                return fallbackPath;
            }
        }

        static void CleanupOldWebViewProfiles(string basePath)
        {
            try
            {
                var cutoff = DateTimeOffset.UtcNow - TimeSpan.FromDays(1);
                foreach (var directory in Directory.GetDirectories(basePath))
                {
                    try
                    {
                        var info = new DirectoryInfo(directory);
                        if (info.LastWriteTimeUtc < cutoff)
                        {
                            info.Delete(true);
                        }
                    }
                    catch
                    {
                        // Ignore profile cleanup errors; they'll be retried next launch.
                    }
                }
            }
            catch
            {
                // Swallow top-level cleanup errors to avoid blocking startup.
            }
        }

        private static string ResolveServerUrl(WebApplication app)
        {
            var server = app.Services.GetRequiredService<Microsoft.AspNetCore.Hosting.Server.IServer>();
            var serverAddresses = server.Features.Get<IServerAddressesFeature>();
            return serverAddresses?.Addresses.FirstOrDefault() ?? "http://localhost:5000";
        }
    }
}
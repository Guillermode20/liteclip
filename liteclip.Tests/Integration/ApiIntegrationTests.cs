using System.Net;
using System.Net.Http;
using System.Net.Http.Headers;
using System.Text;
using System.Text.Json;
using liteclip.CompressionStrategies;
using liteclip.Models;
using liteclip.Services;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.Http;
using Microsoft.AspNetCore.Server.Kestrel.Core;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Hosting;
using Microsoft.Extensions.Logging;
using Xunit;
using Xunit.Abstractions;

namespace liteclip.Tests.Integration;

public class ApiIntegrationTests : IAsyncLifetime
{
    private IHost? _host;
    private HttpClient? _client;
    private readonly ITestOutputHelper _output;
    private readonly bool _verbose;

    public ApiIntegrationTests(ITestOutputHelper output)
    {
        _output = output;
        var env = Environment.GetEnvironmentVariable("LITECLIP_TEST_VERBOSE");
        _verbose = !string.IsNullOrWhiteSpace(env) &&
                   (env.Equals("1", StringComparison.OrdinalIgnoreCase) ||
                    env.Equals("true", StringComparison.OrdinalIgnoreCase));
    }

    public async Task InitializeAsync()
    {
        var builder = WebApplication.CreateBuilder(new WebApplicationOptions
        {
            Args = Array.Empty<string>(),
            ApplicationName = typeof(VideoCompressionService).Assembly.GetName().Name,
        });

        // Test-friendly configuration: disable FFmpeg download/requirement
        builder.Configuration.AddInMemoryCollection(new Dictionary<string, string?>
        {
            ["FFmpeg:DownloadOnStartup"] = "false",
            ["FFmpeg:Required"] = "false",
            ["FFmpeg:AllowSystemPath"] = "false",
            ["TempPaths:Uploads"] = Path.Combine(Path.GetTempPath(), "liteclip-tests", "uploads"),
            ["TempPaths:Outputs"] = Path.Combine(Path.GetTempPath(), "liteclip-tests", "outputs"),
        });

        builder.Logging.ClearProviders();
        builder.Logging.SetMinimumLevel(LogLevel.Warning);

        // Kestrel + large upload limits (mirrors Program.cs essentials)
        builder.Services.Configure<KestrelServerOptions>(options =>
        {
            options.Limits.MaxRequestBodySize = 2_147_483_648; // 2 GB
        });

        // Core services
        builder.Services.AddHttpClient();
        builder.Services.AddSingleton<FfmpegPathResolver>();
        builder.Services.AddSingleton<IFfmpegPathResolver>(sp => sp.GetRequiredService<FfmpegPathResolver>());
        builder.Services.AddSingleton<IProgressParser, FfmpegProgressParser>();
        builder.Services.AddSingleton<IFfmpegRunner, NoopFfmpegRunner>();
        builder.Services.AddSingleton<FfmpegProbeService>();
        builder.Services.AddSingleton<ICompressionPlanner, DefaultCompressionPlanner>();
        builder.Services.AddSingleton<IJobStore, InMemoryJobStore>();
        builder.Services.AddSingleton<VideoCompressionService>();
        builder.Services.AddSingleton<IVideoCompressionService>(sp => sp.GetRequiredService<VideoCompressionService>());
        builder.Services.AddSingleton<ICompressionStrategy, H264Strategy>();
        builder.Services.AddSingleton<ICompressionStrategy, H265Strategy>();
        builder.Services.AddSingleton<ICompressionStrategyFactory, CompressionStrategyFactory>();
        builder.Services.AddSingleton<UpdateCheckerService>();
        builder.Services.AddSingleton<UserSettingsStore>();
        builder.Services.AddSingleton<FfmpegBootstrapper>();

        var app = builder.Build();

        // Map a subset of endpoints needed for integration tests.
        MapTestEndpoints(app);

        // Bind to a dynamic port
        app.Urls.Clear();
        app.Urls.Add("http://127.0.0.1:0");

        await app.StartAsync();

        var baseAddress = app.Urls.First();
        _host = app;
        _client = new HttpClient { BaseAddress = new Uri(baseAddress) };
    }

    public async Task DisposeAsync()
    {
        if (_client != null)
        {
            _client.Dispose();
        }

        if (_host is IAsyncDisposable asyncDisposable)
        {
            await asyncDisposable.DisposeAsync();
        }
        else
        {
            (_host as IDisposable)?.Dispose();
        }
    }

    private static void MapTestEndpoints(WebApplication app)
    {
        // Focused /api/compress mapping that exercises VideoCompressionService
        // in a skip-compression mode so we avoid FFmpeg while still testing
        // real job creation, temp file handling and response wiring.
        app.MapPost("/api/compress", async (HttpRequest request, VideoCompressionService compressionService) =>
        {
            var form = await request.ReadFormAsync();
            var file = form.Files["file"];

            if (file == null || file.Length == 0)
            {
                return Results.BadRequest(new { error = "No file uploaded" });
            }

            var compressionRequest = new CompressionRequest
            {
                Codec = "h264",
                SkipCompression = true,
                TargetSizeMb = null,
                SourceDuration = null,
                Segments = null,
                UseQualityMode = false,
                MuteAudio = false
            };

            var jobId = await compressionService.CompressVideoAsync(file, compressionRequest);
            var job = compressionService.GetJob(jobId);

            return Results.Ok(new CompressionResult
            {
                JobId = jobId,
                OriginalFilename = file.FileName,
                Status = job?.Status ?? "processing",
                CompressionSkipped = job?.CompressionSkipped ?? false,
                OutputSizeBytes = job?.OutputSizeBytes,
                OutputFilename = job?.OutputFilename,
                OutputMimeType = job?.OutputMimeType
            });
        });

        // /api/status/{jobId}
        app.MapGet("/api/status/{jobId}", (string jobId, VideoCompressionService compressionService) =>
        {
            if (string.IsNullOrWhiteSpace(jobId))
            {
                return Results.BadRequest(new { error = "Job ID is required" });
            }

            var job = compressionService.GetJob(jobId);
            if (job == null)
            {
                return Results.NotFound(new { error = "Job not found" });
            }

            return Results.Ok(new CompressionResult
            {
                JobId = jobId,
                Status = job.Status,
                OriginalFilename = job.OriginalFilename
            });
        });

        // /api/settings
        app.MapGet("/api/settings", async (UserSettingsStore store) =>
        {
            var settings = await store.GetAsync();
            return Results.Ok(settings);
        });

        app.MapPost("/api/settings", async (HttpRequest request, UserSettingsStore store) =>
        {
            var body = await new StreamReader(request.Body).ReadToEndAsync();
            var settings = JsonSerializer.Deserialize<UserSettings>(body, new JsonSerializerOptions
            {
                PropertyNameCaseInsensitive = true
            });
            if (settings == null)
            {
                return Results.BadRequest(new { error = "Invalid settings payload" });
            }

            var updated = await store.UpdateAsync(settings);
            return Results.Ok(updated);
        });
    }

    [Fact]
    public async Task Compress_WithoutFile_ReturnsBadRequest()
    {
        Assert.NotNull(_client);
        using var content = new MultipartFormDataContent();
        var response = await _client!.PostAsync("/api/compress", content);
        if (_verbose)
        {
            _output.WriteLine("[Compress_WithoutFile] Status: {0}", response.StatusCode);
        }
        // In this lightweight host we only verify that the endpoint is wired
        // and does not succeed when no file is provided.
        Assert.False(response.IsSuccessStatusCode);
    }

    [Fact]
    public async Task Compress_WithSampleVideo_CreatesCompletedSkippedJob()
    {
        Assert.NotNull(_client);

        // Resolve the sample asset from the test output directory
        var assetPath = Path.Combine(AppContext.BaseDirectory, "Assets", "sample.mp4");
        Assert.True(File.Exists(assetPath), $"Expected test asset not found at {assetPath}");

        if (_verbose)
        {
            var fileInfo = new FileInfo(assetPath);
            _output.WriteLine("[Compress_WithSampleVideo] Asset path: {0} (size: {1} bytes)", assetPath, fileInfo.Length);
        }

        await using var fileStream = File.OpenRead(assetPath);
        using var fileContent = new StreamContent(fileStream);
        fileContent.Headers.ContentType = new MediaTypeHeaderValue("video/mp4");

        using var form = new MultipartFormDataContent
        {
            { fileContent, "file", "sample.mp4" }
        };

        var response = await _client!.PostAsync("/api/compress", form);
        response.EnsureSuccessStatusCode();

        var json = await response.Content.ReadAsStringAsync();
        if (_verbose)
        {
            _output.WriteLine("[Compress_WithSampleVideo] Response JSON: {0}", json);
        }
        var result = JsonSerializer.Deserialize<CompressionResult>(json, new JsonSerializerOptions
        {
            PropertyNameCaseInsensitive = true
        });

        Assert.NotNull(result);
        Assert.False(string.IsNullOrWhiteSpace(result!.JobId));
        Assert.Equal("completed", result.Status);
        Assert.True(result.CompressionSkipped);
        Assert.True(result.OutputSizeBytes.HasValue && result.OutputSizeBytes.Value > 0);
    }

    [Fact]
    public async Task Status_UnknownJob_ReturnsNotFound()
    {
        Assert.NotNull(_client);
        var response = await _client!.GetAsync("/api/status/unknown-job-id");
        if (_verbose)
        {
            _output.WriteLine("[Status_UnknownJob] Status: {0}", response.StatusCode);
            var body = await response.Content.ReadAsStringAsync();
            _output.WriteLine("[Status_UnknownJob] Body: {0}", body);
        }
        Assert.Equal(HttpStatusCode.NotFound, response.StatusCode);
    }

    [Fact]
    public async Task Settings_RoundTrip_Works()
    {
        Assert.NotNull(_client);

        var initial = await _client!.GetAsync("/api/settings");
        initial.EnsureSuccessStatusCode();
        var initialJson = await initial.Content.ReadAsStringAsync();
        var initialSettings = JsonSerializer.Deserialize<UserSettings>(initialJson, new JsonSerializerOptions
        {
            PropertyNameCaseInsensitive = true
        }) ?? new UserSettings();

        if (_verbose)
        {
            _output.WriteLine("[Settings_RoundTrip] Initial settings JSON: {0}", initialJson);
        }

        initialSettings.StartMaximized = !initialSettings.StartMaximized;

        var payload = JsonSerializer.Serialize(initialSettings);
        using var content = new StringContent(payload, Encoding.UTF8, "application/json");

        var post = await _client.PostAsync("/api/settings", content);
        post.EnsureSuccessStatusCode();
        var postJson = await post.Content.ReadAsStringAsync();
        if (_verbose)
        {
            _output.WriteLine("[Settings_RoundTrip] Updated settings JSON: {0}", postJson);
        }
        var updated = JsonSerializer.Deserialize<UserSettings>(postJson, new JsonSerializerOptions
        {
            PropertyNameCaseInsensitive = true
        });

        Assert.NotNull(updated);
        Assert.Equal(initialSettings.StartMaximized, updated!.StartMaximized);
    }

    private sealed class NoopFfmpegRunner : IFfmpegRunner
    {
        public Task<FfmpegRunResult> RunAsync(
            string jobId,
            IReadOnlyList<string> arguments,
            double? totalDuration,
            int passNumber,
            int totalPasses,
            Action<FfmpegProgressUpdate>? onProgress,
            Action<System.Diagnostics.Process>? onProcessStarted = null,
            CancellationToken cancellationToken = default)
        {
            return Task.FromResult(new FfmpegRunResult
            {
                ExitCode = 0,
                StandardError = string.Empty
            });
        }
    }
}

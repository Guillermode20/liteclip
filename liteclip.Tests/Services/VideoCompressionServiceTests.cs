using System.IO;
using liteclip.CompressionStrategies;
using liteclip.Services;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.Logging.Abstractions;
using Xunit;

namespace liteclip.Tests.Services;

public class VideoCompressionServiceTests
{
    [Fact]
    public void Constructor_InitializesWithDefaults_WhenConfigMissing()
    {
        var tempRoot = Path.Combine(Path.GetTempPath(), "liteclip-tests", Guid.NewGuid().ToString("N"));

        var inMemory = new Dictionary<string, string?>
        {
            ["TempPaths:Uploads"] = Path.Combine(tempRoot, "uploads"),
            ["TempPaths:Outputs"] = Path.Combine(tempRoot, "outputs")
        };

        IConfiguration config = new ConfigurationBuilder()
            .AddInMemoryCollection(inMemory)
            .Build();

        var logger = new NullLogger<VideoCompressionService>();
        var ffmpegResolver = new FfmpegPathResolver(new NullLogger<FfmpegPathResolver>(), config);
        var encoderSelectionService = new MockEncoderSelectionService();
        var strategyFactory = new CompressionStrategyFactory(new ICompressionStrategy[]
        {
            new H264Strategy(encoderSelectionService),
            new H265Strategy(encoderSelectionService)
        });

        var planner = new DefaultCompressionPlanner();
        var jobStore = new InMemoryJobStore();

        var ffmpegRunner = new NoopFfmpegRunner();
        var service = new VideoCompressionService(config, logger, ffmpegResolver, ffmpegRunner, strategyFactory, planner, jobStore, encoderSelectionService);

        Assert.NotNull(service);
        Assert.True(Directory.Exists(Path.Combine(tempRoot, "uploads")));
        Assert.True(Directory.Exists(Path.Combine(tempRoot, "outputs")));
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

    // Mock implementation for testing - extends concrete class
    private class MockEncoderSelectionService : EncoderSelectionService
    {
        public MockEncoderSelectionService() : base(new MockEncoderProbe(), NullLogger<EncoderSelectionService>.Instance) { }
        
        private class MockEncoderProbe : FfmpegEncoderProbe
        {
            public MockEncoderProbe() : base(new MockPathResolver(), NullLogger<FfmpegEncoderProbe>.Instance) { }
            
            public override bool IsEncoderAvailable(string encoderName) => false;
            public override string GetBestEncoder(string codecKey, string[] preferredEncoders, string fallbackEncoder) => fallbackEncoder;
            
            private class MockPathResolver : IFfmpegPathResolver
            {
                public string? ResolveFfmpegPath() => "/usr/bin/ffmpeg";
                public string? ResolveFfprobePath() => "/usr/bin/ffprobe";
            }
        }
    }
}

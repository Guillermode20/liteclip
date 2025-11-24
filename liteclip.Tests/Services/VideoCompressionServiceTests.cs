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
        var strategyFactory = new CompressionStrategyFactory(new ICompressionStrategy[]
        {
            new H264Strategy(),
            new H265Strategy()
        });

        var planner = new DefaultCompressionPlanner();
        var jobStore = new InMemoryJobStore();

        var service = new VideoCompressionService(config, logger, ffmpegResolver, strategyFactory, planner, jobStore);

        Assert.NotNull(service);
        Assert.True(Directory.Exists(Path.Combine(tempRoot, "uploads")));
        Assert.True(Directory.Exists(Path.Combine(tempRoot, "outputs")));
    }
}

using System.Threading.Tasks;
using liteclip.Services;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.Logging.Abstractions;
using Xunit;

namespace liteclip.Tests.Services;

public class FfmpegBootstrapperTests
{
    private sealed class NoOpResolver : IFfmpegPathResolver
    {
        public string? ResolveFfmpegPath() => null;
        public string? ResolveFfprobePath() => null;
    }

    [Fact]
    public async Task EnsureReadyAsync_WithDownloadDisabled_DoesNotThrow()
    {
        var inMemory = new Dictionary<string, string?>
        {
            ["FFmpeg:DownloadOnStartup"] = "false",
            ["FFmpeg:Required"] = "false"
        };
        IConfiguration config = new ConfigurationBuilder()
            .AddInMemoryCollection(inMemory)
            .Build();

        var bootstrapper = new FfmpegBootstrapper(
            new NullLogger<FfmpegBootstrapper>(),
            new NoOpResolver(),
            config);

        var exception = await Record.ExceptionAsync(() => bootstrapper.EnsureReadyAsync());

        Assert.Null(exception);
    }
}

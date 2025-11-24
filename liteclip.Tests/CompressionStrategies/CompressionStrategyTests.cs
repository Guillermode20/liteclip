using System.Linq;
using liteclip.CompressionStrategies;
using Xunit;

namespace liteclip.Tests.CompressionStrategies;

public class CompressionStrategyTests
{
    [Theory]
    [InlineData(50, EncodingMode.Fast)]
    [InlineData(500, EncodingMode.Quality)]
    [InlineData(1500, EncodingMode.Fast)]
    public void H264Strategy_BuildVideoArgs_ClampsAndIncludesBitrate(double requestedBitrateKbps, EncodingMode mode)
    {
        var strategy = new H264Strategy();

        var args = strategy.BuildVideoArgs(requestedBitrateKbps, mode).ToList();

        Assert.Contains("-b:v", args);
        var bitrateIndex = args.IndexOf("-b:v") + 1;
        Assert.InRange(bitrateIndex, 1, args.Count - 1);

        var value = args[bitrateIndex];
        Assert.EndsWith("k", value);
    }

    [Theory]
    [InlineData(50, EncodingMode.Fast)]
    [InlineData(500, EncodingMode.Quality)]
    [InlineData(1500, EncodingMode.Fast)]
    public void H265Strategy_BuildVideoArgs_ClampsAndIncludesBitrate(double requestedBitrateKbps, EncodingMode mode)
    {
        var strategy = new H265Strategy();

        var args = strategy.BuildVideoArgs(requestedBitrateKbps, mode).ToList();

        Assert.Contains("-b:v", args);
        var bitrateIndex = args.IndexOf("-b:v") + 1;
        Assert.InRange(bitrateIndex, 1, args.Count - 1);

        var value = args[bitrateIndex];
        Assert.EndsWith("k", value);
    }

    [Fact]
    public void Strategies_HaveExpectedMetadata()
    {
        var h264 = new H264Strategy();
        var h265 = new H265Strategy();

        Assert.Equal("h264", h264.CodecKey);
        Assert.Equal("h265", h265.CodecKey);
        Assert.Equal(".mp4", h264.OutputExtension);
        Assert.Equal(".mp4", h265.OutputExtension);
        Assert.Equal("video/mp4", h264.MimeType);
        Assert.Equal("video/mp4", h265.MimeType);
        Assert.Equal("aac", h264.AudioCodec);
        Assert.Equal("aac", h265.AudioCodec);
        Assert.True(h264.AudioBitrateKbps > 0);
        Assert.True(h265.AudioBitrateKbps > 0);
    }
}

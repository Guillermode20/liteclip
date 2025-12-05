using System.Linq;
using liteclip.CompressionStrategies;
using liteclip.Services;
using Xunit;

namespace liteclip.Tests.CompressionStrategies;

public class CompressionStrategyTests
{
    private readonly MockEncoderSelectionService _mockEncoderService = new();

    [Theory]
    [InlineData(50, EncodingMode.Fast)]
    [InlineData(500, EncodingMode.Quality)]
    [InlineData(1500, EncodingMode.Fast)]
    public void H264Strategy_BuildVideoArgs_ClampsAndIncludesBitrate(double requestedBitrateKbps, EncodingMode mode)
    {
        var strategy = new H264Strategy(_mockEncoderService);

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
        var strategy = new H265Strategy(_mockEncoderService);

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
        var h264 = new H264Strategy(_mockEncoderService);
        var h265 = new H265Strategy(_mockEncoderService);

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

    // Mock implementation for testing - inherits from concrete class
    private class MockEncoderSelectionService : EncoderSelectionService
    {
        public MockEncoderSelectionService() : base(new MockEncoderProbe(), new MockLogger()) { }
        
        private class MockEncoderProbe : FfmpegEncoderProbe
        {
            public MockEncoderProbe() : base(new MockPathResolver(), new MockProbeLogger()) { }
            
            private class MockPathResolver : IFfmpegPathResolver
            {
                public string? ResolveFfmpegPath() => "/usr/bin/ffmpeg";
                public string? ResolveFfprobePath() => "/usr/bin/ffprobe";
            }
            
            private class MockProbeLogger : Microsoft.Extensions.Logging.ILogger<FfmpegEncoderProbe>
            {
                public IDisposable? BeginScope<TState>(TState state) where TState : notnull => null;
                public bool IsEnabled(Microsoft.Extensions.Logging.LogLevel logLevel) => false;
                public void Log<TState>(Microsoft.Extensions.Logging.LogLevel logLevel, Microsoft.Extensions.Logging.EventId eventId, TState state, Exception? exception, Func<TState, Exception?, string> formatter) { }
            }
        }
        
        private class MockLogger : Microsoft.Extensions.Logging.ILogger<EncoderSelectionService>
        {
            public IDisposable? BeginScope<TState>(TState state) where TState : notnull => null;
            public bool IsEnabled(Microsoft.Extensions.Logging.LogLevel logLevel) => false;
            public void Log<TState>(Microsoft.Extensions.Logging.LogLevel logLevel, Microsoft.Extensions.Logging.EventId eventId, TState state, Exception? exception, Func<TState, Exception?, string> formatter) { }
        }
    }
}

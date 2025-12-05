using System;
using System.Collections.Generic;
using liteclip.Services;
using Microsoft.Extensions.Logging.Abstractions;
using Xunit;

namespace liteclip.Tests.Services;

public class EncoderSelectionServiceTests
{
    [Fact]
    public void GetBestEncoder_PrefersHardwareWhenAvailable()
    {
        var available = new[] { "h264_qsv", "libx264" };
        var probe = new FakeFfmpegEncoderProbe(available);
        var logger = new NullLogger<EncoderSelectionService>();
        var service = new EncoderSelectionService(probe, logger);

        var best = service.GetBestEncoder("h264");

        Assert.Equal("h264_qsv", best);
    }

    [Fact]
    public void GetBestEncoder_FallsBackToSoftware_WhenNoHardwareAvailable()
    {
        var available = Array.Empty<string>();
        var probe = new FakeFfmpegEncoderProbe(available);
        var logger = new NullLogger<EncoderSelectionService>();
        var service = new EncoderSelectionService(probe, logger);

        var best = service.GetBestEncoder("h264");

        Assert.Equal("libx264", best);
    }

    [Theory]
    [InlineData("h264_nvenc", true)]
    [InlineData("hevc_qsv", true)]
    [InlineData("libx264", false)]
    [InlineData("libx265", false)]
    [InlineData("", false)]
    [InlineData(null, false)]
    public void IsHardwareEncoder_ClassifiesEncodersByName(string? encoderName, bool expected)
    {
        var probe = new FakeFfmpegEncoderProbe(Array.Empty<string>());
        var logger = new NullLogger<EncoderSelectionService>();
        var service = new EncoderSelectionService(probe, logger);

        var isHardware = EncoderSelectionService.IsHardwareEncoder(encoderName ?? string.Empty);

        Assert.Equal(expected, isHardware);
    }

    private sealed class FakeFfmpegEncoderProbe : FfmpegEncoderProbe
    {
        private readonly HashSet<string> _available;

        public FakeFfmpegEncoderProbe(IEnumerable<string> availableEncoders) 
            : base(new FakePathResolver(), NullLogger<FfmpegEncoderProbe>.Instance)
        {
            _available = new HashSet<string>(availableEncoders, StringComparer.OrdinalIgnoreCase);
        }

        public override bool IsEncoderAvailable(string encoderName)
        {
            if (string.IsNullOrWhiteSpace(encoderName))
            {
                return false;
            }

            return _available.Contains(encoderName);
        }

        public override string GetBestEncoder(string codecKey, string[] preferredEncoders, string fallbackEncoder)
        {
            foreach (var encoder in preferredEncoders)
            {
                if (IsEncoderAvailable(encoder))
                {
                    return encoder;
                }
            }

            return fallbackEncoder;
        }
        
        private sealed class FakePathResolver : IFfmpegPathResolver
        {
            public string? ResolveFfmpegPath() => "/usr/bin/ffmpeg";
            public string? ResolveFfprobePath() => "/usr/bin/ffprobe";
        }
    }
}

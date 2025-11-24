using System;
using System.Collections.Generic;
using liteclip.Services;
using Microsoft.Extensions.Logging.Abstractions;
using Xunit;

namespace liteclip.Tests.Services;

public class EncoderSelectionServiceTests
{
    [Fact]
    public void GetBestH264Encoder_PrefersHardwareWhenAvailable()
    {
        var available = new[] { "h264_qsv", "libx264" };
        var probe = new FakeFfmpegEncoderProbe(available);
        var logger = new NullLogger<EncoderSelectionService>();
        var service = new EncoderSelectionService(probe, logger);

        var best = service.GetBestH264Encoder();

        Assert.Equal("h264_qsv", best);
    }

    [Fact]
    public void GetBestH264Encoder_FallsBackToSoftware_WhenNoHardwareAvailable()
    {
        var available = Array.Empty<string>();
        var probe = new FakeFfmpegEncoderProbe(available);
        var logger = new NullLogger<EncoderSelectionService>();
        var service = new EncoderSelectionService(probe, logger);

        var best = service.GetBestH264Encoder();

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

        var isHardware = service.IsHardwareEncoder(encoderName ?? string.Empty);

        Assert.Equal(expected, isHardware);
    }

    private sealed class FakeFfmpegEncoderProbe : IFfmpegEncoderProbe
    {
        private readonly HashSet<string> _available;

        public FakeFfmpegEncoderProbe(IEnumerable<string> availableEncoders)
        {
            _available = new HashSet<string>(availableEncoders, StringComparer.OrdinalIgnoreCase);
        }

        public bool IsEncoderAvailable(string encoderName)
        {
            if (string.IsNullOrWhiteSpace(encoderName))
            {
                return false;
            }

            return _available.Contains(encoderName);
        }

        public string GetBestEncoder(string codecKey, string[] preferredEncoders, string fallbackEncoder)
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

        public void ClearCache()
        {
            // No-op for fake implementation
        }
    }
}

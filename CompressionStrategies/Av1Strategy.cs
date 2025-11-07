using System;
using System.Collections.Generic;

namespace smart_compressor.CompressionStrategies;

public class Av1Strategy : ICompressionStrategy
{
    public string CodecKey => "av1";
    public string OutputExtension => ".webm";
    public string MimeType => "video/webm";
    public string VideoCodec => "libaom-av1";
    public string AudioCodec => "libopus";
    public int AudioBitrateKbps => 128;

    public IEnumerable<string> BuildVideoArgs(double videoBitrateKbps)
    {
        var targetBitrate = Math.Max(100, Math.Round(videoBitrateKbps));
        var maxRate = Math.Round(targetBitrate * 1.03);
        var minRate = Math.Round(targetBitrate * 0.97);
        var buffer = Math.Round(targetBitrate * 1.0);

        var args = new List<string>
        {
            "-c:v", VideoCodec,
            "-c:v", VideoCodec,
            // For highest quality prefer lowest cpu-used (slower) for libaom-av1
            "-cpu-used", "0",
            "-row-mt", "1",
            "-b:v", $"{targetBitrate}k",
            "-maxrate", $"{maxRate}k",
            "-bufsize", $"{buffer}k",
            "-minrate", $"{minRate}k"
        };

        return args;
    }

    public IEnumerable<string> BuildAudioArgs()
    {
        return new List<string> { "-c:a", AudioCodec, "-b:a", $"{AudioBitrateKbps}k", "-ac", "2" };
    }

    public IEnumerable<string> BuildContainerArgs()
    {
        return Array.Empty<string>();
    }

    public IEnumerable<string> GetPassExtras(int passNumber, string passLogFile)
    {
        // Use webm for av1 passes
        return new[] { "-pass", passNumber.ToString(), "-passlogfile", passLogFile, "-f", "webm" };
    }
}

using System;
using System.Collections.Generic;

namespace liteclip.CompressionStrategies;

public class Av1Strategy : ICompressionStrategy
{
    public string CodecKey => "av1";
    public string OutputExtension => ".webm";
    public string MimeType => "video/webm";
    public string VideoCodec => "libaom-av1";
    public string AudioCodec => "libopus";
    public int AudioBitrateKbps => 128;

    public IEnumerable<string> BuildVideoArgs(double videoBitrateKbps, EncodingMode mode)
    {
        _ = mode;
        var targetBitrate = Math.Max(100, Math.Round(videoBitrateKbps));
        var maxRate = Math.Round(targetBitrate * 1.06);
        var minRate = Math.Round(targetBitrate * 0.85);
        var buffer = Math.Round(targetBitrate * 1.5);

        var args = new List<string>
        {
            "-c:v", VideoCodec,
            // Moderate speed; still interactive, but much higher quality
            "-cpu-used", "3",
            // Enable lookahead and restoration so AV1 can shape bitrate
            "-lag-in-frames", "25",
            "-tile-columns", "1",
            "-tile-rows", "0",
            "-enable-cdef", "1",
            "-enable-restoration", "1",
            // GOP settings
            "-g", "160",
            "-sc_threshold", "0",
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

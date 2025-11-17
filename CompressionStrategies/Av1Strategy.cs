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

    public IEnumerable<string> BuildVideoArgs(double videoBitrateKbps, bool useQualityMode, bool useUltraMode = false)
    {
        _ = useQualityMode;
        var targetBitrate = Math.Max(100, Math.Round(videoBitrateKbps));
        var maxRate = Math.Round(targetBitrate * 1.03);
        var minRate = Math.Round(targetBitrate * 0.97);
        var buffer = Math.Round(targetBitrate * 1.0);

        var args = new List<string>
        {
            "-c:v", VideoCodec,
            // cpu-used 5 for faster speed (still good quality)
            "-cpu-used", "5",
            // Disable lookahead for much faster encoding on short clips
            "-lag-in-frames", "0",
            // Use 1 tile column for faster processing
            "-tile-columns", "1",
            "-tile-rows", "0",
            // Disable restoration for speed
            "-enable-cdef", "0",
            "-enable-restoration", "0",
            // GOP settings
            "-g", "120",
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

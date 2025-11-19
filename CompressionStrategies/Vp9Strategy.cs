using System;
using System.Collections.Generic;

namespace liteclip.CompressionStrategies;

public class Vp9Strategy : ICompressionStrategy
{
    public string CodecKey => "vp9";
    public string OutputExtension => ".webm";
    public string MimeType => "video/webm";
    public string VideoCodec => "libvpx-vp9";
    public string AudioCodec => "libopus";
    public int AudioBitrateKbps => 128;

    public IEnumerable<string> BuildVideoArgs(double videoBitrateKbps, EncodingMode mode)
    {
        _ = mode;
        var targetBitrate = Math.Max(100, Math.Round(videoBitrateKbps));
        var maxRate = Math.Round(targetBitrate * 1.08);
        var buffer = Math.Round(targetBitrate * 1.6);

        var args = new List<string>
        {
            "-c:v", VideoCodec,
            // deadline good and cpu-used 1 for higher quality
            "-deadline", "good",
            "-cpu-used", "1",
            // Enable lookahead and alt-ref so VP9 can allocate bits better
            "-lag-in-frames", "15",
            "-auto-alt-ref", "1",
            // Use 2 tile columns to keep decode fast while improving
            // encoder parallelism.
            "-tile-columns", "2",
            // GOP and scene detection
            "-g", "160",
            "-sc_threshold", "0",
            "-b:v", $"{targetBitrate}k",
            "-maxrate", $"{maxRate}k",
            "-bufsize", $"{buffer}k"
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
        // Use webm for vp9 passes
        return new[] { "-pass", passNumber.ToString(), "-passlogfile", passLogFile, "-f", "webm" };
    }
}

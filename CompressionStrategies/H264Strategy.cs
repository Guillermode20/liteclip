using System;
using System.Collections.Generic;

namespace smart_compressor.CompressionStrategies;

public class H264Strategy : ICompressionStrategy
{
    public string CodecKey => "h264";
    public string OutputExtension => ".mp4";
    public string MimeType => "video/mp4";
    public string VideoCodec => "libx264";
    public string AudioCodec => "aac";
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
            "-preset", "slower",
            "-pix_fmt", "yuv420p",
            "-g", "60",
            "-sc_threshold", "0",
            "-bf", "4",
            "-refs", "5",
            "-b:v", $"{targetBitrate}k",
            "-maxrate", $"{maxRate}k",
            "-bufsize", $"{buffer}k",
            "-minrate", $"{minRate}k"
        };

        // Enhance adaptive quantization and psy tuning for better perceived quality
        args.AddRange(new[] { "-x264-params", "aq-mode=3:aq-strength=1.0:rc_lookahead=60:psy=1:psy_rd=1.0" });

        return args;
    }

    public IEnumerable<string> BuildAudioArgs()
    {
        return new List<string> { "-c:a", AudioCodec, "-b:a", $"{AudioBitrateKbps}k" };
    }

    public IEnumerable<string> BuildContainerArgs()
    {
        return new[] { "-movflags", "+faststart" };
    }

    public IEnumerable<string> GetPassExtras(int passNumber, string passLogFile)
    {
        // Use mp4 container for passes
        return new[] { "-pass", passNumber.ToString(), "-passlogfile", passLogFile, "-f", "mp4" };
    }
}

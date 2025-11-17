using System;
using System.Collections.Generic;
using System.Diagnostics;

namespace liteclip.CompressionStrategies;

public class H264Strategy : ICompressionStrategy
{
    private string? _detectedEncoder;
    private bool _encoderDetected = false;
    
    public string CodecKey => "h264";
    public string OutputExtension => ".mp4";
    public string MimeType => "video/mp4";
    public string VideoCodec => GetBestEncoder();
    public string AudioCodec => "aac";
    public int AudioBitrateKbps => 128;

    private string GetBestEncoder()
    {
        if (_encoderDetected)
            return _detectedEncoder ?? "libx264";
            
        _encoderDetected = true;
        
        // Try hardware encoders in order of preference: NVENC > QuickSync > AMF > Software
        var encodersToTry = new[] { "h264_nvenc", "h264_qsv", "h264_amf" };
        
        foreach (var encoder in encodersToTry)
        {
            if (IsEncoderAvailable(encoder))
            {
                _detectedEncoder = encoder;
                return encoder;
            }
        }
        
        _detectedEncoder = "libx264";
        return "libx264";
    }
    
    private static bool IsEncoderAvailable(string encoderName)
    {
        try
        {
            // Try two more robust tests before falling back to a minimal one:
            // 1) testsrc with NV12 pix_fmt, reasonable resolution/frame-rate and GOP (-g)
            // 2) fallback to the older minimal color test if the first fails
            var attempts = new[]
            {
                // Use testsrc (video test signal) and set NV12 pixel format which AMF expects
                $"-loglevel error -f lavfi -i testsrc=duration=0.5:size=1280x720:rate=30 -pix_fmt nv12 -c:v {encoderName} -g 60 -b:v 2000k -bf 0 -f null -",
                // Fallback minimal test (keeps previous behavior)
                $"-f lavfi -i color=black:s=64x64:d=0.1 -c:v {encoderName} -f null -"
            };

            foreach (var args in attempts)
            {
                var psi = new ProcessStartInfo
                {
                    FileName = "ffmpeg",
                    Arguments = args,
                    RedirectStandardOutput = true,
                    RedirectStandardError = true,
                    UseShellExecute = false,
                    CreateNoWindow = true
                };

                using var process = Process.Start(psi);
                if (process == null) continue;

                // Read stderr (some encoders print diagnostics there)
                var stdOut = process.StandardOutput.ReadToEnd();
                var error = process.StandardError.ReadToEnd();
                process.WaitForExit();

                // If exit code is zero, the encoder initialized successfully
                if (process.ExitCode == 0)
                {
                    return true;
                }

                // If the error clearly indicates the encoder is unavailable, break early
                var errLower = error.ToLowerInvariant();
                if (errLower.Contains("not available") || errLower.Contains("cannot load") || errLower.Contains("no nvenc") )
                {
                    return false;
                }

                // Otherwise try the next attempt (the fallback may succeed for some drivers)
            }

            return false;
        }
        catch
        {
            return false;
        }
    }

    public IEnumerable<string> BuildVideoArgs(double videoBitrateKbps, bool useQualityMode)
    {
        var targetBitrate = Math.Max(100, Math.Round(videoBitrateKbps));
        // Base CBR-ish defaults; strategies can override per encoder
        var maxRate = Math.Round(targetBitrate * 1.01);
        var minRate = Math.Round(targetBitrate * 0.99);
        var buffer = Math.Round(targetBitrate * 0.8);
        
        var encoder = GetBestEncoder();
        var isHardware = encoder != "libx264";

        var args = new List<string>
        {
            "-c:v", encoder,
            "-b:v", $"{targetBitrate}k",
            "-maxrate", $"{maxRate}k",
            "-bufsize", $"{buffer}k"
        };
        
        if (isHardware)
        {
            // Hardware encoder settings - optimized for speed while maintaining quality
            args.AddRange(new[] { "-minrate", $"{minRate}k" });
            
            if (encoder == "h264_nvenc")
            {
                // NVENC: P4 preset balances speed and quality well, spatial AQ for quality
                args.AddRange(new[]
                {
                    "-preset", "p4", // medium preset
                    "-rc", "vbr",
                    "-spatial-aq", "1",
                    "-temporal-aq", "1",
                    "-rc-lookahead", "32", // lookahead helps with bitrate accuracy
                    "-g", "60",
                    "-bf", "3"
                });
            }
            else if (encoder == "h264_qsv")
            {
                // QuickSync: balanced preset, lookahead for better quality
                args.AddRange(new[]
                {
                    "-preset", "medium",
                    "-look_ahead", "1",
                    "-look_ahead_depth", "40",
                    "-g", "60",
                    "-bf", "3"
                });
            }
            else if (encoder == "h264_amf")
            {
                // AMD AMF: use CBR-style mode for accurate sizing
                // H.264 prioritizes speed by default (lighter AQ settings)
                maxRate = targetBitrate;
                minRate = targetBitrate;
                buffer = targetBitrate;

                args.AddRange(new[]
                {
                    "-quality", "quality",
                    "-rc", "cbr",
                    "-qmin", "0",
                    "-qmax", "51",
                    "-pix_fmt", "nv12"
                });

                // Default: speed-optimized (smaller GOP, less lookahead, less AQ)
                // Quality mode: can add more AQ if needed, but H.264 stays lean by design
                args.AddRange(new[]
                {
                    "-g", "60",
                    "-bf", "1",
                    "-rc-lookahead", "32",
                    "-temporal-aq", "1",
                    "-spatial-aq", "0"
                });

                args.AddRange(new[]
                {
                    "-b:v", $"{targetBitrate}k",
                    "-maxrate", $"{maxRate}k",
                    "-minrate", $"{minRate}k",
                    "-bufsize", $"{buffer}k"
                });
            }
        }
        else
        {
            // Software encoder: Use slower preset for highest quality
            args.AddRange(new[]
            {
                "-preset", "slower",
                "-pix_fmt", "yuv420p",
                "-g", "60",
                "-sc_threshold", "0",
                "-bf", "4",
                "-refs", "5",
                "-minrate", $"{minRate}k",
                // Maximum lookahead for best quality
                "-x264-params", "aq-mode=3:aq-strength=1.0:rc_lookahead=60:psy=1:psy_rd=1.0:deblock=-1,-1:me=umh:subme=10"
            });
        }

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

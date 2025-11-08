using System;
using System.Collections.Generic;
using System.Diagnostics;

namespace smart_compressor.CompressionStrategies;

public class H265Strategy : ICompressionStrategy
{
    private string? _detectedEncoder;
    private bool _encoderDetected = false;
    
    public string CodecKey => "h265";
    public string OutputExtension => ".mp4";
    public string MimeType => "video/mp4";
    public string VideoCodec => GetBestEncoder();
    public string AudioCodec => "aac";
    public int AudioBitrateKbps => 128;

    private string GetBestEncoder()
    {
        if (_encoderDetected)
            return _detectedEncoder ?? "libx265";
            
        _encoderDetected = true;
        
        // Try hardware encoders: NVENC > QuickSync > AMF > Software
        var encodersToTry = new[] { "hevc_nvenc", "hevc_qsv", "hevc_amf" };
        
        foreach (var encoder in encodersToTry)
        {
            if (IsEncoderAvailable(encoder))
            {
                _detectedEncoder = encoder;
                return encoder;
            }
        }
        
        _detectedEncoder = "libx265";
        return "libx265";
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

    public IEnumerable<string> BuildVideoArgs(double videoBitrateKbps)
    {
        var targetBitrate = Math.Max(100, Math.Round(videoBitrateKbps));
        // Tighter bitrate control to prevent file size overshoot
        var maxRate = Math.Round(targetBitrate * 1.01);
        var minRate = Math.Round(targetBitrate * 0.99);
        var buffer = Math.Round(targetBitrate * 0.8);
        
        var encoder = GetBestEncoder();
        var isHardware = encoder != "libx265";

        var args = new List<string>
        {
            "-c:v", encoder,
            "-b:v", $"{targetBitrate}k",
            "-maxrate", $"{maxRate}k",
            "-bufsize", $"{buffer}k"
        };
        
        if (isHardware)
        {
            args.AddRange(new[] { "-minrate", $"{minRate}k" });
            
            if (encoder == "hevc_nvenc")
            {
                // NVENC HEVC: P4 preset for balance
                args.AddRange(new[]
                {
                    "-preset", "p4",
                    "-rc", "vbr",
                    "-spatial-aq", "1",
                    "-temporal-aq", "1",
                    "-rc-lookahead", "32",
                    "-g", "60",
                    "-bf", "3",
                    "-tag:v", "hvc1"
                });
            }
            else if (encoder == "hevc_qsv")
            {
                args.AddRange(new[]
                {
                    "-preset", "medium",
                    "-look_ahead", "1",
                    "-look_ahead_depth", "40",
                    "-g", "60",
                    "-bf", "3",
                    "-tag:v", "hvc1"
                });
            }
            else if (encoder == "hevc_amf")
            {
                args.AddRange(new[]
                {
                    "-quality", "quality",
                    "-rc", "vbr_peak",
                    "-qmin", "18",
                    "-qmax", "51",
                    "-preanalysis", "1",
                    "-g", "60",
                    "-bf", "3",
                    "-tag:v", "hvc1"
                });
            }
        }
        else
        {
            // Software: Use slower preset for highest quality
            args.AddRange(new[]
            {
                "-preset", "slower",
                "-pix_fmt", "yuv420p",
                "-tag:v", "hvc1",
                "-g", "60",
                "-sc_threshold", "0",
                "-bf", "4",
                "-refs", "5",
                "-minrate", $"{minRate}k",
                // Maximum lookahead and psychovisual tuning for best quality
                "-x265-params", $"vbv-bufsize={buffer}:vbv-maxrate={maxRate}:aq-mode=3:aq-strength=1.0:psy-rd=2.0:psy-rdoq=1.0:rc-lookahead=60:me=star:subme=7:rd=6"
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
        // Use mp4 container for h265 passes
        return new[] { "-pass", passNumber.ToString(), "-passlogfile", passLogFile, "-f", "mp4" };
    }
}

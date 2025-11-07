using System;
using System.Collections.Generic;
using System.Diagnostics;

namespace smart_compressor.CompressionStrategies;

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
            // Actually test if encoder can initialize (not just if it's compiled in)
            // Use a minimal test encode to verify hardware is available
            var psi = new ProcessStartInfo
            {
                FileName = "ffmpeg",
                Arguments = $"-f lavfi -i color=black:s=64x64:d=0.1 -c:v {encoderName} -f null -",
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                UseShellExecute = false,
                CreateNoWindow = true
            };
            
            using var process = Process.Start(psi);
            if (process == null) return false;
            
            process.StandardOutput.ReadToEnd();
            var error = process.StandardError.ReadToEnd();
            process.WaitForExit();
            
            // Check for success and common failure messages
            return process.ExitCode == 0 && 
                   !error.Contains("Cannot load") && 
                   !error.Contains("not available") &&
                   !error.Contains("No NVENC") &&
                   !error.Contains("failed");
        }
        catch
        {
            return false;
        }
    }

    public IEnumerable<string> BuildVideoArgs(double videoBitrateKbps)
    {
        var targetBitrate = Math.Max(100, Math.Round(videoBitrateKbps));
        var maxRate = Math.Round(targetBitrate * 1.03);
        var minRate = Math.Round(targetBitrate * 0.97);
        var buffer = Math.Round(targetBitrate * 1.0);
        
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
                // AMD AMF: balanced preset
                args.AddRange(new[]
                {
                    "-quality", "balanced",
                    "-rc", "vbr_latency",
                    "-g", "60",
                    "-bf", "3"
                });
            }
        }
        else
        {
            // Software encoder: Use medium preset instead of slower for 2-3x speed improvement
            // Quality difference is minimal with constrained bitrate
            args.AddRange(new[]
            {
                "-preset", "medium",
                "-pix_fmt", "yuv420p",
                "-g", "60",
                "-sc_threshold", "0",
                "-bf", "4",
                "-refs", "4", // reduced from 5
                "-minrate", $"{minRate}k",
                // Reduced lookahead from 60 to 40 for faster encoding
                "-x264-params", "aq-mode=3:aq-strength=1.0:rc_lookahead=40:psy=1:psy_rd=1.0"
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

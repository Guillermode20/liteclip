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
            // Actually test if encoder can initialize (not just if it's compiled in)
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
                    "-quality", "balanced",
                    "-rc", "vbr_latency",
                    "-g", "60",
                    "-bf", "3",
                    "-tag:v", "hvc1"
                });
            }
        }
        else
        {
            // Software: Use medium preset for 2-3x speedup
            args.AddRange(new[]
            {
                "-preset", "medium",
                "-pix_fmt", "yuv420p",
                "-tag:v", "hvc1",
                "-g", "60",
                "-sc_threshold", "0",
                "-bf", "4",
                "-refs", "4",
                "-minrate", $"{minRate}k",
                // Reduced lookahead from 60 to 40
                "-x265-params", $"vbv-bufsize={buffer}:vbv-maxrate={maxRate}:aq-mode=3:aq-strength=1.0:psy-rd=2.0:rc-lookahead=40"
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

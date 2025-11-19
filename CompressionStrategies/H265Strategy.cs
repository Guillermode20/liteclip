using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Linq;
using liteclip.Services;

namespace liteclip.CompressionStrategies;

public class H265Strategy : ICompressionStrategy
{
    private readonly FfmpegCapabilityProbe? _probe;
    private string? _detectedEncoder;
    private bool _encoderDetected = false;

    public string CodecKey => "h265";
    public string OutputExtension => ".mp4";
    public string MimeType => "video/mp4";
    public string VideoCodec => GetBestEncoder();
    public string AudioCodec => "aac";
    public int AudioBitrateKbps => 128;

    public H265Strategy(FfmpegCapabilityProbe? probe = null)
    {
        _probe = probe;
    }

    private string GetBestEncoder()
    {
        if (_encoderDetected)
            return _detectedEncoder ?? "libx265";

        _encoderDetected = true;

        // PRIORITY: Hardware Encoders
        // We strictly prefer hardware for speed. 
        var encodersToTry = new[] 
        { 
            "hevc_nvenc",       // NVIDIA (Best balance of speed/quality)
            "hevc_qsv",         // Intel QuickSync (Excellent speed)
            "hevc_videotoolbox",// MacOS Apple Silicon (Fast)
            "hevc_amf",         // AMD (Fast, requires careful tuning)
            "hevc_vaapi"        // Linux Generic
        };

        foreach (var encoder in encodersToTry)
        {
            // Check probe cache first
            if (_probe != null && _probe.SupportedEncoders.Contains(encoder))
            {
                _detectedEncoder = encoder;
                return encoder;
            }

            // Runtime check fallback
            if (IsEncoderAvailable(encoder))
            {
                _detectedEncoder = encoder;
                return encoder;
            }
        }

        // Fallback to software (faster preset) only if NO hardware is found
        _detectedEncoder = "libx265";
        return "libx265";
    }

    private static bool IsEncoderAvailable(string encoderName)
    {
        try
        {
            // Simple color test to verify encoder init
            var args = $"-f lavfi -i color=black:s=64x64:d=0.1 -c:v {encoderName} -f null -";
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
            if (process == null) return false;
            process.WaitForExit();
            return process.ExitCode == 0;
        }
        catch
        {
            return false;
        }
    }

    public IEnumerable<string> BuildVideoArgs(double videoBitrateKbps, EncodingMode mode)
    {
        var encoder = GetBestEncoder();
        var targetBitrate = Math.Max(100, Math.Round(videoBitrateKbps));
        
        var args = new List<string>
        {
            "-c:v", encoder,
            "-b:v", $"{targetBitrate}k",
            "-pix_fmt", "yuv420p", // Maximum compatibility
            "-movflags", "+faststart"
        };

        // Strict Bitrate Control (Hardware encoders need strict limits to not overshoot size)
        // We allow a tight 5-10% maxrate swing to handle motion, but rely on buffer to average it out.
        var maxRate = Math.Round(targetBitrate * 1.10); 
        var bufSize = Math.Round(targetBitrate * 2.0); 

        args.Add("-maxrate");
        args.Add($"{maxRate}k");
        args.Add("-bufsize");
        args.Add($"{bufSize}k");

        // Apply "Brutal" Quality Settings per Hardware Vendor
        if (encoder.Contains("nvenc"))
        {
            ApplyNvencSettings(args);
        }
        else if (encoder.Contains("qsv"))
        {
            ApplyQsvSettings(args);
        }
        else if (encoder.Contains("amf"))
        {
            ApplyAmfSettings(args);
        }
        else if (encoder.Contains("videotoolbox"))
        {
            args.Add("-preset");
            args.Add("quality"); 
        }
        else
        {
            // Software Fallback: Use 'faster' because the user demanded speed
            args.Add("-preset");
            args.Add("faster");
            args.Add("-x265-params");
            args.Add($"vbv-maxrate={maxRate}:vbv-bufsize={bufSize}:aq-mode=3");
        }

        return args;
    }

    private void ApplyNvencSettings(List<string> args)
    {
        // NVENC "Brutal" Quality Settings
        // P7 is the slowest NVENC preset, but it is still insanely fast compared to CPU.
        args.Add("-preset");
        args.Add("p7"); 

        // High Quality Variable Bitrate
        args.Add("-rc");
        args.Add("vbr_hq"); 

        // Spatial AQ: Allocates more bits to complex textures (grass, water)
        args.Add("-spatial-aq");
        args.Add("1");

        // Temporal AQ: improves perceptual quality over time
        args.Add("-temporal-aq");
        args.Add("1");

        // Lookahead: Allows encoder to see 32 frames ahead to plan bitrate allocation
        args.Add("-rc-lookahead");
        args.Add("32");

        // Tier High allows for better peak bitrate handling if needed
        args.Add("-tier");
        args.Add("high");
    }

    private void ApplyQsvSettings(List<string> args)
    {
        // Intel QuickSync Quality Settings
        args.Add("-load_plugin");
        args.Add("hevc_hw");

        args.Add("-preset");
        args.Add("veryslow"); // QSV 'veryslow' is still very fast

        // Enable Hardware Lookahead
        args.Add("-look_ahead");
        args.Add("1");
        
        args.Add("-look_ahead_depth");
        args.Add("40");
    }

    private void ApplyAmfSettings(List<string> args)
    {
        // AMD AMF Quality Settings
        args.Add("-quality");
        args.Add("quality");

        args.Add("-rc");
        args.Add("vbr_peak"); // VBR with Peak Constraint

        // Enable Variance Based Adaptive Quantization (if supported)
        // Note: AMF flags can be driver specific, but this is standard for ffmpeg-amf
        args.Add("-usage");
        args.Add("transcoding"); 
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
        return new[] { "-pass", passNumber.ToString(), "-passlogfile", passLogFile, "-f", "mp4" };
    }
}
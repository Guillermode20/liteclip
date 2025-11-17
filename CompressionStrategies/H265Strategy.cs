using System;
using System.Collections.Generic;
using System.Diagnostics;

namespace liteclip.CompressionStrategies;

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
        
        // For small or tightly-targeted outputs we prefer libx265 (two-pass) for accuracy.
        // The selection of hardware vs software is made at a higher level based on target size.
        // Here we only detect hardware for general use.

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

    public IEnumerable<string> BuildVideoArgs(double videoBitrateKbps, bool useQualityMode)
    {
        var targetBitrate = Math.Max(100, Math.Round(videoBitrateKbps));
        // Base CBR-ish defaults; strategies can override per encoder
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
            args.AddRange(new[] { "-pix_fmt", "yuv420p" });
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
                // H.265 defaults to quality-focused tuning (good mix of speed + quality)
                // This is the primary codec recommendation and can afford slower encoding
                maxRate = targetBitrate;
                minRate = targetBitrate;
                buffer = targetBitrate;

                args.AddRange(new[]
                {
                    "-quality", "quality",
                    "-rc", "cbr",
                    "-qmin", "0",
                    "-qmax", "51",
                    "-tag:v", "hvc1"
                });

                // Default: quality-focused (larger GOP, high lookahead, adaptive AQ)
                args.AddRange(new[]
                {
                    "-g", "120",
                    "-bf", "2",
                    "-rc-lookahead", "64",
                    "-temporal-aq", "2",
                    "-spatial-aq", "2",
                    "-profile:v", "main",
                    "-no-scenecut", "1"
                });

                // Quality mode OFF (unlikely): scale back to faster, lighter settings
                if (!useQualityMode)
                {
                    // Remove the quality-focused args and add speed-optimized ones
                    args.RemoveRange(args.Count - 8, 8);
                    args.AddRange(new[]
                    {
                        "-g", "60",
                        "-bf", "1",
                        "-rc-lookahead", "32",
                        "-temporal-aq", "1",
                        "-spatial-aq", "0",
                        "-profile:v", "main",
                        "-no-scenecut", "0"
                    });
                }

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
            // Software: Use medium preset for good quality/speed balance
            // Quality-focused codec deserves more processing time than H.264
            args.AddRange(new[]
            {
                "-preset", "medium",
                "-pix_fmt", "yuv420p",
                "-tag:v", "hvc1",
                "-g", "60",
                "-sc_threshold", "0",
                "-bf", "3",
                "-refs", "4",
                "-minrate", $"{minRate}k",
                // Balanced quality settings: good psychovisual optimization with reasonable lookahead
                "-x265-params", $"vbv-bufsize={buffer}:vbv-maxrate={maxRate}:aq-mode=3:aq-strength=0.9:psy-rd=1.5:psy-rdoq=0.8:rc-lookahead=40:me=star:subme=5:rd=4"
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

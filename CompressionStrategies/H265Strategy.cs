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

    public IEnumerable<string> BuildVideoArgs(double videoBitrateKbps, bool useQualityMode, bool useUltraMode = false)
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
                // NVENC HEVC: Higher preset for ultra mode, maximum quality settings
                args.AddRange(new[]
                {
                    "-preset", useUltraMode ? "p7" : "p6",
                    "-rc", "vbr",
                    "-spatial-aq", "1",
                    "-temporal-aq", "1",
                    "-rc-lookahead", useUltraMode ? "64" : "48",
                    "-g", "60",
                    "-bf", useUltraMode ? "5" : "4",
                    "-b_ref_mode", useUltraMode ? "each" : "middle",
                    "-multipass", useUltraMode ? "fullres" : "disabled",
                    "-tag:v", "hvc1"
                });
            }
            else if (encoder == "hevc_qsv")
            {
                args.AddRange(new[]
                {
                    "-preset", useUltraMode ? "veryslow" : "slower",
                    "-look_ahead", "1",
                    "-look_ahead_depth", useUltraMode ? "80" : "60",
                    "-g", "60",
                    "-bf", useUltraMode ? "5" : "4",
                    "-tag:v", "hvc1"
                });
            }
            else if (encoder == "hevc_amf")
            {
                // H.265 defaults to quality-focused tuning (good mix of speed + quality)
                // This is the primary codec recommendation and can afford slower encoding
                maxRate = Math.Round(targetBitrate * 1.05);
                minRate = Math.Round(targetBitrate * 0.95);
                buffer = Math.Round(targetBitrate * 1.5);

                args.AddRange(new[]
                {
                    "-quality", "quality",
                    "-rc", "vbr_peak",
                    "-qmin", "15",
                    "-qmax", "45",
                    "-tag:v", "hvc1"
                });

                // Ultra mode: Maximum quality settings with extended lookahead and more B-frames
                // Quality mode: Enhanced quality settings
                // Default: quality-focused (larger GOP, high lookahead, more B-frames, adaptive AQ)
                if (useUltraMode)
                {
                    args.AddRange(new[]
                    {
                        "-g", "120",
                        "-bf", "6",
                        "-rc-lookahead", "120",
                        "-temporal-aq", "2",
                        "-spatial-aq", "2",
                        "-profile:v", "main",
                        "-no-scenecut", "1",
                        "-preanalysis", "1"
                    });
                }
                else
                {
                    args.AddRange(new[]
                    {
                        "-g", "120",
                        "-bf", "4",
                        "-rc-lookahead", "80",
                        "-temporal-aq", "2",
                        "-spatial-aq", "2",
                        "-profile:v", "main",
                        "-no-scenecut", "1"
                    });
                }

                // Quality mode OFF (unlikely): scale back to faster, lighter settings
                if (!useQualityMode && !useUltraMode)
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
            // Software encoder (libx265)
            if (useUltraMode)
            {
                // Ultra quality mode: veryslow preset with maximum quality settings
                // This will take significantly longer but produce the best possible quality
                args.AddRange(new[]
                {
                    "-preset", "veryslow",
                    "-pix_fmt", "yuv420p",
                    "-tag:v", "hvc1",
                    "-g", "120",
                    "-sc_threshold", "0",
                    "-bf", "8",
                    "-refs", "6",
                    "-minrate", $"{minRate}k",
                    // Maximum quality settings: extreme psychovisual optimization, maximum lookahead, best motion estimation
                    "-x265-params", $"vbv-bufsize={buffer}:vbv-maxrate={maxRate}:aq-mode=3:aq-strength=1.8:psy-rd=3.0:psy-rdoq=2.5:rc-lookahead=120:me=star:subme=11:rd=6:ref=6:bframes=8:b-adapt=2:ctu=64:max-tu-size=32:rdoq-level=2:tu-intra-depth=4:tu-inter-depth=4:limit-modes=0:limit-refs=0:limit-tu=0:early-skip=0:rskip=0:rskip-edge-threshold=0:tskip=1:tskip-fast=0:strong-intra-smoothing=1:constrained-intra=0:fast-intra=0:b-intra=1:cu-lossless=0:signhide=1:weightp=1:weightb=1:analyze-src-pics=1:deblock=-2,-2:no-sao=0:selective-sao=4:pmode=1:pmode=1"
                });
            }
            else
            {
                // Quality mode: slower preset with enhanced quality settings for better visual fidelity
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
                    // Enhanced quality settings: stronger psychovisual optimization, better motion estimation, improved adaptive quantization
                    "-x265-params", $"vbv-bufsize={buffer}:vbv-maxrate={maxRate}:aq-mode=3:aq-strength=1.4:psy-rd=2.5:psy-rdoq=1.5:rc-lookahead=80:me=star:subme=10:rd=6:ref=6:sao=1:deblock=-1,-1:rdoq-level=2:ctu=32:tu-intra-depth=3:tu-inter-depth=3"
                });
            }
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

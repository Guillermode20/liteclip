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

    public IEnumerable<string> BuildVideoArgs(double videoBitrateKbps, bool useQualityMode, bool useUltraMode = false)
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
                // NVENC: Higher preset for better quality when quality mode is enabled
                args.AddRange(new[]
                {
                    "-preset", (useQualityMode || useUltraMode) ? "p6" : "p4",
                    "-rc", "vbr",
                    "-spatial-aq", "1",
                    "-temporal-aq", "1",
                    "-rc-lookahead", (useQualityMode || useUltraMode) ? "48" : "32",
                    "-g", "60",
                    "-bf", (useQualityMode || useUltraMode) ? "4" : "3"
                });
                
                if (useQualityMode || useUltraMode)
                {
                    args.AddRange(new[] { "-b_ref_mode", "middle" });
                }
            }
            else if (encoder == "h264_qsv")
            {
                // QuickSync: slower preset for quality mode
                args.AddRange(new[]
                {
                    "-preset", (useQualityMode || useUltraMode) ? "slower" : "medium",
                    "-look_ahead", "1",
                    "-look_ahead_depth", (useQualityMode || useUltraMode) ? "60" : "40",
                    "-g", "60",
                    "-bf", (useQualityMode || useUltraMode) ? "4" : "3"
                });
            }
            else if (encoder == "h264_amf")
            {
                // AMD AMF: Better bitrate control for quality mode
                if (useQualityMode || useUltraMode)
                {
                    maxRate = Math.Round(targetBitrate * 1.05);
                    minRate = Math.Round(targetBitrate * 0.95);
                    buffer = Math.Round(targetBitrate * 1.5);
                }
                else
                {
                    maxRate = targetBitrate;
                    minRate = targetBitrate;
                    buffer = targetBitrate;
                }

                args.AddRange(new[]
                {
                    "-quality", "quality",
                    "-rc", (useQualityMode || useUltraMode) ? "vbr_peak" : "cbr",
                    "-qmin", (useQualityMode || useUltraMode) ? "18" : "0",
                    "-qmax", (useQualityMode || useUltraMode) ? "45" : "51",
                    "-pix_fmt", "nv12"
                });

                // Quality mode: Enhanced AQ and lookahead for better visual quality
                // Speed mode: Lighter settings for faster encoding
                if (useQualityMode || useUltraMode)
                {
                    args.AddRange(new[]
                    {
                        "-g", "60",
                        "-bf", "3",
                        "-rc-lookahead", "64",
                        "-temporal-aq", "2",
                        "-spatial-aq", "2"
                    });
                }
                else
                {
                    args.AddRange(new[]
                    {
                        "-g", "60",
                        "-bf", "1",
                        "-rc-lookahead", "32",
                        "-temporal-aq", "1",
                        "-spatial-aq", "0"
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
            // Software encoder: Adjust preset based on quality mode
            if (useQualityMode || useUltraMode)
            {
                // Quality/Ultra mode: Use much slower preset for ultra mode
                args.AddRange(new[]
                {
                    "-preset", useUltraMode ? "veryslow" : "medium",
                    "-pix_fmt", "yuv420p",
                    "-g", "60",
                    "-sc_threshold", "0",
                    "-bf", useUltraMode ? "5" : "3",
                    "-refs", useUltraMode ? "6" : "4",
                    "-minrate", $"{minRate}k",
                    // Ultra mode: Maximum quality settings with extreme psychovisual optimization
                    // Quality mode: Enhanced quality settings
                    "-x264-params", useUltraMode 
                        ? "aq-mode=3:aq-strength=1.5:rc_lookahead=120:psy=1:psy-rd=1.5:me=umh:subme=11:ref=6:deblock=-2,-2:mbtree=1:trellis=2:fast-pskip=0:no-fast-pskip=1:no-dct-decimate=1:no-mbtree=0:direct=auto:weightb=1:weightp=2:aq-strength=1.5:aq-sensitivity=10:aq-bias-strength=1.0:aq-bias=0:mixed-refs=1:8x8dct=1:fast-pskip=0:no-fast-pskip=1:no-dct-decimate=1"
                        : "aq-mode=3:aq-strength=1.0:rc_lookahead=50:psy=1:psy-rd=1.0:me=umh:subme=8:ref=4:mbtree=1"
                });
            }
            else
            {
                // Fast mode: Use fast preset for speed while maintaining decent quality
                args.AddRange(new[]
                {
                    "-preset", "fast",
                    "-pix_fmt", "yuv420p",
                    "-g", "60",
                    "-sc_threshold", "0",
                    "-bf", "2",
                    "-refs", "2",
                    "-minrate", $"{minRate}k",
                    // Balanced settings: speed-focused with some quality preservation
                    "-x264-params", "aq-mode=2:aq-strength=0.8:rc_lookahead=30:psy=0:me=hex:subme=6"
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
        // Use mp4 container for passes
        return new[] { "-pass", passNumber.ToString(), "-passlogfile", passLogFile, "-f", "mp4" };
    }
}

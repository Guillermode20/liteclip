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

    public IEnumerable<string> BuildVideoArgs(double videoBitrateKbps, EncodingMode mode)
    {
        var targetBitrate = Math.Max(100, Math.Round(videoBitrateKbps));

        var encoder = GetBestEncoder();
        var config = EncodingModeConfigs.Get(CodecKey, encoder, mode);

        var maxRate = Math.Round(targetBitrate * config.MaxRateMultiplier);
        var minRate = Math.Round(targetBitrate * config.MinRateMultiplier);
        var buffer = Math.Round(targetBitrate * config.BufferMultiplier);
        
        var args = new List<string>
        {
            "-c:v", encoder,
            "-b:v", $"{targetBitrate}k"
        };

        // Apply encoder/mode-specific tuning first.
        foreach (var token in config.VideoArgs)
        {
            if (token == "{maxrate}")
            {
                args.Add($"{maxRate}k");
            }
            else if (token == "{minrate}")
            {
                args.Add($"{minRate}k");
            }
            else if (token == "{buffer}")
            {
                args.Add($"{buffer}k");
            }
            else if (token == "{target}")
            {
                args.Add($"{targetBitrate}k");
            }
            else if (token.Contains("{maxrate}", StringComparison.Ordinal) ||
                     token.Contains("{buffer}", StringComparison.Ordinal))
            {
                // Handle tokens used inside x265 param strings.
                var replaced = token
                    .Replace("{maxrate}", maxRate.ToString(), StringComparison.Ordinal)
                    .Replace("{buffer}", buffer.ToString(), StringComparison.Ordinal);
                args.Add(replaced);
            }
            else
            {
                args.Add(token);
            }
        }

        // Attach standard bitrate constraints unless already provided.
        if (!args.Contains("-maxrate"))
        {
            args.AddRange(new[] { "-maxrate", $"{maxRate}k" });
        }
        if (!args.Contains("-bufsize"))
        {
            args.AddRange(new[] { "-bufsize", $"{buffer}k" });
        }
        if (!args.Contains("-minrate"))
        {
            args.AddRange(new[] { "-minrate", $"{minRate}k" });
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

using System.Diagnostics;
using System.Text.RegularExpressions;
using System.Collections.Generic;

namespace liteclip.Services;

public class FfmpegCapabilityProbe
{
    private readonly ILogger<FfmpegCapabilityProbe> _logger;
    private readonly FfmpegPathResolver _resolver;

    public HashSet<string> SupportedEncoders { get; } = new(StringComparer.OrdinalIgnoreCase);
    public int? MaxX265Subme { get; private set; }

    public FfmpegCapabilityProbe(ILogger<FfmpegCapabilityProbe> logger, FfmpegPathResolver resolver)
    {
        _logger = logger;
        _resolver = resolver;
    }

    public async Task ProbeAsync()
    {
        try
        {
            var ffmpeg = _resolver.GetFfmpegPath();
            _logger.LogInformation("Probing FFmpeg capabilities using: {Path}", ffmpeg);

            // Check common encoders of interest
            var encodersToCheck = new[] {
                "libx265", "libx264", "libvpx-vp9", "libaom-av1", "hevc_nvenc", "h264_nvenc", "hevc_qsv", "h264_qsv", "hevc_amf", "h264_amf"
            };

            foreach (var enc in encodersToCheck)
            {
                if (await IsEncoderAvailableAsync(ffmpeg, enc))
                {
                    SupportedEncoders.Add(enc);
                    _logger.LogInformation("Detected encoder: {Encoder}", enc);
                }
            }

            // Probe libx265 "subme" support by trying a highest candidate and stepping down
            if (SupportedEncoders.Contains("libx265"))
            {
                // test subme values from 10 down to 0 and pick the highest that succeeds
                int detected = -1;
                for (int test = 10; test >= 0; test--)
                {
                    if (await TestX265Subme(ffmpeg, test))
                    {
                        detected = test;
                        break;
                    }
                }

                if (detected >= 0)
                {
                    MaxX265Subme = detected;
                    _logger.LogInformation("libx265 max supported subme inferred as {Max}", detected);
                }
                else
                {
                    _logger.LogWarning("Unable to probe libx265 'subme' max value - will rely on runtime sanitization");
                }
            }
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "Error while probing ffmpeg capabilities — continuing without probe results");
        }
    }

    private async Task<bool> IsEncoderAvailableAsync(string ffmpegPath, string encoderKey)
    {
        try
        {
            // We use a minimal test like other strategies - a tiny lavfi input
            var args = $"-hide_banner -loglevel error -f lavfi -i color=black:s=64x64:d=0.15 -c:v {encoderKey} -f null -";

            var psi = new ProcessStartInfo
            {
                FileName = ffmpegPath,
                Arguments = args,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                UseShellExecute = false,
                CreateNoWindow = true
            };

            using var p = Process.Start(psi);
            if (p == null) return false;

            var err = await p.StandardError.ReadToEndAsync();
            var outp = await p.StandardOutput.ReadToEndAsync();
            await p.WaitForExitAsync();

            if (p.ExitCode == 0) return true;

            var low = (err ?? string.Empty).ToLowerInvariant();
            if (low.Contains("not available") || low.Contains("cannot load") || low.Contains("no nvenc") )
            {
                return false;
            }

            return false;
        }
        catch
        {
            return false;
        }
    }

    private async Task<bool> TestX265Subme(string ffmpegPath, int subme)
    {
        try
        {
            // We test whether libx265 accepts a particular 'subme' value
            var args = $"-hide_banner -loglevel error -f lavfi -i color=black:s=64x64:d=0.2 -c:v libx265 -x265-params subme={subme} -f null -";

            var psi = new ProcessStartInfo
            {
                FileName = ffmpegPath,
                Arguments = args,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                UseShellExecute = false,
                CreateNoWindow = true
            };

            using var p = Process.Start(psi);
            if (p == null) return false;

            var err = await p.StandardError.ReadToEndAsync();
            var outp = await p.StandardOutput.ReadToEndAsync();
            await p.WaitForExitAsync();

            if (p.ExitCode == 0)
            {
                return true;
            }

            // If ffmpeg stderr contains 'subme must be less' we can parse the limit too
            if (!string.IsNullOrEmpty(err))
            {
                var m = Regex.Match(err, @"subme must be less than or equal to X265_MAX_SUBPEL_LEVEL \((\d+)\)", RegexOptions.IgnoreCase);
                if (m.Success && int.TryParse(m.Groups[1].Value, out var limit))
                {
                    _logger.LogInformation("Probe: libx265 reported max subme {Limit}", limit);
                    return limit >= subme;
                }
            }

            return false;
        }
        catch
        {
            return false;
        }
    }
}

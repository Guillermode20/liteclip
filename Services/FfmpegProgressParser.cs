using System;
using System.Globalization;
using System.Text.RegularExpressions;

namespace liteclip.Services;

public interface IProgressParser
{
    FfmpegProgressUpdate? TryParse(string line, double? totalDuration);
}

public class FfmpegProgressParser : IProgressParser
{
    // Support both classic ffmpeg stderr stats (time=HH:MM:SS.xx) and
    // the -progress protocol (out_time=HH:MM:SS.xx).
    private static readonly Regex TimeRegex = new(@"time=(\d{2}):(\d{2}):(\d{2}(?:\.\d+)?)", RegexOptions.Compiled);
    private static readonly Regex OutTimeRegex = new(@"out_time=(\d{2}):(\d{2}):(\d{2}(?:\.\d+)?)", RegexOptions.Compiled);

    public FfmpegProgressUpdate? TryParse(string line, double? totalDuration)
    {
        if (string.IsNullOrEmpty(line) || !totalDuration.HasValue || totalDuration.Value <= 0)
        {
            return null;
        }

        try
        {
            // Prefer classic time= first, then fall back to out_time=
            var match = TimeRegex.Match(line);
            if (!match.Success)
            {
                match = OutTimeRegex.Match(line);
            }

            if (!match.Success)
            {
                return null;
            }

            var hours = double.Parse(match.Groups[1].Value, CultureInfo.InvariantCulture);
            var minutes = double.Parse(match.Groups[2].Value, CultureInfo.InvariantCulture);
            var seconds = double.Parse(match.Groups[3].Value, CultureInfo.InvariantCulture);

            var currentTime = hours * 3600 + minutes * 60 + seconds;
            var progress = (currentTime / totalDuration.Value) * 100.0;
            var clamped = Math.Clamp(progress, 0.0, 100.0);

            return new FfmpegProgressUpdate(clamped, currentTime, null);
        }
        catch
        {
            return null;
        }
    }
}

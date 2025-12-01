using System.Globalization;

namespace liteclip.Services;

public static class AdaptiveFilterBuilder
{
    private enum CompressionIntensity
    {
        Light,
        Moderate,
        Heavy
    }

    public static List<string> Build(int scalePercent, int targetFps, double? targetSizeMb, double? sourceDuration)
    {
        var filters = new List<string>();

        var intensity = DetermineIntensity(targetSizeMb, sourceDuration);

        // 1. Temporal denoising - ALWAYS apply before scaling
        var (lumSpat, chromaSpat, lumTemp, chromaTemp) = intensity switch
        {
            CompressionIntensity.Heavy => (2.8, 2.3, 4.5, 4.5),
            CompressionIntensity.Moderate => (1.7, 1.2, 3.2, 3.2),
            _ => (1.0, 0.8, 2.2, 2.2)
        };

        filters.Add($"hqdn3d={lumSpat.ToString(CultureInfo.InvariantCulture)}:{chromaSpat.ToString(CultureInfo.InvariantCulture)}:{lumTemp.ToString(CultureInfo.InvariantCulture)}:{chromaTemp.ToString(CultureInfo.InvariantCulture)}");

        // 2. Scaling (if needed)
        if (scalePercent < 100)
        {
            var scaleFactor = Math.Clamp(scalePercent, 10, 100) / 100.0;
            filters.Add($"scale=trunc(iw*{scaleFactor.ToString(CultureInfo.InvariantCulture)}/2)*2:trunc(ih*{scaleFactor.ToString(CultureInfo.InvariantCulture)}/2)*2");
        }

        // 3. Debanding - ALWAYS apply after scaling
        var debandThreshold = intensity switch
        {
            CompressionIntensity.Heavy => 0.035,
            CompressionIntensity.Moderate => 0.022,
            _ => 0.015
        };
        var debandRange = intensity == CompressionIntensity.Heavy ? 18 : 14;
        var thresholdStr = debandThreshold.ToString(CultureInfo.InvariantCulture);
        filters.Add($"deband=1thr={thresholdStr}:2thr={thresholdStr}:3thr={thresholdStr}:range={debandRange}:blur=0");

        // 4. Contrast-adaptive sharpening - ALWAYS apply
        if (scalePercent < 100)
        {
            var downscaleFactor = 1.0 - (scalePercent / 100.0);
            var baseStrength = intensity == CompressionIntensity.Heavy ? 0.45 : 0.35;
            var unsharpStrength = Math.Round(baseStrength + (downscaleFactor * 1.5), 2);
            filters.Add($"unsharp=3:3:{unsharpStrength.ToString(CultureInfo.InvariantCulture)}");
        }
        else
        {
            var defaultStrength = intensity switch
            {
                CompressionIntensity.Heavy => 0.4,
                CompressionIntensity.Moderate => 0.32,
                _ => 0.25
            };
            filters.Add($"unsharp=3:3:{defaultStrength.ToString(CultureInfo.InvariantCulture)}");
        }

        // 5. FPS limiting (if specified)
        if (targetFps > 0)
        {
            filters.Add($"fps={targetFps}");
        }

        return filters;
    }

    private static CompressionIntensity DetermineIntensity(double? targetSizeMb, double? sourceDuration)
    {
        if (!targetSizeMb.HasValue || !sourceDuration.HasValue || sourceDuration.Value <= 0)
        {
            return CompressionIntensity.Light;
        }

        var bitrateKbps = (targetSizeMb.Value * 8 * 1024) / sourceDuration.Value;
        return bitrateKbps switch
        {
            < 900 => CompressionIntensity.Heavy,
            < 2000 => CompressionIntensity.Moderate,
            _ => CompressionIntensity.Light
        };
    }
}

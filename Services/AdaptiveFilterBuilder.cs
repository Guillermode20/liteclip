using System.Globalization;

namespace liteclip.Services;

/// <summary>
/// Options to control which filters are applied during encoding.
/// </summary>
public sealed class FilterOptions
{
    /// <summary>Apply HQDN3D temporal denoising (CPU-intensive).</summary>
    public bool EnableDenoising { get; init; } = true;
    
    /// <summary>Apply debanding filter (CPU-intensive).</summary>
    public bool EnableDebanding { get; init; } = true;
    
    /// <summary>Apply unsharp/sharpening filter.</summary>
    public bool EnableSharpening { get; init; } = true;
    
    /// <summary>Apply scaling filter when scalePercent &lt; 100.</summary>
    public bool EnableScaling { get; init; } = true;
    
    /// <summary>Apply FPS limiting filter.</summary>
    public bool EnableFpsLimit { get; init; } = true;
    
    /// <summary>
    /// Creates filter options appropriate for the given compression scenario.
    /// Skips expensive filters when bitrate budget is high.
    /// </summary>
    public static FilterOptions ForCompression(double? targetSizeMb, double? sourceDuration, bool isSimpleTrim = false)
    {
        // If this is just a trim with no compression, skip all expensive filters
        if (isSimpleTrim)
        {
            return new FilterOptions
            {
                EnableDenoising = false,
                EnableDebanding = false,
                EnableSharpening = false,
                EnableScaling = false,
                EnableFpsLimit = false
            };
        }
        
        // Calculate bitrate to determine if we need expensive filters
        if (!targetSizeMb.HasValue || !sourceDuration.HasValue || sourceDuration.Value <= 0)
        {
            // No target size info - use light filters only
            return new FilterOptions
            {
                EnableDenoising = false,
                EnableDebanding = false,
                EnableSharpening = true,
                EnableScaling = true,
                EnableFpsLimit = true
            };
        }
        
        var bitrateKbps = (targetSizeMb.Value * 8 * 1024) / sourceDuration.Value;
        
        // High bitrate (>3000 kbps) - skip expensive denoising/debanding
        if (bitrateKbps > 3000)
        {
            return new FilterOptions
            {
                EnableDenoising = false,
                EnableDebanding = false,
                EnableSharpening = true,
                EnableScaling = true,
                EnableFpsLimit = true
            };
        }
        
        // Medium-high bitrate (>2000 kbps) - skip denoising but keep debanding
        if (bitrateKbps > 2000)
        {
            return new FilterOptions
            {
                EnableDenoising = false,
                EnableDebanding = true,
                EnableSharpening = true,
                EnableScaling = true,
                EnableFpsLimit = true
            };
        }
        
        // Low bitrate - enable all filters for best quality
        return new FilterOptions
        {
            EnableDenoising = true,
            EnableDebanding = true,
            EnableSharpening = true,
            EnableScaling = true,
            EnableFpsLimit = true
        };
    }
}

public static class AdaptiveFilterBuilder
{
    private enum CompressionIntensity
    {
        Light,
        Moderate,
        Heavy
    }

    /// <summary>
    /// Builds the filter chain with all filters enabled (legacy behavior).
    /// </summary>
    public static List<string> Build(int scalePercent, int targetFps, double? targetSizeMb, double? sourceDuration)
    {
        var options = FilterOptions.ForCompression(targetSizeMb, sourceDuration);
        return Build(scalePercent, targetFps, targetSizeMb, sourceDuration, options);
    }
    
    /// <summary>
    /// Builds the filter chain with configurable filter options.
    /// </summary>
    public static List<string> Build(int scalePercent, int targetFps, double? targetSizeMb, double? sourceDuration, FilterOptions options)
    {
        var filters = new List<string>(5); // Pre-allocate for typical filter count

        var intensity = DetermineIntensity(targetSizeMb, sourceDuration);

        // 1. Temporal denoising - apply before scaling (CPU-intensive, skip for high bitrate)
        if (options.EnableDenoising)
        {
            var (lumSpat, chromaSpat, lumTemp, chromaTemp) = intensity switch
            {
                CompressionIntensity.Heavy => (2.8, 2.3, 4.5, 4.5),
                CompressionIntensity.Moderate => (1.7, 1.2, 3.2, 3.2),
                _ => (1.0, 0.8, 2.2, 2.2)
            };

            filters.Add(string.Create(CultureInfo.InvariantCulture, $"hqdn3d={lumSpat}:{chromaSpat}:{lumTemp}:{chromaTemp}"));
        }

        // 2. Scaling (if needed)
        if (options.EnableScaling && scalePercent < 100)
        {
            var scaleFactor = Math.Clamp(scalePercent, 10, 100) / 100.0;
            filters.Add(string.Create(CultureInfo.InvariantCulture, $"scale=trunc(iw*{scaleFactor}/2)*2:trunc(ih*{scaleFactor}/2)*2"));
        }

        // 3. Debanding - apply after scaling (CPU-intensive, skip for high bitrate)
        if (options.EnableDebanding)
        {
            var debandThreshold = intensity switch
            {
                CompressionIntensity.Heavy => 0.035,
                CompressionIntensity.Moderate => 0.022,
                _ => 0.015
            };
            var debandRange = intensity == CompressionIntensity.Heavy ? 18 : 14;
            filters.Add(string.Create(CultureInfo.InvariantCulture, $"deband=1thr={debandThreshold}:2thr={debandThreshold}:3thr={debandThreshold}:range={debandRange}:blur=0"));
        }

        // 4. Contrast-adaptive sharpening
        if (options.EnableSharpening)
        {
            if (scalePercent < 100)
            {
                var downscaleFactor = 1.0 - (scalePercent / 100.0);
                var baseStrength = intensity == CompressionIntensity.Heavy ? 0.45 : 0.35;
                var unsharpStrength = Math.Round(baseStrength + (downscaleFactor * 1.5), 2);
                filters.Add(string.Create(CultureInfo.InvariantCulture, $"unsharp=3:3:{unsharpStrength}"));
            }
            else
            {
                var defaultStrength = intensity switch
                {
                    CompressionIntensity.Heavy => 0.4,
                    CompressionIntensity.Moderate => 0.32,
                    _ => 0.25
                };
                filters.Add(string.Create(CultureInfo.InvariantCulture, $"unsharp=3:3:{defaultStrength}"));
            }
        }

        // 5. FPS limiting (if specified)
        if (options.EnableFpsLimit && targetFps > 0)
        {
            filters.Add(string.Create(CultureInfo.InvariantCulture, $"fps={targetFps}"));
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

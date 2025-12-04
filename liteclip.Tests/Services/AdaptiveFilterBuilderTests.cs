using liteclip.Services;
using Xunit;

namespace liteclip.Tests.Services;

public class AdaptiveFilterBuilderTests
{
    [Fact]
    public void Build_HighBitrate_SkipsExpensiveFilters()
    {
        // High bitrate (200MB for 60s = ~27,000 kbps) should skip denoising and debanding
        var filters = AdaptiveFilterBuilder.Build(scalePercent: 100, targetFps: 30, targetSizeMb: 200, sourceDuration: 60);

        // Only sharpening and fps filters should be applied for high bitrate
        Assert.Equal(2, filters.Count);
        Assert.Equal("unsharp=3:3:0.25", filters[0]);
        Assert.Equal("fps=30", filters[1]);
    }
    
    [Fact]
    public void Build_LowBitrate_AppliesAllFilters()
    {
        // Low bitrate scenario - should apply all filters including denoising and debanding
        var filters = AdaptiveFilterBuilder.Build(scalePercent: 100, targetFps: 30, targetSizeMb: 5, sourceDuration: 60);

        Assert.Equal(4, filters.Count);
        Assert.Equal("hqdn3d=2.8:2.3:4.5:4.5", filters[0]); // Heavy compression denoising
        Assert.Equal("deband=1thr=0.035:2thr=0.035:3thr=0.035:range=18:blur=0", filters[1]); // Heavy debanding
        Assert.Equal("unsharp=3:3:0.4", filters[2]);
        Assert.Equal("fps=30", filters[3]);
    }

    [Fact]
    public void Build_HeavyCompressionWithScaling_UsesAggressiveChain()
    {
        var filters = AdaptiveFilterBuilder.Build(scalePercent: 70, targetFps: 24, targetSizeMb: 8, sourceDuration: 120);

        Assert.Equal(5, filters.Count);
        Assert.Equal("hqdn3d=2.8:2.3:4.5:4.5", filters[0]);
        Assert.Equal("scale=trunc(iw*0.7/2)*2:trunc(ih*0.7/2)*2", filters[1]);
        Assert.Equal("deband=1thr=0.035:2thr=0.035:3thr=0.035:range=18:blur=0", filters[2]);
        Assert.Equal("unsharp=3:3:0.9", filters[3]);
        Assert.Equal("fps=24", filters[4]);
    }
    
    [Fact]
    public void Build_WithExplicitFilterOptions_RespectsOptions()
    {
        // Test that explicit filter options are respected
        var options = new FilterOptions
        {
            EnableDenoising = true,
            EnableDebanding = false,
            EnableSharpening = true,
            EnableScaling = true,
            EnableFpsLimit = false
        };
        
        var filters = AdaptiveFilterBuilder.Build(scalePercent: 80, targetFps: 30, targetSizeMb: 10, sourceDuration: 60, options);

        Assert.Equal(3, filters.Count);
        Assert.Contains(filters, f => f.StartsWith("hqdn3d="));
        Assert.Contains(filters, f => f.StartsWith("scale="));
        Assert.Contains(filters, f => f.StartsWith("unsharp="));
        Assert.DoesNotContain(filters, f => f.StartsWith("deband="));
        Assert.DoesNotContain(filters, f => f.StartsWith("fps="));
    }
}

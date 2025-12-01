using liteclip.Services;
using Xunit;

namespace liteclip.Tests.Services;

public class AdaptiveFilterBuilderTests
{
    [Fact]
    public void Build_LightCompression_ProducesGentleFilters()
    {
        var filters = AdaptiveFilterBuilder.Build(scalePercent: 100, targetFps: 30, targetSizeMb: 200, sourceDuration: 60);

        Assert.Equal(4, filters.Count);
        Assert.Equal("hqdn3d=1:0.8:2.2:2.2", filters[0]);
        Assert.Equal("deband=1thr=0.015:2thr=0.015:3thr=0.015:range=14:blur=0", filters[1]);
        Assert.Equal("unsharp=3:3:0.25", filters[2]);
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
}

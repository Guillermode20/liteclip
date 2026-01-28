using liteclip.Models;
using liteclip.Services;
using Xunit;

namespace liteclip.Tests.Services;

public class DefaultCompressionPlannerTests
{
    [Fact]
    public void BuildPlan_WhenTargetFpsNotProvided_SelectsAutoFps()
    {
        var planner = new DefaultCompressionPlanner();

        var request = new CompressionRequest
        {
            UseQualityMode = true,
            TargetSizeMb = 8,
            SourceDuration = 120,
            // TargetFps intentionally unset (auto)
            ScalePercent = null
        };

        var normalized = planner.NormalizeRequest(request);
        Assert.Null(normalized.TargetFps);

        var plan = planner.BuildPlan(
            jobId: "job",
            normalizedRequest: normalized,
            codecContext: new CodecPlanningContext("h265", ".mp4", 192),
            sourceWidth: 1920,
            sourceHeight: 1080);

        Assert.NotNull(plan);
        Assert.NotNull(plan.Request);
        Assert.True(plan.Request.TargetFps.HasValue);
        Assert.InRange(plan.Request.TargetFps.Value, 1, 30);
    }

    [Fact]
    public void BuildPlan_AtVeryLowVideoBitrate_AllowsScalingBelow480pFloor()
    {
        var planner = new DefaultCompressionPlanner();

        var request = new CompressionRequest
        {
            UseQualityMode = false,
            TargetSizeMb = 8,
            SourceDuration = 240,
            // TargetFps auto
            ScalePercent = null
        };

        var normalized = planner.NormalizeRequest(request);

        var plan = planner.BuildPlan(
            jobId: "job",
            normalizedRequest: normalized,
            codecContext: new CodecPlanningContext("h264", ".mp4", 192),
            sourceWidth: 1920,
            sourceHeight: 1080);

        // Under such a small budget, we expect scaling to be applied.
        Assert.NotNull(plan);
        Assert.NotNull(plan.Request);
        Assert.True(plan.Request.ScalePercent.HasValue);
        Assert.InRange(plan.Request.ScalePercent.Value, 25, 100);

        // Specifically, we should not force a 480p minimum in all cases.
        // For 1080p source, 480p corresponds to ~45% scale.
        // Allowing <45% is required for long clips at tiny targets.
        Assert.True(plan.Request.ScalePercent.Value < 45 || plan.Request.TargetFps.GetValueOrDefault(30) < 30);
    }
}

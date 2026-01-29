using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using liteclip.CompressionStrategies;
using liteclip.Models;

namespace liteclip.Services;

/// <summary>
/// Context for codec-specific planning decisions.
/// </summary>
public sealed record CodecPlanningContext(string CodecKey, string FileExtension, int AudioBitrateKbps);

public sealed class DefaultCompressionPlanner
{
    public CompressionRequest NormalizeRequest(CompressionRequest request)
    {
        var mode = DeriveEncodingMode(request.QualityMode);

        var codec = mode switch
        {
            EncodingMode.Ultra => "h265",
            EncodingMode.Quality => "h265",
            _ => "h264"
        };

        var normalized = new CompressionRequest
        {
            Codec = codec,
            ScalePercent = request.ScalePercent,
            TargetFps = request.TargetFps,
            TargetSizeMb = request.TargetSizeMb,
            SourceDuration = request.SourceDuration,
            Segments = NormalizeSegments(request.Segments, request.SourceDuration),
            SkipCompression = request.SkipCompression,
            MuteAudio = request.MuteAudio,
            UseQualityMode = request.UseQualityMode,
            CropX = request.CropX,
            CropY = request.CropY,
            CropWidth = request.CropWidth,
            CropHeight = request.CropHeight,
            Mode = mode
        };

        if (normalized.ScalePercent.HasValue)
        {
            normalized.ScalePercent = Math.Clamp(normalized.ScalePercent.Value, 10, 100);
        }

        if (normalized.TargetSizeMb.HasValue && normalized.TargetSizeMb.Value <= 0)
        {
            normalized.TargetSizeMb = null;
        }

        if (normalized.SourceDuration.HasValue && normalized.SourceDuration.Value <= 0)
        {
            normalized.SourceDuration = null;
        }

        if (normalized.TargetFps.HasValue)
        {
            normalized.TargetFps = Math.Clamp(normalized.TargetFps.Value, 1, 240);
        }

        // If TargetFps is not provided, treat it as "auto" and let the planner
        // choose an appropriate FPS based on the size/duration budget.

        return normalized;
    }

    public CompressionPlan BuildPlan(
        string jobId,
        CompressionRequest normalizedRequest,
        CodecPlanningContext codecContext,
        int? sourceWidth,
        int? sourceHeight)
    {
        double? totalKbps = null;
        double? videoKbps = null;

        if (normalizedRequest.TargetSizeMb.HasValue &&
            normalizedRequest.TargetSizeMb.Value > 0 &&
            normalizedRequest.SourceDuration.HasValue &&
            normalizedRequest.SourceDuration.Value > 0)
        {
            var bitratePlan = CalculateBitratePlan(normalizedRequest, codecContext);
            if (bitratePlan != null)
            {
                totalKbps = bitratePlan.Value.TotalKbps;
                videoKbps = bitratePlan.Value.VideoKbps;

                // If the user did not specify FPS, choose one automatically.
                // Reducing FPS can be a better tradeoff than crushing resolution at low bitrates.
                if (!normalizedRequest.TargetFps.HasValue &&
                    videoKbps.HasValue &&
                    sourceWidth.HasValue &&
                    sourceHeight.HasValue)
                {
                    normalizedRequest.TargetFps = ChooseAutoFps(
                        sourceWidth.Value,
                        sourceHeight.Value,
                        videoKbps.Value,
                        normalizedRequest.Codec);
                }

                var fpsForScale = normalizedRequest.TargetFps ?? 30;

                if (videoKbps.HasValue &&
                    sourceWidth.HasValue &&
                    sourceHeight.HasValue &&
                    normalizedRequest.ScalePercent != 100)
                {
                    var optimalScale = CalculateOptimalScale(
                        sourceWidth.Value,
                        sourceHeight.Value,
                        videoKbps.Value,
                        fpsForScale,
                        normalizedRequest.Codec);

                    var currentScale = normalizedRequest.ScalePercent ?? 100;
                    if (optimalScale < currentScale)
                    {
                        normalizedRequest.ScalePercent = optimalScale;
                    }
                }
            }
        }

        return new CompressionPlan
        {
            JobId = jobId,
            Request = CloneRequest(normalizedRequest),
            TotalKbps = totalKbps,
            VideoKbps = videoKbps
        };
    }

    private static CompressionRequest CloneRequest(CompressionRequest source)
    {
        return new CompressionRequest
        {
            Codec = source.Codec,
            ScalePercent = source.ScalePercent,
            TargetFps = source.TargetFps,
            TargetSizeMb = source.TargetSizeMb,
            SkipCompression = source.SkipCompression,
            MuteAudio = source.MuteAudio,
            SourceDuration = source.SourceDuration,
            Segments = source.Segments?.Select(segment => new VideoSegment
            {
                Start = segment.Start,
                End = segment.End
            }).ToList(),
            UseQualityMode = source.UseQualityMode,
            QualityMode = source.QualityMode,
            CropX = source.CropX,
            CropY = source.CropY,
            CropWidth = source.CropWidth,
            CropHeight = source.CropHeight,
            Mode = source.Mode
        };
    }

    private static int ChooseAutoFps(int width, int height, double videoKbps, string codec)
    {
        // Prefer higher FPS when we can keep most of the source resolution.
        // If bitrate is tight, lowering FPS often preserves more detail than scaling down.
        var candidates = new[] { 30, 24, 20, 15 };

        var bestFps = 30;
        var bestScale = 0;

        foreach (var fps in candidates)
        {
            var scale = CalculateOptimalScale(width, height, videoKbps, fps, codec);

            // If we can keep >= 70% scale, prefer the highest FPS that meets that.
            if (scale >= 70)
            {
                return fps;
            }

            if (scale > bestScale)
            {
                bestScale = scale;
                bestFps = fps;
            }
        }

        return bestFps;
    }

    private static EncodingMode DeriveEncodingMode(string? qualityMode)
    {
        if (string.Equals(qualityMode, "ultra", StringComparison.OrdinalIgnoreCase))
        {
            return EncodingMode.Ultra;
        }

        if (string.Equals(qualityMode, "high", StringComparison.OrdinalIgnoreCase) || 
            string.Equals(qualityMode, "quality", StringComparison.OrdinalIgnoreCase))
        {
            return EncodingMode.Quality;
        }

        return EncodingMode.Fast;
    }

    private static List<VideoSegment>? NormalizeSegments(List<VideoSegment>? segments, double? sourceDuration)
    {
        if (segments == null || segments.Count == 0)
        {
            return null;
        }

        var normalized = new List<VideoSegment>();
        foreach (var segment in segments)
        {
            if (segment == null)
            {
                continue;
            }

            var start = double.IsFinite(segment.Start) ? Math.Max(0, segment.Start) : 0;
            var end = double.IsFinite(segment.End) ? Math.Max(0, segment.End) : 0;

            if (sourceDuration.HasValue)
            {
                start = Math.Min(start, sourceDuration.Value);
                end = Math.Min(end, sourceDuration.Value);
            }

            if (end - start < 0.05)
            {
                continue;
            }

            normalized.Add(new VideoSegment
            {
                Start = start,
                End = end
            });
        }

        if (normalized.Count == 0)
        {
            return null;
        }

        normalized = normalized
            .OrderBy(s => s.Start)
            .ThenBy(s => s.End)
            .ToList();

        var merged = new List<VideoSegment>();
        foreach (var segment in normalized)
        {
            if (merged.Count == 0)
            {
                merged.Add(new VideoSegment { Start = segment.Start, End = segment.End });
                continue;
            }

            var last = merged[^1];
            if (segment.Start <= last.End + 0.01)
            {
                last.End = Math.Max(last.End, segment.End);
            }
            else
            {
                merged.Add(new VideoSegment { Start = segment.Start, End = segment.End });
            }
        }

        return merged;
    }

    private static (double TotalKbps, double VideoKbps)? CalculateBitratePlan(
        CompressionRequest request,
        CodecPlanningContext codecContext)
    {
        if (!request.TargetSizeMb.HasValue || request.TargetSizeMb.Value <= 0 ||
            !request.SourceDuration.HasValue || request.SourceDuration.Value <= 0)
        {
            return null;
        }

        var targetSizeMb = request.TargetSizeMb.Value * 0.90;
        var durationSeconds = request.SourceDuration.Value;

        var reserveBudgetMb = CalculateReserveBudget(targetSizeMb, durationSeconds, codecContext);
        var containerShare = GetContainerShare(codecContext);

        var containerReserveMb = reserveBudgetMb * (containerShare * 0.7);
        var safetyMarginMb = reserveBudgetMb - containerReserveMb;

        var payloadBudgetMb = targetSizeMb - reserveBudgetMb;
        if (payloadBudgetMb <= 0)
        {
            payloadBudgetMb = Math.Max(targetSizeMb * 0.1, 0.05);
            var adjustedReserve = Math.Max(targetSizeMb - payloadBudgetMb, 0);
            if (reserveBudgetMb > 0)
            {
                var scale = adjustedReserve / reserveBudgetMb;
                containerReserveMb *= scale;
                safetyMarginMb *= scale;
                reserveBudgetMb = adjustedReserve;
            }
        }

        var payloadBits = payloadBudgetMb * 1024 * 1024 * 8;
        var totalKbps = payloadBits / durationSeconds / 1000d;

        // Audio must be budget-aware at tiny target sizes; using a fixed 192k default
        // can starve video and cause severe artifacts.
        var audioBudgetKbps = request.MuteAudio ? 0 : DetermineAudioBudgetKbps(totalKbps, codecContext.AudioBitrateKbps);
        var videoKbps = Math.Max(80, totalKbps - audioBudgetKbps);

        return (Math.Round(totalKbps, 2), Math.Round(videoKbps, 2));
    }

    private static double DetermineAudioBudgetKbps(double totalKbps, int defaultAudioKbps)
    {
        if (totalKbps <= 0)
        {
            return Math.Max(32, Math.Min(defaultAudioKbps, 96));
        }

        // Tiered caps: preserve video first when the overall budget is small.
        // Values are conservative for AAC-in-MP4 compatibility.
        var cap = totalKbps switch
        {
            < 220 => 40,
            < 320 => 56,
            < 450 => 72,
            < 650 => 96,
            _ => defaultAudioKbps * 0.9
        };

        // Also limit audio to a fraction of the total budget.
        var shareCap = totalKbps < 600 ? totalKbps * 0.25 : totalKbps * 0.18;

        var budget = Math.Min(cap, shareCap);
        budget = Math.Clamp(budget, 32, defaultAudioKbps * 0.9);
        return budget;
    }

    private static double CalculateReserveBudget(double targetSizeMb, double durationSeconds, CodecPlanningContext codecContext)
    {
        var baseReserve = 0.20;
        var linearComponent = targetSizeMb * (codecContext.FileExtension.Equals(".mp4", StringComparison.OrdinalIgnoreCase) ? 0.004 : 0.0032);
        var reserve = baseReserve + linearComponent;

        if (durationSeconds >= 1800)
        {
            reserve += 0.14;
        }
        else if (durationSeconds >= 900)
        {
            reserve += 0.07;
        }
        else if (durationSeconds >= 300)
        {
            reserve += 0.035;
        }

        var maxReserve = codecContext.FileExtension.Equals(".mp4", StringComparison.OrdinalIgnoreCase) ? 1.1 : 0.85;
        var minReserve = 0.28;
        reserve = Math.Clamp(reserve, minReserve, maxReserve);

        var maxAllowed = targetSizeMb * 0.82;
        if (reserve > maxAllowed)
        {
            reserve = maxAllowed;
        }

        return Math.Max(reserve, 0);
    }

    private static double GetContainerShare(CodecPlanningContext codecContext)
    {
        return codecContext.FileExtension.Equals(".mp4", StringComparison.OrdinalIgnoreCase) ? 0.68 : 0.48;
    }

    private static int CalculateOptimalScale(int width, int height, double videoKbps, int fps, string codec)
    {
        double targetBpp = codec switch
        {
            var c when c.Equals("h265", StringComparison.OrdinalIgnoreCase) => 0.065,
            var c when c.Equals("hevc", StringComparison.OrdinalIgnoreCase) => 0.065,
            var c when c.Equals("vp9", StringComparison.OrdinalIgnoreCase) => 0.07,
            var c when c.Equals("av1", StringComparison.OrdinalIgnoreCase) => 0.055,
            _ => 0.095
        };

        var pixels = width * height;
        if (pixels <= 0) return 100;

        var maxPixels = (videoKbps * 1000) / (fps * targetBpp);

        if (maxPixels >= pixels) return 100;

        var scale = Math.Sqrt(maxPixels / pixels);

        var percent = (int)(scale * 100);

        // At very low bitrate budgets, enforcing a hard 480p floor produces worse quality
        // than allowing smaller outputs (Discord-style size targets, long clips, etc).
        // Use a bitrate-aware minimum height to preserve perceptual quality.
        var minOutputHeight = DetermineMinOutputHeight(videoKbps, fps, codec);
        if (height >= minOutputHeight)
        {
            var minPercent = (int)Math.Ceiling(minOutputHeight * 100.0 / height);
            if (minPercent > 100) minPercent = 100;
            var minPercentRounded = ((minPercent + 4) / 5) * 5;
            percent = Math.Max(percent, minPercentRounded);
        }

        var finalPercent = Math.Clamp(((percent + 4) / 5) * 5, 25, 100);
        return finalPercent;
    }

    private static int DetermineMinOutputHeight(double videoKbps, int fps, string codec)
    {
        // Conservative floors to keep outputs reasonable, but allow smaller than 480p
        // when bitrate is extremely constrained.
        // Thresholds are video-bitrate based (after audio budget).
        var normalizedCodec = codec?.ToLowerInvariant() ?? "h264";
        var isHevc = normalizedCodec is "h265" or "hevc";

        // Slightly more optimistic for HEVC vs H.264.
        if (isHevc)
        {
            if (videoKbps >= 900) return 480;
            if (videoKbps >= 500) return 360;
            if (videoKbps >= 250) return 240;
            return 144;
        }

        if (videoKbps >= 1200) return 480;
        if (videoKbps >= 700) return 360;
        if (videoKbps >= 350) return 240;
        return 144;
    }
}

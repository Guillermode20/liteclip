using System.Text;

namespace liteclip.Services;

/// <summary>
/// Provides structured logging helpers with better formatting and visual separation
/// </summary>
public static class LoggingHelpers
{
    private const string Separator = "────────────────────────────────────────";
    private const string SubSeparator = "───────────────────";

    /// <summary>
    /// Logs a job lifecycle header with clear visual separation
    /// </summary>
    public static void LogJobHeader(this ILogger logger, string operation, string jobId, string? details = null)
    {
        var header = $"📁 JOB {operation.ToUpper()}";
        if (!string.IsNullOrEmpty(details))
        {
            header += $" - {details}";
        }
        logger.LogInformation($"{Separator}\n{header}\n🆔 Job ID: {jobId}\n{Separator}");
    }

    /// <summary>
    /// Logs a section header for better log organization
    /// </summary>
    public static void LogSection(this ILogger logger, string sectionName)
    {
        logger.LogInformation($"\n{SubSeparator}\n📋 {sectionName.ToUpper()}\n{SubSeparator}");
    }

    /// <summary>
    /// Logs compression request details in a structured format
    /// </summary>
    public static void LogCompressionRequest(this ILogger logger, string jobId, string mode, double? targetSizeMb, double? sourceDuration, int? segmentCount = null)
    {
        logger.LogInformation($"🎬 COMPRESSION REQUEST [{jobId[..8]}]");
        logger.LogInformation($"   Mode: {mode}");
        logger.LogInformation($"   Target Size: {targetSizeMb?.ToString("F2") ?? "N/A"} MB");
        logger.LogInformation($"   Source Duration: {sourceDuration?.ToString("F2") ?? "N/A"}s");
        if (segmentCount.HasValue)
        {
            logger.LogInformation($"   Segments: {segmentCount.Value}");
        }
    }

    /// <summary>
    /// Logs segment processing details
    /// </summary>
    public static void LogSegmentProcessing(this ILogger logger, string jobId, int segmentCount, double totalDuration, double originalDuration)
    {
        logger.LogInformation($"✂️  SEGMENT PROCESSING [{jobId[..8]}]");
        logger.LogInformation($"   Segments: {segmentCount}");
        logger.LogInformation($"   Edited Duration: {totalDuration:F2}s");
        logger.LogInformation($"   Original Duration: {originalDuration:F2}s");
        logger.LogInformation($"   Duration Ratio: {(totalDuration / originalDuration):F2}");
    }

    /// <summary>
    /// Logs FFmpeg command execution
    /// </summary>
    public static void LogFfmpegCommand(this ILogger logger, string operation, string command)
    {
        logger.LogInformation($"🔧 FFMPEG {operation.ToUpper()}");
        logger.LogInformation($"   Command: {command}");
    }

    /// <summary>
    /// Logs bitrate calculation details
    /// </summary>
    public static void LogBitratePlan(this ILogger logger, string jobId, double targetSizeMb, double duration, double videoKbps, double audioKbps, double totalKbps)
    {
        logger.LogInformation($"📊 BITRATE PLAN [{jobId[..8]}]");
        logger.LogInformation($"   Target Size: {targetSizeMb:F2} MB");
        logger.LogInformation($"   Duration: {duration:F2}s");
        logger.LogInformation($"   Video Bitrate: {videoKbps:F0} kbps");
        logger.LogInformation($"   Audio Bitrate: {audioKbps:F0} kbps");
        logger.LogInformation($"   Total Bitrate: {totalKbps:F0} kbps");
    }

    /// <summary>
    /// Logs job completion summary
    /// </summary>
    public static void LogJobCompletion(this ILogger logger, string jobId, bool success, string status, double? outputSizeMb = null, bool? compressionSkipped = null)
    {
        var icon = success ? "✅" : "❌";
        logger.LogInformation($"{icon} JOB {status.ToUpper()} [{jobId[..8]}]");

        if (outputSizeMb.HasValue)
        {
            logger.LogInformation($"   Output Size: {outputSizeMb:F2} MB");
        }

        if (compressionSkipped.HasValue)
        {
            logger.LogInformation($"   Compression Skipped: {compressionSkipped.Value}");
        }
    }

    /// <summary>
    /// Logs API request details
    /// </summary>
    public static void LogApiRequest(this ILogger logger, string method, string endpoint, string? details = null)
    {
        var message = $"🌐 {method} {endpoint}";
        if (!string.IsNullOrEmpty(details))
        {
            message += $" - {details}";
        }
        logger.LogInformation(message);
    }

    /// <summary>
    /// Logs service startup information
    /// </summary>
    public static void LogServiceStartup(this ILogger logger, string serviceName, string? details = null)
    {
        var message = $"🚀 {serviceName} STARTED";
        if (!string.IsNullOrEmpty(details))
        {
            message += $" - {details}";
        }
        logger.LogInformation(message);
    }

    /// <summary>
    /// Logs file operation details
    /// </summary>
    public static void LogFileOperation(this ILogger logger, string operation, string filePath, long? fileSize = null)
    {
        var icon = operation switch
        {
            "Created" => "📝",
            "Deleted" => "🗑️",
            "Copied" => "📋",
            "Merged" => "🔀",
            "Processing" => "⚙️",
            _ => "📁"
        };

        var message = $"{icon} {operation}: {Path.GetFileName(filePath)}";
        if (fileSize.HasValue)
        {
            message += $" ({FormatFileSize(fileSize.Value)})";
        }
        logger.LogInformation($"   {message}");
    }

    private static string FormatFileSize(long bytes)
    {
        string[] sizes = { "B", "KB", "MB", "GB" };
        double len = bytes;
        int order = 0;
        while (len >= 1024 && order < sizes.Length - 1)
        {
            order++;
            len = len / 1024;
        }
        return $"{len:F1} {sizes[order]}";
    }

    /// <summary>
    /// Logs performance timing
    /// </summary>
    public static void LogTiming(this ILogger logger, string operation, TimeSpan duration, string? details = null)
    {
        var message = $"⏱️  {operation}: {duration.TotalSeconds:F2}s";
        if (!string.IsNullOrEmpty(details))
        {
            message += $" - {details}";
        }
        logger.LogInformation(message);
    }

    /// <summary>
    /// Logs warning with better formatting
    /// </summary>
    public static void LogWarning(this ILogger logger, string message, string? context = null)
    {
        var formattedMessage = $"⚠️  {message}";
        if (!string.IsNullOrEmpty(context))
        {
            formattedMessage += $"\n   Context: {context}";
        }
        logger.LogWarning(formattedMessage);
    }

    /// <summary>
    /// Logs error with better formatting
    /// </summary>
    public static void LogError(this ILogger logger, Exception exception, string? context = null)
    {
        var message = $"❌ ERROR: {exception.Message}";
        if (!string.IsNullOrEmpty(context))
        {
            message += $"\n   Context: {context}";
        }
        logger.LogError(exception, message);
    }
}

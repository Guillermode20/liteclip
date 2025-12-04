using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Reflection;
using System.Runtime.InteropServices;

namespace liteclip.Services;

public class FfmpegPathResolver : IFfmpegPathResolver
{
    private readonly ILogger<FfmpegPathResolver> _logger;
    private readonly IConfiguration _configuration;
    private readonly bool _allowSystemPath;
    private readonly string _ffmpegExecutableName;
    private readonly string _ffprobeExecutableName;
    private readonly string? _bundledPathOverride;
    private string? _cachedFfmpegPath;
    private string? _cachedFfprobePath;

    public FfmpegPathResolver(ILogger<FfmpegPathResolver> logger, IConfiguration configuration)
    {
        _logger = logger;
        _configuration = configuration;
        _allowSystemPath = bool.TryParse(_configuration["FFmpeg:AllowSystemPath"], out var allowSystemPath) && allowSystemPath;
        _bundledPathOverride = _configuration["FFmpeg:BundledPath"];
        _ffmpegExecutableName = GetFfmpegExecutableName();
        _ffprobeExecutableName = GetFfprobeExecutableName();
    }

    public string GetFfmpegPath()
    {
        var resolved = ResolveFfmpegPath();
        if (string.IsNullOrWhiteSpace(resolved))
        {
            throw new InvalidOperationException("FFmpeg executable could not be resolved. Ensure the bundled binaries were downloaded or configure FFmpeg:Path.");
        }

        return resolved;
    }

    /// <summary>
    /// Adapter for <see cref="IFfmpegPathResolver"/> - returns the resolved ffmpeg path.
    /// </summary>
    public string? ResolveFfmpegPath()
    {
        if (!string.IsNullOrWhiteSpace(_cachedFfmpegPath) && File.Exists(_cachedFfmpegPath))
        {
            return _cachedFfmpegPath;
        }

        _cachedFfmpegPath = null;
        var seen = new HashSet<string>(StringComparer.OrdinalIgnoreCase);

        foreach (var candidate in GetCandidatePaths())
        {
            var normalized = NormalizePath(candidate);
            if (string.IsNullOrWhiteSpace(normalized) || !seen.Add(normalized))
            {
                continue;
            }

            if (File.Exists(normalized))
            {
                _cachedFfmpegPath = normalized;
                _logger.LogInformation("Using FFmpeg from {Path}", normalized);
                return normalized;
            }
        }

        if (_allowSystemPath)
        {
            var systemFfmpeg = FindInSystemPath(_ffmpegExecutableName);
            if (!string.IsNullOrWhiteSpace(systemFfmpeg))
            {
                _cachedFfmpegPath = systemFfmpeg;
                _logger.LogInformation("Using system FFmpeg from PATH: {Path}", systemFfmpeg);
                return systemFfmpeg;
            }
        }
        else
        {
            _logger.LogDebug("System PATH FFmpeg lookup disabled (FFmpeg:AllowSystemPath=false).");
        }

        return null;
    }

    private static string GetFfmpegExecutableName()
    {
        return RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? "ffmpeg.exe" : "ffmpeg";
    }

    private static string GetFfprobeExecutableName()
    {
        return RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? "ffprobe.exe" : "ffprobe";
    }

    /// <summary>
    /// Returns the cached ffprobe path, deriving it from the ffmpeg path location.
    /// </summary>
    public string? ResolveFfprobePath()
    {
        if (!string.IsNullOrWhiteSpace(_cachedFfprobePath) && File.Exists(_cachedFfprobePath))
        {
            return _cachedFfprobePath;
        }

        var ffmpegPath = ResolveFfmpegPath();
        if (string.IsNullOrWhiteSpace(ffmpegPath))
        {
            return null;
        }

        var directory = Path.GetDirectoryName(ffmpegPath);
        if (string.IsNullOrEmpty(directory))
        {
            return null;
        }

        var probePath = Path.Combine(directory, _ffprobeExecutableName);
        if (File.Exists(probePath))
        {
            _cachedFfprobePath = probePath;
            return probePath;
        }

        // Fallback: try to find in PATH if not next to ffmpeg
        if (_allowSystemPath)
        {
            var systemFfprobe = FindInSystemPath(_ffprobeExecutableName);
            if (!string.IsNullOrWhiteSpace(systemFfprobe))
            {
                _cachedFfprobePath = systemFfprobe;
                return systemFfprobe;
            }
        }

        return null;
    }

    private static string? NormalizePath(string? path)
    {
        if (string.IsNullOrWhiteSpace(path))
        {
            return null;
        }

        try
        {
            var trimmed = path.Trim();
            if (Path.IsPathRooted(trimmed))
            {
                return Path.GetFullPath(trimmed);
            }

            return Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, trimmed));
        }
        catch
        {
            return null;
        }
    }

    private IEnumerable<string> GetCandidatePaths()
    {
        if (!string.IsNullOrWhiteSpace(_configuration["FFmpeg:Path"]))
        {
            yield return _configuration["FFmpeg:Path"]!;
        }

        if (!string.IsNullOrWhiteSpace(_bundledPathOverride))
        {
            yield return _bundledPathOverride!;
        }

        var baseDir = AppContext.BaseDirectory;
        yield return Path.Combine(baseDir, "ffmpeg", _ffmpegExecutableName);
        yield return Path.Combine(baseDir, _ffmpegExecutableName);

        var entryAssemblyLocation = Assembly.GetEntryAssembly()?.Location;
        if (!string.IsNullOrWhiteSpace(entryAssemblyLocation))
        {
            var entryDir = Path.GetDirectoryName(entryAssemblyLocation);
            if (!string.IsNullOrWhiteSpace(entryDir))
            {
                yield return Path.Combine(entryDir, "ffmpeg", _ffmpegExecutableName);
                yield return Path.Combine(entryDir, _ffmpegExecutableName);
            }
        }

        var processPath = Process.GetCurrentProcess().MainModule?.FileName;
        if (!string.IsNullOrWhiteSpace(processPath))
        {
            var processDir = Path.GetDirectoryName(processPath);
            if (!string.IsNullOrWhiteSpace(processDir))
            {
                yield return Path.Combine(processDir, "ffmpeg", _ffmpegExecutableName);
                yield return Path.Combine(processDir, _ffmpegExecutableName);
            }
        }

        var localAppData = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "LiteClip", "ffmpeg", _ffmpegExecutableName);
        yield return localAppData;

        var runtimesDir = Path.Combine(baseDir, "runtimes");
        if (Directory.Exists(runtimesDir))
        {
            foreach (var runtimeSubDir in Directory.EnumerateDirectories(runtimesDir))
            {
                var nativeDir = Path.Combine(runtimeSubDir, "native");
                if (Directory.Exists(nativeDir))
                {
                    yield return Path.Combine(nativeDir, _ffmpegExecutableName);
                }
            }
        }
    }

    private static string? FindInSystemPath(string fileName)
    {
        var pathEnv = Environment.GetEnvironmentVariable("PATH");
        if (string.IsNullOrWhiteSpace(pathEnv))
        {
            return null;
        }

        var paths = pathEnv.Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries);
        
        foreach (var path in paths)
        {
            try
            {
                var fullPath = Path.Combine(path, fileName);
                if (File.Exists(fullPath))
                {
                    return fullPath;
                }
            }
            catch
            {
                // Skip invalid paths
            }
        }

        return null;
    }

}


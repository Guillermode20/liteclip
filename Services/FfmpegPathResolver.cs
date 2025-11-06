using System.Runtime.InteropServices;

namespace smart_compressor.Services;

public class FfmpegPathResolver
{
    private readonly ILogger<FfmpegPathResolver> _logger;
    private readonly IConfiguration _configuration;
    private string? _cachedFfmpegPath;

    public FfmpegPathResolver(ILogger<FfmpegPathResolver> logger, IConfiguration configuration)
    {
        _logger = logger;
        _configuration = configuration;
    }

    public string GetFfmpegPath()
    {
        if (_cachedFfmpegPath != null)
        {
            return _cachedFfmpegPath;
        }

        // 1. Check configuration override
        var configPath = _configuration["FFmpeg:Path"];
        if (!string.IsNullOrWhiteSpace(configPath) && File.Exists(configPath))
        {
            _logger.LogInformation("Using FFmpeg from configuration: {Path}", configPath);
            _cachedFfmpegPath = configPath;
            return _cachedFfmpegPath;
        }

        // 2. Check bundled FFmpeg in the same directory as the executable
        var executableDir = AppContext.BaseDirectory;
        var bundledPath = Path.Combine(executableDir, "ffmpeg", GetFfmpegExecutableName());
        if (File.Exists(bundledPath))
        {
            _logger.LogInformation("Using bundled FFmpeg: {Path}", bundledPath);
            _cachedFfmpegPath = bundledPath;
            return _cachedFfmpegPath;
        }

        // 3. Check ffmpeg folder next to executable (for portable deployments)
        var portablePath = Path.Combine(executableDir, GetFfmpegExecutableName());
        if (File.Exists(portablePath))
        {
            _logger.LogInformation("Using portable FFmpeg: {Path}", portablePath);
            _cachedFfmpegPath = portablePath;
            return _cachedFfmpegPath;
        }

        // 4. Check system PATH
        var systemFfmpeg = FindInSystemPath(GetFfmpegExecutableName());
        if (systemFfmpeg != null)
        {
            _logger.LogInformation("Using system FFmpeg from PATH: {Path}", systemFfmpeg);
            _cachedFfmpegPath = systemFfmpeg;
            return _cachedFfmpegPath;
        }

        // 5. Default fallback - just use "ffmpeg" and hope it's in PATH
        _logger.LogWarning("FFmpeg not found in expected locations. Falling back to 'ffmpeg' command. " +
            "Please ensure FFmpeg is installed and available in system PATH, or configure FFmpeg:Path in appsettings.json");
        _cachedFfmpegPath = GetFfmpegExecutableName();
        return _cachedFfmpegPath;
    }

    public bool VerifyFfmpegAvailable()
    {
        try
        {
            var ffmpegPath = GetFfmpegPath();
            var startInfo = new System.Diagnostics.ProcessStartInfo
            {
                FileName = ffmpegPath,
                Arguments = "-version",
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                UseShellExecute = false,
                CreateNoWindow = true
            };

            using var process = System.Diagnostics.Process.Start(startInfo);
            if (process == null)
            {
                return false;
            }

            process.WaitForExit(5000); // 5 second timeout
            return process.ExitCode == 0;
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Failed to verify FFmpeg availability");
            return false;
        }
    }

    private static string GetFfmpegExecutableName()
    {
        return RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? "ffmpeg.exe" : "ffmpeg";
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

    public void ClearCache()
    {
        _cachedFfmpegPath = null;
    }
}


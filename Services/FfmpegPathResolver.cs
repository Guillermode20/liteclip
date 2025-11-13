using System.Reflection;
using System.Runtime.InteropServices;

namespace liteclip.Services;

public class FfmpegPathResolver : IFfmpegPathResolver
{
    private readonly ILogger<FfmpegPathResolver> _logger;
    private readonly IConfiguration _configuration;
    private string? _cachedFfmpegPath;
    private bool _hasExtractedEmbedded = false;

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

        // 2. Try to extract embedded FFmpeg resource
        if (!_hasExtractedEmbedded)
        {
            var extractedPath = ExtractEmbeddedFfmpeg();
            if (extractedPath != null)
            {
                _logger.LogInformation("Using embedded FFmpeg extracted to: {Path}", extractedPath);
                _cachedFfmpegPath = extractedPath;
                _hasExtractedEmbedded = true;
                return _cachedFfmpegPath;
            }
        }

        // 3. Check bundled FFmpeg in the same directory as the executable
        var executableDir = AppContext.BaseDirectory;
        var bundledPath = Path.Combine(executableDir, "ffmpeg", GetFfmpegExecutableName());
        if (File.Exists(bundledPath))
        {
            _logger.LogInformation("Using bundled FFmpeg: {Path}", bundledPath);
            _cachedFfmpegPath = bundledPath;
            return _cachedFfmpegPath;
        }

        // 4. Check ffmpeg folder next to executable (for portable deployments)
        var portablePath = Path.Combine(executableDir, GetFfmpegExecutableName());
        if (File.Exists(portablePath))
        {
            _logger.LogInformation("Using portable FFmpeg: {Path}", portablePath);
            _cachedFfmpegPath = portablePath;
            return _cachedFfmpegPath;
        }

        // 5. Check system PATH
        var systemFfmpeg = FindInSystemPath(GetFfmpegExecutableName());
        if (systemFfmpeg != null)
        {
            _logger.LogInformation("Using system FFmpeg from PATH: {Path}", systemFfmpeg);
            _cachedFfmpegPath = systemFfmpeg;
            return _cachedFfmpegPath;
        }

        // 6. Default fallback - just use "ffmpeg" and hope it's in PATH
        _logger.LogWarning("FFmpeg not found in expected locations. Falling back to 'ffmpeg' command. " +
            "Please ensure FFmpeg is installed and available in system PATH, or configure FFmpeg:Path in appsettings.json");
        _cachedFfmpegPath = GetFfmpegExecutableName();
        return _cachedFfmpegPath;
    }

    /// <summary>
    /// Adapter for <see cref="IFfmpegPathResolver"/> - returns the resolved ffmpeg path.
    /// </summary>
    public string? ResolveFfmpegPath()
    {
        return GetFfmpegPath();
    }

    private string? ExtractEmbeddedFfmpeg()
    {
        try
        {
            var assembly = Assembly.GetExecutingAssembly();
            var resourceName = "liteclip.ffmpeg.ffmpeg.exe";
            
            // Check if the resource exists
            var resourceNames = assembly.GetManifestResourceNames();
            if (!resourceNames.Contains(resourceName))
            {
                _logger.LogDebug("Embedded FFmpeg resource not found in assembly");
                return null;
            }

            // Extract to temp directory
            var tempDir = Path.Combine(Path.GetTempPath(), "liteclip-ffmpeg");
            Directory.CreateDirectory(tempDir);
            
            var extractedPath = Path.Combine(tempDir, GetFfmpegExecutableName());
            
            // Only extract if it doesn't exist or is different
            if (!File.Exists(extractedPath))
            {
                _logger.LogInformation("Extracting embedded FFmpeg to: {Path}", extractedPath);
                
                using var resourceStream = assembly.GetManifestResourceStream(resourceName);
                if (resourceStream == null)
                {
                    _logger.LogWarning("Failed to open embedded FFmpeg resource stream");
                    return null;
                }
                
                using var fileStream = File.Create(extractedPath);
                resourceStream.CopyTo(fileStream);
                
                _logger.LogInformation("Successfully extracted embedded FFmpeg ({Size} bytes)", new FileInfo(extractedPath).Length);
            }
            
            return extractedPath;
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Failed to extract embedded FFmpeg");
            return null;
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

}


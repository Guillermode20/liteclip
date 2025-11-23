#if !NET7_0_OR_GREATER
using System.Diagnostics;
#endif
using System.Runtime.InteropServices;
using System.Threading;
using Xabe.FFmpeg;
using Xabe.FFmpeg.Downloader;

namespace liteclip.Services;

public enum FfmpegBootstrapState
{
    Idle,
    Checking,
    Downloading,
    Ready,
    Error
}

public record FfmpegBootstrapStatus
{
    public FfmpegBootstrapState State { get; init; } = FfmpegBootstrapState.Idle;
    public double ProgressPercent { get; init; }
        = 0;
    public long DownloadedBytes { get; init; }
        = 0;
    public long TotalBytes { get; init; }
        = 0;
    public string? Message { get; init; }
        = "Initializing";
    public string? ExecutablePath { get; init; }
        = null;
    public string? ErrorMessage { get; init; }
        = null;
    public bool Ready => State == FfmpegBootstrapState.Ready;
}

public class FfmpegBootstrapper
{
    private readonly ILogger<FfmpegBootstrapper> _logger;
    private readonly IFfmpegPathResolver _resolver;
    private readonly object _ensureLock = new();
    private Task? _ensureTask;
    private volatile FfmpegBootstrapStatus _status = new();

    public FfmpegBootstrapper(ILogger<FfmpegBootstrapper> logger, IFfmpegPathResolver resolver)
    {
        _logger = logger;
        _resolver = resolver;
    }

    public FfmpegBootstrapStatus GetStatus() => _status;

    public Task EnsureReadyAsync()
    {
        var existingTask = Volatile.Read(ref _ensureTask);
        if (existingTask != null)
        {
            return existingTask;
        }

        lock (_ensureLock)
        {
            existingTask = _ensureTask;
            if (existingTask == null)
            {
                _ensureTask = EnsureInternalAsync();
                existingTask = _ensureTask;
            }
        }

        return existingTask!;
    }

    private async Task EnsureInternalAsync()
    {
        try
        {
            UpdateStatus(FfmpegBootstrapState.Checking, 0, "Checking FFmpeg binaries...");

            // First, check if a valid FFmpeg exists already via the path resolver.
            // This avoids attempting a download or other probing that may cause a visible window.
            try
            {
                var resolved = _resolver?.ResolveFfmpegPath();
                if (!string.IsNullOrWhiteSpace(resolved))
                {
                    string? fullPath = null;
                    if (Path.IsPathRooted(resolved))
                    {
                        if (File.Exists(resolved)) fullPath = resolved;
                    }
                    else
                    {
                        // If resolver returned a non-rooted name (like "ffmpeg"), explicitly search PATH entries.
                        var exeName = RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? "ffmpeg.exe" : "ffmpeg";
                        var pathEnv = Environment.GetEnvironmentVariable("PATH");
                        if (!string.IsNullOrWhiteSpace(pathEnv))
                        {
                            var paths = pathEnv.Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries);
                            foreach (var p in paths)
                            {
                                try
                                {
                                    var candidate = Path.Combine(p, exeName);
                                    if (File.Exists(candidate))
                                    {
                                        fullPath = candidate;
                                        break;
                                    }
                                }
                                catch { }
                            }
                        }
                    }

                    if (!string.IsNullOrWhiteSpace(fullPath))
                    {
                        UpdateStatus(
                            FfmpegBootstrapState.Ready,
                            100,
                            $"FFmpeg already available at {fullPath}",
                            fullPath
                        );
                        _logger.LogInformation("FFmpeg already available and will be used: {Path}", fullPath);
                        return;
                    }
                }
            }
            catch (Exception ex)
            {
                _logger.LogDebug(ex, "PathResolver check for existing ffmpeg failed; continuing with download attempt.");
            }

            // Prefer the app base "ffmpeg" directory, but if running from Program Files
            // (or another protected location) we'll automatically fall back to a user-local path.
            var defaultExecutablesDirectory = Path.Combine(AppContext.BaseDirectory, "ffmpeg");
            var localAppData = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "LiteClip", "ffmpeg");
            var commonAppData = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData), "LiteClip", "ffmpeg");

            string executablesDirectory;
            if (IsPathWritable(defaultExecutablesDirectory))
            {
                executablesDirectory = defaultExecutablesDirectory;
            }
            else if (IsPathWritable(localAppData))
            {
                executablesDirectory = localAppData;
                _logger.LogInformation("ffmpeg: base directory is not writable; using LocalAppData: {Path}", executablesDirectory);
            }
            else if (IsPathWritable(commonAppData))
            {
                executablesDirectory = commonAppData;
                _logger.LogInformation("ffmpeg: base directory is not writable; using CommonAppData: {Path}", executablesDirectory);
            }
            else
            {
                // Fallback to default; download may still fail due to permissions.
                executablesDirectory = defaultExecutablesDirectory;
            }

            Directory.CreateDirectory(executablesDirectory);

            var ffmpegName = RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? "ffmpeg.exe" : "ffmpeg";
            var ffprobeName = RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? "ffprobe.exe" : "ffprobe";

            var ffmpegPath = Path.Combine(executablesDirectory, ffmpegName);
            var ffprobePath = Path.Combine(executablesDirectory, ffprobeName);

            if (File.Exists(ffmpegPath) && File.Exists(ffprobePath))
            {
                UpdateStatus(
                    FfmpegBootstrapState.Ready,
                    100,
                    $"FFmpeg already available at {ffmpegPath}",
                    ffmpegPath
                );
                _logger.LogInformation("FFmpeg already available and will be used: {Path}", ffmpegPath);
                return;
            }

            UpdateStatus(FfmpegBootstrapState.Downloading, 0, $"Downloading FFmpeg binaries to {executablesDirectory}...");

            var progress = new Progress<ProgressInfo>(info =>
            {
                var percent = info.TotalBytes > 0
                    ? Math.Clamp(info.DownloadedBytes / (double)info.TotalBytes * 100d, 0, 100)
                    : 0;

                _status = _status with
                {
                    ProgressPercent = percent,
                    DownloadedBytes = info.DownloadedBytes,
                    TotalBytes = info.TotalBytes,
                    Message = "Downloading FFmpeg binaries..."
                };
            });

            await FFmpegDownloader.GetLatestVersion(FFmpegVersion.Official, executablesDirectory, progress);

            GrantUnixPermissionsIfNeeded(ffmpegPath);
            GrantUnixPermissionsIfNeeded(ffprobePath);

            UpdateStatus(
                FfmpegBootstrapState.Ready,
                100,
                $"FFmpeg download completed ({executablesDirectory})",
                ffmpegPath
            );
            _logger.LogInformation("FFmpeg downloaded and ready at: {Path}", ffmpegPath);
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Failed to download FFmpeg binaries");
            _status = _status with
            {
                State = FfmpegBootstrapState.Error,
                ErrorMessage = ex.Message,
                Message = "Failed to download FFmpeg binaries. See logs for details."
            };
            throw;
        }
    }

    public void ResetForRetry()
    {
        if (_status.State == FfmpegBootstrapState.Error)
        {
            lock (_ensureLock)
            {
                _ensureTask = null;
                _status = new FfmpegBootstrapStatus
                {
                    State = FfmpegBootstrapState.Idle,
                    Message = "Retrying FFmpeg download..."
                };
            }
        }
    }

    private void UpdateStatus(FfmpegBootstrapState state, double progress, string message, string? executablePath = null)
    {
        _status = _status with
        {
            State = state,
            ProgressPercent = progress,
            Message = message,
            ExecutablePath = executablePath
        };
    }

    private static void GrantUnixPermissionsIfNeeded(string path)
    {
        if (string.IsNullOrWhiteSpace(path) || !File.Exists(path))
        {
            return;
        }

        if (!OperatingSystem.IsLinux() && !OperatingSystem.IsMacOS())
        {
            return;
        }

        try
        {
#if NET7_0_OR_GREATER
            var mode = UnixFileMode.UserExecute | UnixFileMode.UserRead | UnixFileMode.UserWrite |
                       UnixFileMode.GroupExecute | UnixFileMode.GroupRead |
                       UnixFileMode.OtherExecute | UnixFileMode.OtherRead;
            File.SetUnixFileMode(path, mode);
#else
            // Fallback: rely on chmod
            System.Diagnostics.Process.Start("chmod", $"+x \"{path}\"");
#endif
        }
        catch
        {
            // Best-effort only; log suppressed to avoid noisy startup.
        }
    }

    private static bool IsPathWritable(string directory)
    {
        try
        {
            Directory.CreateDirectory(directory);
            var testFile = Path.Combine(directory, ".liteclip_write_test");
            File.WriteAllText(testFile, "ok");
            File.Delete(testFile);
            return true;
        }
        catch
        {
            return false;
        }
    }
}

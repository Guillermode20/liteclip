using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using liteclip.Models;
using Microsoft.Extensions.Logging;

namespace liteclip.Services;

public class UserSettingsStore
{
    private readonly ILogger<UserSettingsStore> _logger;
    private readonly string _settingsPath;
    private readonly SemaphoreSlim _syncLock = new(1, 1);
    private UserSettings? _cachedSettings;

    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web)
    {
        WriteIndented = true
    };

    public UserSettingsStore(ILogger<UserSettingsStore> logger)
    {
        _logger = logger;
        _settingsPath = ResolveSettingsPath();
    }

    public async Task<UserSettings> GetAsync(CancellationToken cancellationToken = default)
    {
        await _syncLock.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            if (_cachedSettings is null)
            {
                _cachedSettings = await LoadFromDiskAsync(cancellationToken).ConfigureAwait(false) ?? UserSettings.CreateDefault();
            }

            return Clone(_cachedSettings);
        }
        finally
        {
            _syncLock.Release();
        }
    }

    public async Task<UserSettings> UpdateAsync(UserSettings incoming, CancellationToken cancellationToken = default)
    {
        await _syncLock.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            var sanitized = Sanitize(incoming);
            _cachedSettings = sanitized;
            await SaveToDiskAsync(sanitized, cancellationToken).ConfigureAwait(false);
            return Clone(sanitized);
        }
        finally
        {
            _syncLock.Release();
        }
    }

    private async Task<UserSettings?> LoadFromDiskAsync(CancellationToken cancellationToken)
    {
        try
        {
            if (!File.Exists(_settingsPath))
            {
                return null;
            }

            await using var stream = File.OpenRead(_settingsPath);
            return await System.Text.Json.JsonSerializer.DeserializeAsync<UserSettings>(stream, liteclip.Serialization.LiteClipJsonContext.Default.UserSettings, cancellationToken).ConfigureAwait(false);
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "Failed to read user settings. Falling back to defaults.");
            return null;
        }
    }

    private async Task SaveToDiskAsync(UserSettings settings, CancellationToken cancellationToken)
    {
        try
        {
            var directory = Path.GetDirectoryName(_settingsPath);
            if (!string.IsNullOrWhiteSpace(directory))
            {
                Directory.CreateDirectory(directory);
            }

            await using var stream = File.Create(_settingsPath);
            await System.Text.Json.JsonSerializer.SerializeAsync(stream, settings, liteclip.Serialization.LiteClipJsonContext.Default.UserSettings, cancellationToken).ConfigureAwait(false);
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "Failed to persist user settings to disk.");
        }
    }

    private static UserSettings Sanitize(UserSettings settings)
    {
        var sanitized = new UserSettings
        {
            DefaultCodec = NormalizeCodec(settings.DefaultCodec),
            DefaultResolution = NormalizeResolution(settings.DefaultResolution),
            DefaultMuteAudio = settings.DefaultMuteAudio,
            DefaultTargetSizeMb = ClampMb(settings.DefaultTargetSizeMb),
            StartMaximized = settings.StartMaximized,
            CheckForUpdatesOnLaunch = settings.CheckForUpdatesOnLaunch,
            DefaultFolder = NormalizeFolder(settings.DefaultFolder)
        };

        return sanitized;
    }

    private static string NormalizeCodec(string? value)
    {
        return string.Equals(value, "fast", StringComparison.OrdinalIgnoreCase) ? "fast" : "quality";
    }

    private static string NormalizeResolution(string? value)
    {
        var normalized = value?.ToLowerInvariant();
        return normalized switch
        {
            "source" => "source",
            "1080p" => "1080p",
            "720p" => "720p",
            "480p" => "480p",
            "360p" => "360p",
            _ => "auto"
        };
    }

    private static double ClampMb(double? value)
    {
        if (!value.HasValue || double.IsNaN(value.Value) || double.IsInfinity(value.Value))
        {
            return 25;
        }

        return Math.Max(1, value.Value);
    }

    private static string NormalizeFolder(string? value)
    {
        if (string.IsNullOrWhiteSpace(value))
        {
            return string.Empty;
        }

        try
        {
            // Convert to absolute path and validate it exists or is potentially valid
            var fullPath = Path.GetFullPath(value);
            return fullPath;
        }
        catch
        {
            // If path is invalid, return empty string
            return string.Empty;
        }
    }

    private static UserSettings Clone(UserSettings source)
    {
        return new UserSettings
        {
            DefaultCodec = source.DefaultCodec,
            DefaultResolution = source.DefaultResolution,
            DefaultMuteAudio = source.DefaultMuteAudio,
            DefaultTargetSizeMb = source.DefaultTargetSizeMb,
            StartMaximized = source.StartMaximized,
            CheckForUpdatesOnLaunch = source.CheckForUpdatesOnLaunch,
            DefaultFolder = source.DefaultFolder
        };
    }

    private static string ResolveSettingsPath()
    {
        var appDataDirectory = Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData);
        var settingsDirectory = Path.Combine(appDataDirectory, "LiteClip");
        return Path.Combine(settingsDirectory, "user-settings.json");
    }
}

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
            return await JsonSerializer.DeserializeAsync<UserSettings>(stream, JsonOptions, cancellationToken).ConfigureAwait(false);
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
            await JsonSerializer.SerializeAsync(stream, settings, JsonOptions, cancellationToken).ConfigureAwait(false);
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
            DefaultTargetSizePercent = ClampPercent(settings.DefaultTargetSizePercent),
            CheckForUpdatesOnLaunch = settings.CheckForUpdatesOnLaunch
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

    private static double ClampPercent(double? value)
    {
        if (!value.HasValue || double.IsNaN(value.Value) || double.IsInfinity(value.Value))
        {
            return 50;
        }

        return Math.Clamp(value.Value, 1, 100);
    }

    private static UserSettings Clone(UserSettings source)
    {
        return new UserSettings
        {
            DefaultCodec = source.DefaultCodec,
            DefaultResolution = source.DefaultResolution,
            DefaultMuteAudio = source.DefaultMuteAudio,
            DefaultTargetSizePercent = source.DefaultTargetSizePercent,
            CheckForUpdatesOnLaunch = source.CheckForUpdatesOnLaunch
        };
    }

    private static string ResolveSettingsPath()
    {
        string baseDirectory;
        try
        {
            baseDirectory = Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData);
            if (string.IsNullOrWhiteSpace(baseDirectory))
            {
                baseDirectory = AppContext.BaseDirectory;
            }
        }
        catch
        {
            baseDirectory = AppContext.BaseDirectory;
        }

        var settingsDirectory = Path.Combine(baseDirectory, "LiteClip");
        return Path.Combine(settingsDirectory, "user-settings.json");
    }
}

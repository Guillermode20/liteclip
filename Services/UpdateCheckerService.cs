using System.Net.Http.Headers;
using System.Net.Http.Json;
using liteclip.Models;
using System.Reflection;
using System.Text.Json.Serialization;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.Extensions.Logging;

namespace liteclip.Services;

public sealed class UpdateCheckerService
{
    private static readonly TimeSpan CacheDuration = TimeSpan.FromHours(6);

    private readonly IHttpClientFactory _httpClientFactory;
    private readonly ILogger<UpdateCheckerService> _logger;
    private readonly SemaphoreSlim _cacheLock = new(1, 1);

    private UpdateInfo? _cachedInfo;
    private DateTime _cachedAt;

    public UpdateCheckerService(IHttpClientFactory httpClientFactory, ILogger<UpdateCheckerService> logger)
    {
        _httpClientFactory = httpClientFactory;
        _logger = logger;
    }

    public async Task<UpdateInfo> GetUpdateInfoAsync(bool forceRefresh = false, CancellationToken cancellationToken = default)
    {
        await _cacheLock.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            if (!forceRefresh && _cachedInfo is not null && (DateTime.UtcNow - _cachedAt) < CacheDuration)
            {
                return _cachedInfo;
            }

            var currentVersion = ResolveCurrentVersion();
            var release = await FetchLatestReleaseAsync(cancellationToken).ConfigureAwait(false);

            if (release is null)
            {
                var fallback = new UpdateInfo(currentVersion, currentVersion, false, null, DateTime.UtcNow, null);
                _cachedInfo = fallback;
                _cachedAt = DateTime.UtcNow;
                return fallback;
            }

            var latestVersion = NormalizeVersion(release.TagName ?? release.Name ?? currentVersion, currentVersion);
            var updateAvailable = IsUpdateAvailable(currentVersion, latestVersion);
            var info = new UpdateInfo(
                currentVersion,
                latestVersion,
                updateAvailable,
                release.HtmlUrl,
                DateTime.UtcNow,
                release.Body);

            _cachedInfo = info;
            _cachedAt = DateTime.UtcNow;
            return info;
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "Update check failed");
            var fallback = new UpdateInfo(ResolveCurrentVersion(), ResolveCurrentVersion(), false, null, DateTime.UtcNow, null);
            _cachedInfo = fallback;
            _cachedAt = DateTime.UtcNow;
            return fallback;
        }
        finally
        {
            _cacheLock.Release();
        }
    }

    private async Task<GitHubRelease?> FetchLatestReleaseAsync(CancellationToken cancellationToken)
    {
        try
        {
            var client = _httpClientFactory.CreateClient(nameof(UpdateCheckerService));
            client.DefaultRequestHeaders.UserAgent.Clear();
            client.DefaultRequestHeaders.UserAgent.Add(new ProductInfoHeaderValue("liteclip", "1.0"));
            client.DefaultRequestHeaders.Accept.Add(new MediaTypeWithQualityHeaderValue("application/vnd.github+json"));

            var response = await client.GetAsync("https://api.github.com/repos/Guillermode20/smart-compressor/releases/latest", cancellationToken).ConfigureAwait(false);
            if (!response.IsSuccessStatusCode)
            {
                _logger.LogWarning("GitHub update check failed with status {StatusCode}", response.StatusCode);
                return null;
            }

            // Use explicit deserialization to avoid ReadFromJsonAsync trimmer warnings
            var json = await response.Content.ReadAsStringAsync(cancellationToken).ConfigureAwait(false);
            if (string.IsNullOrWhiteSpace(json)) return null;
            try
            {
                var release = System.Text.Json.JsonSerializer.Deserialize<GitHubRelease>(json, liteclip.Serialization.LiteClipJsonContext.Default.GitHubRelease);
                return release;
            }
            catch (Exception ex)
            {
                _logger.LogWarning(ex, "Failed to parse GitHub release JSON");
                return null;
            }
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "GitHub update check threw an exception");
            return null;
        }
    }

    private static string ResolveCurrentVersion()
    {
        var assembly = Assembly.GetExecutingAssembly();
        var informational = assembly.GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion;
        if (!string.IsNullOrWhiteSpace(informational))
        {
            return NormalizeVersion(informational, informational);
        }

        var version = assembly.GetName().Version?.ToString();
        return string.IsNullOrWhiteSpace(version) ? "0.0.0" : NormalizeVersion(version!, version!);
    }

    private static bool IsUpdateAvailable(string currentVersion, string latestVersion)
    {
        var current = ParseVersion(currentVersion);
        var latest = ParseVersion(latestVersion);
        return latest > current;
    }

    private static string NormalizeVersion(string? value, string fallback)
    {
        if (string.IsNullOrWhiteSpace(value))
        {
            return fallback;
        }

        var sanitized = value.Trim();
        if (sanitized.StartsWith("v", StringComparison.OrdinalIgnoreCase))
        {
            sanitized = sanitized[1..];
        }

        var plusIndex = sanitized.IndexOf('+');
        if (plusIndex >= 0)
        {
            sanitized = sanitized[..plusIndex];
        }

        var dashIndex = sanitized.IndexOf('-');
        if (dashIndex >= 0)
        {
            sanitized = sanitized[..dashIndex];
        }

        return sanitized;
    }

    private static Version ParseVersion(string? value)
    {
        if (Version.TryParse(value, out var parsed))
        {
            return parsed;
        }

        return new Version(0, 0, 0, 0);
    }

    // Use public GitHubRelease record defined in Models/GitHubRelease.cs
}

public sealed record UpdateInfo(
    string CurrentVersion,
    string LatestVersion,
    bool UpdateAvailable,
    string? DownloadUrl,
    DateTime CheckedAt,
    string? ReleaseNotes);

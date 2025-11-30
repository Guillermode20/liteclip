using System.Reflection;
using System;
using System.Threading;

namespace liteclip.Services;

public interface IAppVersionProvider
{
    string GetCurrentVersion();
}

public sealed class AppVersionProvider : IAppVersionProvider
{
    private static readonly Lazy<string> CachedVersion = new(ResolveCurrentVersion, LazyThreadSafetyMode.ExecutionAndPublication);

    public string GetCurrentVersion() => CachedVersion.Value;

    private static string ResolveCurrentVersion()
    {
        var assembly = Assembly.GetExecutingAssembly();
        var informational = assembly.GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion;
        if (!string.IsNullOrWhiteSpace(informational))
        {
            return NormalizeVersion(informational, informational);
        }

        var version = assembly.GetName().Version?.ToString();
        return string.IsNullOrWhiteSpace(version)
            ? "0.0.0"
            : NormalizeVersion(version!, version!);
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
}

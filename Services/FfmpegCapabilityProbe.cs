// NOTE: FfmpegCapabilityProbe has been deprecated and removed from runtime registration.
// The old probe used to actively probe encoder availability and x265 'subme' support.
// Keep a lightweight placeholder for compatibility with old references in the repo.
namespace liteclip.Services;

public sealed class FfmpegCapabilityProbe
{
    // Deprecated: this class is now a no-op and is intentionally left non-functional.
    // It exists only to prevent compile-time errors in references that may linger.
    public System.Collections.Generic.HashSet<string> SupportedEncoders { get; } = new(System.StringComparer.OrdinalIgnoreCase);
    public int? MaxX265Subme { get; } = null;
}

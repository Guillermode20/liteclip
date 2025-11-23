using System.Text.Json.Serialization;

namespace liteclip.Models;

public sealed record GitHubRelease(
    [property: JsonPropertyName("tag_name")] string? TagName,
    [property: JsonPropertyName("html_url")] string? HtmlUrl,
    [property: JsonPropertyName("name")] string? Name,
    [property: JsonPropertyName("body")] string? Body);

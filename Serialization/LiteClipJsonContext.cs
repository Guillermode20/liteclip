using System.Text.Json.Serialization;
using liteclip.Models;

namespace liteclip.Serialization;

[JsonSerializable(typeof(UserSettings))]
[JsonSerializable(typeof(GitHubRelease))]
[JsonSerializable(typeof(VideoSegment[]))]
[JsonSerializable(typeof(List<VideoSegment>))]
public partial class LiteClipJsonContext : JsonSerializerContext
{
}

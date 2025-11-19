namespace liteclip.Models;

public class UserSettings
{
    public string DefaultCodec { get; set; } = "quality";
    public string DefaultResolution { get; set; } = "auto";
    public bool DefaultMuteAudio { get; set; }
    public double DefaultTargetSizePercent { get; set; } = 50;
    public bool CheckForUpdatesOnLaunch { get; set; } = true;

    public static UserSettings CreateDefault() => new();
}

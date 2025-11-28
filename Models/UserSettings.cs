namespace liteclip.Models;

public class UserSettings
{
    public string DefaultCodec { get; set; } = "quality";
    public string DefaultResolution { get; set; } = "auto";
    public bool DefaultMuteAudio { get; set; }
    public double DefaultTargetSizeMb { get; set; } = 25;
    public bool CheckForUpdatesOnLaunch { get; set; } = true;
    public bool StartMaximized { get; set; } = true;
    public string DefaultFolder { get; set; } = string.Empty;
    public double AppScale { get; set; } = 1.0;

    public static UserSettings CreateDefault() => new();
}

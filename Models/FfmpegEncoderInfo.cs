using System;

namespace liteclip.Models
{
    public class FfmpegEncoderInfo
    {
        public string Name { get; set; } = string.Empty;
        public string? Description { get; set; }
        public bool IsHardware { get; set; }
        public bool? IsAvailable { get; set; }
        public string? Notes { get; set; }
    }
}

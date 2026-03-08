use ffmpeg_next as ffmpeg;

use crate::config::RateControl;

use super::FfmpegEncoder;

impl FfmpegEncoder {
    pub(super) fn apply_qsv_options(&self, options: &mut ffmpeg::Dictionary<'_>, bitrate: usize) {
        let bitrate_bps = bitrate.to_string();
        let peak_bitrate_bps = self.peak_bitrate_bps().to_string();

        options.set("preset", self.qsv_preset());
        options.set("look_ahead", "0");
        options.set(
            "rc",
            match self.config.rate_control {
                RateControl::Cbr => "cbr",
                RateControl::Vbr | RateControl::Cq => "vbr",
            },
        );
        options.set("b", &bitrate_bps);
        options.set("maxrate", &peak_bitrate_bps);
        options.set("bufsize", &bitrate_bps);
    }
}

use ffmpeg_next as ffmpeg;

use crate::config::RateControl;

use super::FfmpegEncoder;

impl FfmpegEncoder {
    pub(super) fn apply_nvenc_options(&self, options: &mut ffmpeg::Dictionary<'_>, bitrate: usize) {
        let bitrate_bps = bitrate.to_string();
        let peak_bitrate_bps = self.peak_bitrate_bps().to_string();

        options.set("preset", self.nvenc_preset());
        options.set("tune", self.nvenc_tune());
        options.set("delay", "0");
        options.set("zerolatency", "1");
        options.set("strict_gop", "1");
        options.set("b_ref_mode", "disabled");
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

        if matches!(self.config.rate_control, RateControl::Cbr) {
            options.set("minrate", &bitrate_bps);
        }
        if matches!(self.config.rate_control, RateControl::Cq) {
            options.set("cq", &self.cq_value().to_string());
        }

        options.set("forced-idr", "1");
    }
}

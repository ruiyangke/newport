//! Explicit per-session encoding policy. No transport or device discovery.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodecMode {
    /// Default adaptive policy: balance processing and delivery against the FPS goal.
    #[default]
    Auto,
    /// Adapt quality, AV1 regions and bounded raw/LZ4 trials from measured service.
    Adaptive,
    /// Require raw/LZ4 tiles. Pacing still obeys the FPS ceiling.
    Fast,
    /// Use compressed lossless tiles; never JPEG or AV1.
    Lossless,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub codec_mode: CodecMode,
    /// Per-window FPS goal and scheduling ceiling, not a delivered-rate guarantee.
    pub max_fps: u32,
    /// Ceiling for adaptive photographic tiles, also mapped to AV1 quantization.
    /// Alpha stays lossless.
    pub jpeg_quality: u8,
    /// A bounded low-CPU codec trial, for local transports: never a
    /// bandwidth guarantee.
    #[serde(default)]
    pub fast_trial: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            codec_mode: CodecMode::Auto,
            max_fps: 60,
            jpeg_quality: 90,
            fast_trial: false,
        }
    }
}
impl Settings {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=240).contains(&self.max_fps) {
            return Err("max_fps must be between 1 and 240".into());
        }
        if !(45..=95).contains(&self.jpeg_quality) {
            return Err("jpeg_quality must be between 45 and 95".into());
        }
        Ok(())
    }
}

#[cfg(all(test, feature = "protobuf"))]
mod tests {
    use super::*;
    #[test]
    fn settings_are_bounded_and_round_trip() {
        for mode in [
            CodecMode::Auto,
            CodecMode::Adaptive,
            CodecMode::Fast,
            CodecMode::Lossless,
        ] {
            for fps in [1, 24, 60, 144, 240] {
                let settings = Settings {
                    codec_mode: mode,
                    max_fps: fps,
                    jpeg_quality: 95,
                    fast_trial: fps == 24,
                };
                settings.validate().unwrap();
                let wire = crate::encoding::protobuf::generated::newport::remote::apps::PerformanceSettings::from(settings);
                assert_eq!(Settings::try_from(wire).unwrap(), settings);
            }
        }
        for fps in [0, 241, u32::MAX] {
            assert!(
                Settings {
                    max_fps: fps,
                    ..Default::default()
                }
                .validate()
                .is_err()
            );
        }
        for quality in [0, 44, 96, 255] {
            assert!(
                Settings {
                    jpeg_quality: quality,
                    ..Default::default()
                }
                .validate()
                .is_err()
            );
        }
    }
}

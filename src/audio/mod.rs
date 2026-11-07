//! A small music player: Symphonia and libopus decode, ALSA plays. Nothing here knows about GTK.

mod alsa;
mod analyzer;
mod cover;
mod downmix;
mod dynlib;
mod engine;
mod fft;
mod ogg;
mod opus;
mod playlist;
mod sink;
mod source;
mod style;

pub use alsa::available as alsa_available;
pub use analyzer::{Analyzer, Frame, WINDOW};
pub use cover::folder_cover;
pub use engine::{DEFAULT_VOLUME, Event, Player, State};
pub use opus::available as opus_available;
pub use playlist::{Playlist, Repeat};
pub use sink::{Sink, SinkFactory, system_sink};
pub use source::{Cover, FileSource, OpenError, TrackInfo};
pub use style::VisualStyle;

/// Formats the player may be asked to open. Others go to the default application.
pub fn can_try(name: &str) -> bool {
    crate::listing::is_audio(name)
}

/// 3:07, or 1:02:03 for an hour or more.
pub fn clock(time: std::time::Duration) -> String {
    let secs = time.as_secs();
    let (hours, minutes, seconds) = (secs / 3600, secs / 60 % 60, secs % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// "FLAC · 44.1 kHz · stereo", for the line under the title.
pub fn describe(info: &TrackInfo) -> String {
    let mut parts = Vec::new();
    if !info.codec.is_empty() {
        parts.push(info.codec.clone());
    }
    if info.sample_rate > 0 {
        let khz = f64::from(info.sample_rate) / 1000.0;
        parts.push(if khz.fract() == 0.0 { format!("{khz:.0} kHz") } else { format!("{khz:.1} kHz") });
    }
    match info.channels {
        0 => {}
        1 => parts.push("mono".into()),
        2 => parts.push("stereo".into()),
        6 => parts.push("5.1".into()),
        8 => parts.push("7.1".into()),
        n => parts.push(format!("{n} channels")),
    }
    parts.join(" \u{b7} ")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn the_details_line_names_codec_rate_and_channels() {
        let mut info = TrackInfo { codec: "FLAC".into(), sample_rate: 44_100, channels: 2, ..TrackInfo::default() };
        assert_eq!(describe(&info), "FLAC \u{b7} 44.1 kHz \u{b7} stereo");
        info = TrackInfo { codec: "Opus".into(), sample_rate: 48_000, channels: 6, ..TrackInfo::default() };
        assert_eq!(describe(&info), "Opus \u{b7} 48 kHz \u{b7} 5.1");
        info = TrackInfo { codec: "MP3".into(), sample_rate: 22_050, channels: 1, ..TrackInfo::default() };
        assert_eq!(describe(&info), "MP3 \u{b7} 22.1 kHz \u{b7} mono");
        assert_eq!(describe(&TrackInfo::default()), "");
        let odd = TrackInfo { channels: 3, ..TrackInfo::default() };
        assert_eq!(describe(&odd), "3 channels");
    }

    #[test]
    fn clock_formats_minutes_and_hours() {
        assert_eq!(super::clock(Duration::ZERO), "0:00");
        assert_eq!(super::clock(Duration::from_secs(187)), "3:07");
        assert_eq!(super::clock(Duration::from_millis(59_999)), "0:59");
        assert_eq!(super::clock(Duration::from_secs(3723)), "1:02:03");
    }

    #[test]
    fn audio_names_are_recognised_case_insensitively() {
        assert!(super::can_try("Song.MP3"));
        assert!(super::can_try("a.opus"));
        assert!(!super::can_try("a.txt"));
    }
}

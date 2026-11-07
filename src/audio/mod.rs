//! A small music player: Symphonia and libopus decode, ALSA plays. Nothing here knows about GTK.

mod alsa;
mod downmix;
mod engine;
mod ogg;
mod opus;
mod sink;
mod source;

pub use alsa::available as alsa_available;
pub use engine::{DEFAULT_VOLUME, Event, Player, State};
pub use opus::available as opus_available;
pub use sink::{Sink, SinkFactory, system_sink};
pub use source::{Cover, FileSource, OpenError, TrackInfo};

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

#[cfg(test)]
mod tests {
    use std::time::Duration;

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

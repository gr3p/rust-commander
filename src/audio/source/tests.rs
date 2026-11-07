use std::path::{Path, PathBuf};
use std::time::Duration;

use super::*;

/// Opus needs libopus at run time. Without it the Opus files are skipped, not failed.
fn skip(name: &str) -> bool {
    let missing = name.ends_with(".opus") && !crate::audio::opus_available();
    if missing {
        eprintln!("skipped {name}: no libopus");
    }
    missing
}

pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio").join(name)
}

/// Decodes a whole file the way the player does.
fn read_all(source: &mut FileSource) -> Vec<f32> {
    let mut out = Vec::new();
    while source.read(&mut out).unwrap() {}
    out
}

fn open(name: &str) -> FileSource {
    FileSource::open(&fixture(name)).unwrap_or_else(|err| panic!("{name}: {err}"))
}

fn secs(source: &FileSource) -> f64 {
    source.info().duration.map_or(f64::NAN, |d| d.as_secs_f64())
}

fn zero_crossings(samples: &[f32], channel: usize, width: usize, frames: usize) -> usize {
    let mut count = 0;
    let mut last = 0.0f32;
    for frame in samples.chunks_exact(width).take(frames) {
        let s = frame[channel];
        if (last < 0.0) != (s < 0.0) && last != 0.0 {
            count += 1;
        }
        last = s;
    }
    count
}

const CHIRPS: [&str; 6] = ["chirp.wav", "chirp.flac", "chirp.mp3", "chirp.ogg", "chirp.opus", "chirp.m4a"];

#[test]
fn every_format_reports_two_seconds_and_decodes_about_that_much() {
    for name in CHIRPS {
        if skip(name) {
            continue;
        }
        let mut source = open(name);
        let rate = source.sample_rate();
        let reported = secs(&source);
        assert!((reported - 2.0).abs() < 0.06, "{name}: reported {reported}");
        assert_eq!(source.channels(), 1, "{name}");
        let samples = read_all(&mut source);
        let seconds = samples.len() as f64 / f64::from(rate);
        // AAC keeps its encoder delay, which is about 50 ms. The rest are gapless.
        let slack = if name.ends_with(".m4a") { 0.06 } else { 0.01 };
        assert!((seconds - 2.0).abs() < slack, "{name}: decoded {seconds} s at {rate} Hz");
        assert!(samples.iter().all(|s| s.is_finite() && s.abs() <= 1.0001), "{name}: out of range");
        let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.3, "{name}: silent, peak {peak}");
    }
}

#[test]
fn lossless_and_opus_files_are_exactly_two_seconds() {
    for (name, rate) in [("chirp.wav", 44_100), ("chirp.flac", 44_100), ("chirp.opus", 48_000)] {
        if skip(name) {
            continue;
        }
        let mut source = open(name);
        assert_eq!(source.sample_rate(), rate, "{name}");
        let samples = read_all(&mut source);
        assert_eq!(samples.len(), rate as usize * 2, "{name}");
        assert_eq!(source.position_frames(), rate as u64 * 2, "{name}");
    }
}

#[test]
fn the_pitch_rises_through_the_file() {
    // The chirp runs from 300 Hz up to 1100 Hz, so later parts cross zero more often.
    for name in CHIRPS {
        if skip(name) {
            continue;
        }
        let mut source = open(name);
        let rate = source.sample_rate() as usize;
        let samples = read_all(&mut source);
        let early = zero_crossings(&samples[rate / 10..], 0, 1, rate / 4);
        let late = zero_crossings(&samples[rate + rate / 2..], 0, 1, rate / 4);
        assert!(late > early * 2, "{name}: {early} then {late}");
    }
}

#[test]
fn stereo_keeps_the_two_channels_apart() {
    for name in ["stereo.flac", "stereo.opus", "stereo.mp3"] {
        if skip(name) {
            continue;
        }
        let mut source = open(name);
        assert_eq!(source.channels(), 2, "{name}");
        let rate = source.sample_rate() as usize;
        let samples = read_all(&mut source);
        let left = zero_crossings(&samples[rate / 5 * 2..], 0, 2, rate / 2);
        let right = zero_crossings(&samples[rate / 5 * 2..], 1, 2, rate / 2);
        // 440 Hz and 660 Hz over half a second, two crossings per cycle.
        assert!((left as f64 - 440.0).abs() < 25.0, "{name}: left {left}");
        assert!((right as f64 - 660.0).abs() < 35.0, "{name}: right {right}");
    }
}

#[test]
fn surround_files_fold_to_stereo() {
    for (name, rate) in [("surround.flac", 16_000), ("surround.opus", 48_000)] {
        if skip(name) {
            continue;
        }
        let mut source = open(name);
        assert_eq!(source.info().channels, 6, "{name}");
        assert_eq!(source.channels(), 2, "{name}");
        assert_eq!(source.sample_rate(), rate, "{name}");
        let samples = read_all(&mut source);
        let frames = samples.len() / 2;
        assert!((frames as f64 / f64::from(rate) - 0.5).abs() < 0.02, "{name}: {frames} frames");
        let energy = |channel: usize| samples.chunks_exact(2).map(|f| f[channel] * f[channel]).sum::<f32>();
        // Left, centre and a rear channel feed the left side, so both sides carry sound.
        assert!(energy(0) > 1.0 && energy(1) > 1.0, "{name}: {} {}", energy(0), energy(1));
        assert!(samples.iter().all(|s| s.abs() <= 1.0001), "{name}: clipped");
    }
}

#[test]
fn tags_and_cover_art_come_through() {
    for name in ["tagged.mp3", "tagged.flac", "tagged.m4a", "tagged.opus"] {
        if skip(name) {
            continue;
        }
        let source = open(name);
        let info = source.info();
        assert_eq!(info.title.as_deref(), Some("Test_Song"), "{name}");
        assert_eq!(info.artist.as_deref(), Some("Test_Artist"), "{name}");
        assert_eq!(info.album.as_deref(), Some("Test_Album"), "{name}");
    }
    for name in ["tagged.mp3", "tagged.flac", "tagged.m4a"] {
        let cover = open(name).info().cover.clone().unwrap_or_else(|| panic!("{name}: no cover"));
        assert!(cover.mime.contains("jpeg") || cover.mime.contains("jpg"), "{name}: {}", cover.mime);
        assert_eq!(&cover.data[..2], &[0xff, 0xd8], "{name}: not a JPEG");
    }
    // A file without tags has none to show.
    let plain = open("chirp.flac");
    assert!(plain.info().title.is_none() && plain.info().cover.is_none());
}

#[test]
fn the_codec_is_named_for_the_details_line() {
    let expect = [("chirp.mp3", "MP3"), ("chirp.flac", "FLAC"), ("chirp.opus", "Opus"), ("chirp.ogg", "Vorbis"), ("chirp.wav", "PCM")];
    for (name, label) in expect {
        if skip(name) {
            continue;
        }
        assert_eq!(open(name).info().codec, label, "{name}");
    }
    assert_eq!(open("chirp.m4a").info().codec, "AAC");
}

/// Mean absolute difference between `a` and `b` over `len` frames, with `b` shifted by `lag`.
fn distance(a: &[f32], b: &[f32], at_a: usize, at_b: usize, lag: isize, len: usize) -> f32 {
    let mut sum = 0.0;
    for i in 0..len {
        let x = a[at_a + i];
        let y = b[(at_b as isize + i as isize + lag) as usize];
        sum += (x - y).abs();
    }
    sum / len as f32
}

#[test]
fn seeking_lands_on_the_same_audio_as_playing_through() {
    // (file, largest accepted offset in frames)
    let cases = [("chirp.wav", 0), ("chirp.flac", 0), ("chirp.mp3", 40), ("chirp.ogg", 40), ("chirp.opus", 40), ("chirp.m4a", 1100)];
    for (name, tolerance) in cases {
        if skip(name) {
            continue;
        }
        let mut whole = open(name);
        let reference = read_all(&mut whole);
        let rate = whole.sample_rate() as usize;
        for target in [0.1f64, 0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75] {
            let mut source = open(name);
            source.seek(Duration::from_secs_f64(target)).unwrap();
            let want = (target * rate as f64).round() as usize;
            assert_eq!(source.position_frames() as usize, want, "{name} at {target}: clock");
            let tail = read_all(&mut source);
            assert!(!tail.is_empty(), "{name} at {target}: nothing after the seek");
            // Skip the decoder's warm-up, then find the best alignment around the target.
            let skip = 2048;
            let len = 2000;
            assert!(tail.len() > skip + len, "{name} at {target}: tail {}", tail.len());
            let mut best = (isize::MAX, f32::MAX);
            for lag in -(tolerance as isize + 8)..=(tolerance as isize + 8) {
                let d = distance(&tail, &reference, skip, want + skip, lag, len);
                if d < best.1 {
                    best = (lag, d);
                }
            }
            assert!(best.0.unsigned_abs() <= tolerance, "{name} at {target}: off by {} frames (error {})", best.0, best.1);
            let limit = if tolerance == 0 { 1e-6 } else { 0.06 };
            assert!(best.1 < limit, "{name} at {target}: differs by {} at lag {}", best.1, best.0);
        }
    }
}

#[test]
fn the_tail_length_after_a_seek_matches_what_is_left() {
    for (name, tolerance) in [("chirp.wav", 0), ("chirp.flac", 0), ("chirp.opus", 64), ("chirp.ogg", 64), ("chirp.mp3", 1200)] {
        if skip(name) {
            continue;
        }
        let mut source = open(name);
        let rate = source.sample_rate() as usize;
        source.seek(Duration::from_millis(1500)).unwrap();
        let tail = read_all(&mut source);
        let left = rate * 2 - rate * 3 / 2;
        let off = (tail.len() as isize - left as isize).unsigned_abs();
        assert!(off <= tolerance, "{name}: {} frames left, expected {left}", tail.len());
    }
}

#[test]
fn seeking_back_and_forth_keeps_working() {
    for name in CHIRPS {
        if skip(name) {
            continue;
        }
        let mut source = open(name);
        let mut block = Vec::new();
        for target in [1.2, 0.1, 1.9, 0.0, 0.7] {
            source.seek(Duration::from_secs_f64(target)).unwrap();
            block.clear();
            assert!(source.read(&mut block).unwrap(), "{name} at {target}");
            assert!(!block.is_empty());
        }
    }
}

#[test]
fn seeking_past_the_end_ends_the_track() {
    for name in CHIRPS {
        if skip(name) {
            continue;
        }
        let mut source = open(name);
        source.seek(Duration::from_secs(60)).unwrap();
        let mut out = Vec::new();
        // At most a trailing scrap, then the end.
        while source.read(&mut out).unwrap() {
            assert!(out.len() < 4_000, "{name}: still playing after a seek to the end");
        }
        // The track can be played again from the top.
        source.seek(Duration::ZERO).unwrap();
        out.clear();
        assert!(source.read(&mut out).unwrap(), "{name}: dead after seeking to the end");
    }
}

#[test]
fn reading_after_the_end_stays_at_the_end() {
    let mut source = open("chirp.wav");
    read_all(&mut source);
    let mut out = Vec::new();
    for _ in 0..3 {
        assert!(!source.read(&mut out).unwrap());
    }
    assert!(out.is_empty());
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rust-commander-audio-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn files_that_are_not_audio_are_refused_with_a_reason() {
    let dir = scratch("refuse");
    let text = dir.join("notes.mp3");
    std::fs::write(&text, "this is not music, whatever the name says\n".repeat(50)).unwrap();
    assert!(matches!(FileSource::open(&text), Err(OpenError::Unsupported(_))));

    let empty = dir.join("empty.flac");
    std::fs::write(&empty, b"").unwrap();
    assert!(matches!(FileSource::open(&empty), Err(OpenError::Failed(_))));

    let missing = dir.join("gone.ogg");
    assert!(matches!(FileSource::open(&missing), Err(OpenError::Io(_))));

    // A WMA header, which no decoder here reads.
    let wma = dir.join("song.wma");
    let mut bytes = vec![0x30, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11, 0xa6, 0xd9, 0x00, 0xaa, 0x00, 0x62, 0xce, 0x6c];
    bytes.extend(vec![0u8; 4096]);
    std::fs::write(&wma, bytes).unwrap();
    assert!(matches!(FileSource::open(&wma), Err(OpenError::Unsupported(_)) | Err(OpenError::Failed(_))));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_wrong_extension_does_not_stop_a_real_file() {
    let dir = scratch("renamed");
    let renamed = dir.join("song.dat");
    std::fs::copy(fixture("chirp.flac"), &renamed).unwrap();
    let mut source = FileSource::open(&renamed).unwrap();
    assert_eq!(source.info().codec, "FLAC");
    assert_eq!(read_all(&mut source).len(), 88_200);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn truncated_files_play_what_is_there_and_never_hang() {
    let dir = scratch("cut");
    for name in CHIRPS {
        if skip(name) {
            continue;
        }
        let bytes = std::fs::read(fixture(name)).unwrap();
        for part in [3, 2] {
            let cut = dir.join(format!("{part}-{name}"));
            std::fs::write(&cut, &bytes[..bytes.len() / part]).unwrap();
            if let Ok(mut source) = FileSource::open(&cut) {
                let mut out = Vec::new();
                let mut blocks = 0;
                while let Ok(true) = source.read(&mut out) {
                    blocks += 1;
                    assert!(blocks < 10_000, "{name}: never ends");
                }
                assert!(out.len() as f64 / f64::from(source.sample_rate()) < 2.1, "{name}: longer than the original");
            }
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// Small deterministic generator, so a failing corruption can be replayed.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

#[test]
fn damaged_files_never_panic_or_hang() {
    let dir = scratch("fuzz");
    let mut rng = Lcg(0x5eed);
    for name in CHIRPS.iter().chain(["stereo.opus", "stereo.mp3", "tagged.mp3", "tagged.flac", "tagged.m4a", "surround.opus"].iter()) {
        if skip(name) {
            continue;
        }
        let original = std::fs::read(fixture(name)).unwrap();
        for round in 0..25 {
            let mut bytes = original.clone();
            // Flip a handful of bytes, sometimes in the header and sometimes in the body.
            for _ in 0..1 + round % 6 {
                let at = if round % 3 == 0 { rng.next() as usize % bytes.len().min(256) } else { rng.next() as usize % bytes.len() };
                bytes[at] = rng.next() as u8;
            }
            if round % 5 == 4 {
                let keep = rng.next() as usize % bytes.len();
                bytes.truncate(keep.max(1));
            }
            let path = dir.join(format!("{round}-{name}"));
            std::fs::write(&path, &bytes).unwrap();
            let outcome = std::panic::catch_unwind(|| {
                let Ok(mut source) = FileSource::open(&path) else { return };
                let mut out = Vec::new();
                let mut blocks = 0;
                while let Ok(true) = source.read(&mut out) {
                    blocks += 1;
                    out.clear();
                    assert!(blocks < 200_000, "never ends");
                }
                let _ = source.seek(Duration::from_millis(700));
                let _ = source.read(&mut out);
            });
            assert!(outcome.is_ok(), "panic on {name} round {round}; file kept in {dir:?}");
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

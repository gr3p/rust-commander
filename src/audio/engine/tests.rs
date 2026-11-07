use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc::{Receiver, channel};

use super::*;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio").join(name)
}

/// What the fake sound card heard.
#[derive(Clone, Default)]
struct Record {
    samples: Arc<Mutex<Vec<f32>>>,
    opened: Arc<Mutex<Vec<(u32, usize)>>>,
    flushes: Arc<AtomicUsize>,
    pauses: Arc<AtomicUsize>,
}

impl Record {
    fn frames(&self, width: usize) -> usize {
        self.samples.lock().unwrap().len() / width
    }

    fn len(&self) -> usize {
        self.samples.lock().unwrap().len()
    }
}

struct FakeSink {
    rec: Record,
    rate: u32,
    channels: usize,
    // Times faster than real time. 0 writes at once.
    speed: f64,
    hardware_pause: bool,
    fail_after: Option<usize>,
    // What the device claims is still queued, however long it has been since the last write.
    queued: u64,
}

impl Sink for FakeSink {
    fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        if self.fail_after.is_some_and(|limit| self.rec.len() >= limit) {
            return Err("The sound card was unplugged".into());
        }
        self.rec.samples.lock().unwrap().extend_from_slice(samples);
        if self.speed > 0.0 {
            let seconds = samples.len() as f64 / self.channels as f64 / f64::from(self.rate) / self.speed;
            std::thread::sleep(Duration::from_secs_f64(seconds));
        }
        Ok(())
    }

    fn pause(&mut self, on: bool) -> bool {
        self.rec.pauses.fetch_add(1, Ordering::Relaxed);
        !on || self.hardware_pause
    }

    fn flush(&mut self) {
        self.rec.flushes.fetch_add(1, Ordering::Relaxed);
    }

    fn delay(&mut self) -> u64 {
        self.queued
    }
}

struct Options {
    speed: f64,
    hardware_pause: bool,
    fail_after: Option<usize>,
    refuse_open: bool,
    queued: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self { speed: 0.0, hardware_pause: true, fail_after: None, refuse_open: false, queued: 0 }
    }
}

struct Rig {
    player: Player,
    events: Receiver<Event>,
    rec: Record,
}

fn rig_with(options: Options) -> Rig {
    let rec = Record::default();
    let made = rec.clone();
    let Options { speed, hardware_pause, fail_after, refuse_open, queued } = options;
    let factory: SinkFactory = Arc::new(move |rate, channels| {
        if refuse_open {
            return Err("No sound card".into());
        }
        made.opened.lock().unwrap().push((rate, channels));
        Ok(Box::new(FakeSink { rec: made.clone(), rate, channels, speed, hardware_pause, fail_after, queued }))
    });
    let (tx, events) = channel();
    let tx = Mutex::new(tx);
    let player = Player::spawn(factory, 1.0, move |event| {
        let _ = tx.lock().unwrap().send(event);
    });
    Rig { player, events, rec }
}

fn rig() -> Rig {
    rig_with(Options::default())
}

impl Rig {
    fn wait(&self, wanted: impl Fn(&Event) -> bool) -> Event {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(left) {
                Ok(event) if wanted(&event) => return event,
                Ok(_) => {}
                Err(_) => panic!("timed out waiting for an event"),
            }
        }
    }

    fn play_to_end(&self, name: &str, generation: u64) {
        self.player.play(fixture(name), generation);
        self.wait(|e| matches!(e, Event::Ended { generation: g } if *g == generation));
    }
}

fn until(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out: {what}");
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt()
}

const FORMATS: [&str; 6] = ["chirp.wav", "chirp.flac", "chirp.mp3", "chirp.ogg", "chirp.opus", "chirp.m4a"];

/// Opus needs libopus at run time. Without it the Opus files are skipped, not failed.
fn skip(name: &str) -> bool {
    let missing = name.ends_with(".opus") && !crate::audio::opus_available();
    if missing {
        eprintln!("skipped {name}: no libopus");
    }
    missing
}

#[test]
fn every_format_plays_to_the_end_and_reports_it() {
    for name in FORMATS {
        if skip(name) {
            continue;
        }
        let rig = rig();
        rig.player.play(fixture(name), 7);
        let loaded = rig.wait(|e| matches!(e, Event::Loaded { .. }));
        let Event::Loaded { generation, info } = loaded else { unreachable!() };
        assert_eq!(generation, 7);
        assert!((info.duration.unwrap().as_secs_f64() - 2.0).abs() < 0.06, "{name}");
        rig.wait(|e| matches!(e, Event::Ended { generation: 7 }));
        assert_eq!(rig.player.state(), State::Ended, "{name}");
        // About two seconds of one-channel audio reached the device, at the file's own rate.
        let (rate, channels) = rig.rec.opened.lock().unwrap()[0];
        assert_eq!(channels, 1, "{name}");
        let seconds = rig.rec.frames(1) as f64 / f64::from(rate);
        assert!((seconds - 2.0).abs() < 0.07, "{name}: {seconds} s at {rate} Hz");
        // The clock rests on the end.
        let dur = rig.player.duration().unwrap();
        assert_eq!(rig.player.position(), dur, "{name}");
    }
}

#[test]
fn stereo_and_surround_open_the_device_with_the_right_channels() {
    let rig = rig();
    let mut expect = Vec::new();
    if !skip("stereo.opus") {
        rig.play_to_end("stereo.opus", 1);
        expect.push((48_000, 2));
    }
    rig.play_to_end("surround.flac", 2);
    rig.play_to_end("chirp.wav", 3);
    expect.extend([(16_000, 2), (44_100, 1)]);
    let opened = rig.rec.opened.lock().unwrap().clone();
    assert_eq!(opened, expect);
}

#[test]
fn the_device_is_reused_between_tracks_of_the_same_format() {
    let rig = rig_with(Options { speed: 12.0, ..Options::default() });
    rig.player.play(fixture("chirp.flac"), 1);
    rig.wait(|e| matches!(e, Event::Loaded { generation: 1, .. }));
    until("first track playing", || rig.rec.len() > 1_000);
    // Switch while the first is playing, to a file of the same rate and channel count.
    rig.player.play(fixture("chirp.wav"), 2);
    rig.wait(|e| matches!(e, Event::Ended { generation: 2 }));
    assert_eq!(rig.rec.opened.lock().unwrap().len(), 1, "a second open means the device was not reused");
}

#[test]
fn tags_arrive_in_the_loaded_event() {
    let rig = rig();
    rig.player.play(fixture("tagged.mp3"), 1);
    let Event::Loaded { info, .. } = rig.wait(|e| matches!(e, Event::Loaded { .. })) else { unreachable!() };
    assert_eq!(info.title.as_deref(), Some("Test_Song"));
    assert_eq!(info.artist.as_deref(), Some("Test_Artist"));
    assert!(info.cover.is_some());
}

#[test]
fn a_file_that_cannot_play_fails_with_a_reason_and_does_not_open_the_device() {
    let dir = std::env::temp_dir().join(format!("rust-commander-engine-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let text = dir.join("notes.mp3");
    std::fs::write(&text, "not audio ".repeat(200)).unwrap();
    let rig = rig();
    rig.player.play(text, 4);
    let Event::Failed { generation, message, unsupported } = rig.wait(|e| matches!(e, Event::Failed { .. })) else { unreachable!() };
    assert_eq!(generation, 4);
    assert!(unsupported, "{message}");
    assert!(!message.is_empty());
    assert_eq!(rig.player.state(), State::Failed);
    assert!(rig.rec.opened.lock().unwrap().is_empty());
    // The player is still usable afterwards.
    rig.play_to_end("chirp.wav", 5);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_missing_file_is_not_called_unsupported() {
    let rig = rig();
    rig.player.play(fixture("does-not-exist.mp3"), 1);
    let Event::Failed { unsupported, message, .. } = rig.wait(|e| matches!(e, Event::Failed { .. })) else { unreachable!() };
    assert!(!unsupported);
    assert!(message.contains("gone"), "{message}");
}

#[test]
fn a_missing_sound_card_still_shows_the_tags_and_then_fails() {
    let rig = rig_with(Options { refuse_open: true, ..Options::default() });
    rig.player.play(fixture("tagged.flac"), 1);
    rig.wait(|e| matches!(e, Event::Loaded { .. }));
    let Event::Failed { message, unsupported, .. } = rig.wait(|e| matches!(e, Event::Failed { .. })) else { unreachable!() };
    assert_eq!(message, "No sound card");
    assert!(!unsupported);
    assert_eq!(rig.player.state(), State::Failed);
}

#[test]
fn a_device_that_dies_mid_track_fails_the_track() {
    let rig = rig_with(Options { fail_after: Some(20_000), ..Options::default() });
    rig.player.play(fixture("chirp.wav"), 1);
    let Event::Failed { message, .. } = rig.wait(|e| matches!(e, Event::Failed { .. })) else { unreachable!() };
    assert!(message.contains("unplugged"));
    assert_eq!(rig.player.state(), State::Failed);
}

#[test]
fn pause_stops_the_flow_and_resume_finishes_the_track() {
    let rig = rig_with(Options { speed: 12.0, ..Options::default() });
    rig.player.play(fixture("chirp.wav"), 1);
    until("some audio", || rig.rec.len() > 4_000);
    rig.player.pause();
    until("paused", || rig.player.state() == State::Paused);
    // Anything already on its way lands, then nothing more.
    std::thread::sleep(Duration::from_millis(80));
    let held = rig.rec.len();
    std::thread::sleep(Duration::from_millis(250));
    assert_eq!(rig.rec.len(), held, "audio kept flowing while paused");
    let position = rig.player.position();
    assert!(position > Duration::ZERO && position < Duration::from_secs(2));
    rig.player.resume();
    rig.wait(|e| matches!(e, Event::Ended { .. }));
    assert_eq!(rig.rec.frames(1), 88_200, "pausing must not lose or repeat audio");
}

#[test]
fn a_device_that_cannot_pause_resumes_where_it_stopped() {
    let rig = rig_with(Options { speed: 12.0, hardware_pause: false, ..Options::default() });
    rig.player.play(fixture("chirp.flac"), 1);
    until("some audio", || rig.rec.len() > 8_000);
    rig.player.pause();
    until("paused", || rig.player.state() == State::Paused);
    std::thread::sleep(Duration::from_millis(80));
    let flushes = rig.rec.flushes.load(Ordering::Relaxed);
    assert!(flushes >= 1, "a device that cannot pause must be flushed");
    let held = rig.rec.len();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(rig.rec.len(), held);
    rig.player.resume();
    rig.wait(|e| matches!(e, Event::Ended { .. }));
    // The track finishes, and nothing is skipped: the total is at most the length plus the lead-in
    // that was flushed and then played again.
    let frames = rig.rec.frames(1);
    assert!((88_200..88_200 + 4_800).contains(&frames), "{frames} frames");
}

#[test]
fn toggle_pauses_resumes_and_restarts_an_ended_track() {
    let rig = rig_with(Options { speed: 12.0, ..Options::default() });
    rig.player.play(fixture("chirp.wav"), 1);
    until("playing", || rig.rec.len() > 2_000);
    rig.player.toggle();
    until("paused", || rig.player.state() == State::Paused);
    rig.player.toggle();
    until("playing again", || rig.player.state() == State::Playing);
    rig.wait(|e| matches!(e, Event::Ended { .. }));
    let first = rig.rec.frames(1);
    // Toggling an ended track plays it again from the start.
    rig.player.toggle();
    rig.wait(|e| matches!(e, Event::Ended { .. }));
    assert_eq!(rig.rec.frames(1), first * 2);
}

#[test]
fn stop_silences_and_forgets_the_track() {
    let rig = rig_with(Options { speed: 12.0, ..Options::default() });
    rig.player.play(fixture("chirp.wav"), 1);
    until("some audio", || rig.rec.len() > 2_000);
    rig.player.stop();
    until("idle", || rig.player.state() == State::Idle);
    std::thread::sleep(Duration::from_millis(60));
    let held = rig.rec.len();
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(rig.rec.len(), held);
    assert_eq!(rig.player.position(), Duration::ZERO);
    assert_eq!(rig.player.duration(), None);
    // Nothing to resume or toggle.
    rig.player.toggle();
    rig.player.resume();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(rig.player.state(), State::Idle);
}

#[test]
fn seeking_jumps_and_the_rest_of_the_track_follows() {
    for name in ["chirp.wav", "chirp.flac", "chirp.opus", "chirp.mp3"] {
        if skip(name) {
            continue;
        }
        let rig = rig_with(Options { speed: 12.0, ..Options::default() });
        rig.player.play(fixture(name), 1);
        until("started", || rig.rec.len() > 1_000);
        rig.player.seek(Duration::from_millis(1500));
        rig.wait(|e| matches!(e, Event::Ended { .. }));
        let samples = rig.rec.samples.lock().unwrap().len();
        let (rate, _) = rig.rec.opened.lock().unwrap()[0];
        // The audio before the jump plus the half second after it. The first part is short.
        let seconds = samples as f64 / f64::from(rate);
        assert!(seconds > 0.5 && seconds < 1.5, "{name}: {seconds} s in total");
        assert!(rig.rec.flushes.load(Ordering::Relaxed) >= 1, "{name}: a seek must flush the device");
    }
}

#[test]
fn seek_by_moves_relative_to_the_clock_and_stays_inside_the_track() {
    let rig = rig_with(Options { speed: 12.0, ..Options::default() });
    rig.player.play(fixture("chirp.flac"), 1);
    until("started", || rig.rec.len() > 4_000);
    rig.player.pause();
    until("paused", || rig.player.state() == State::Paused);
    std::thread::sleep(Duration::from_millis(50));
    rig.player.seek_by(1);
    until("moved forward", || rig.player.position() >= Duration::from_millis(900));
    let after = rig.player.position();
    assert!(after < Duration::from_millis(1600), "{after:?}");
    rig.player.seek_by(-30);
    until("clamped to the start", || rig.player.position() == Duration::ZERO);
    rig.player.seek_by(30);
    until("clamped to the end", || rig.player.position() >= Duration::from_millis(1990));
    assert!(rig.player.position() <= rig.player.duration().unwrap());
    assert_eq!(rig.player.state(), State::Paused, "seeking keeps a pause");
}

#[test]
fn a_seek_while_paused_keeps_the_pause_and_resumes_from_the_new_place() {
    let rig = rig_with(Options { speed: 12.0, ..Options::default() });
    rig.player.play(fixture("chirp.wav"), 1);
    until("started", || rig.rec.len() > 4_000);
    rig.player.pause();
    until("paused", || rig.player.state() == State::Paused);
    std::thread::sleep(Duration::from_millis(60));
    rig.player.seek(Duration::from_millis(1800));
    std::thread::sleep(Duration::from_millis(120));
    assert_eq!(rig.player.state(), State::Paused);
    let held = rig.rec.len();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(rig.rec.len(), held);
    rig.player.resume();
    rig.wait(|e| matches!(e, Event::Ended { .. }));
    // Only the last 0.2 s plays after the jump.
    let added = rig.rec.len() - held;
    assert!((added as f64 - 8_820.0).abs() < 600.0, "{added} samples after resuming");
}

#[test]
fn seeking_past_the_end_ends_the_track() {
    let rig = rig_with(Options { speed: 12.0, ..Options::default() });
    rig.player.play(fixture("chirp.flac"), 1);
    until("started", || rig.rec.len() > 1_000);
    rig.player.seek(Duration::from_secs(600));
    rig.wait(|e| matches!(e, Event::Ended { .. }));
    assert_eq!(rig.player.state(), State::Ended);
}

#[test]
fn a_seek_after_the_end_starts_the_track_again_from_there() {
    let rig = rig();
    rig.play_to_end("chirp.wav", 1);
    let first = rig.rec.len();
    rig.player.seek(Duration::from_millis(1000));
    rig.wait(|e| matches!(e, Event::Ended { .. }));
    let again = rig.rec.len() - first;
    assert!((again as f64 - 44_100.0).abs() < 600.0, "{again} samples");
}

#[test]
fn volume_scales_the_sound_and_mute_silences_it() {
    let loud = rig();
    loud.play_to_end("chirp.wav", 1);
    let full = rms(&loud.rec.samples.lock().unwrap());

    let quiet = rig();
    quiet.player.set_volume(0.5);
    quiet.play_to_end("chirp.wav", 1);
    let half = rms(&quiet.rec.samples.lock().unwrap());
    // 0.5 on a cubic curve is an eighth of the amplitude.
    assert!((half / full - 0.125).abs() < 0.01, "{}", half / full);
    assert_eq!(quiet.player.volume(), 0.5);

    let muted = rig();
    muted.player.set_muted(true);
    muted.play_to_end("chirp.wav", 1);
    assert!(muted.rec.samples.lock().unwrap().iter().all(|s| *s == 0.0));
    assert!(muted.player.muted());
}

#[test]
fn volume_is_clamped_and_changes_without_a_jump() {
    let rig = rig_with(Options { speed: 12.0, ..Options::default() });
    rig.player.set_volume(3.0);
    assert_eq!(rig.player.volume(), 1.0);
    rig.player.set_volume(-1.0);
    assert_eq!(rig.player.volume(), 0.0);
    rig.player.set_volume(1.0);
    rig.player.play(fixture("chirp.wav"), 1);
    until("playing", || rig.rec.len() > 20_000);
    rig.player.set_volume(0.2);
    rig.wait(|e| matches!(e, Event::Ended { .. }));
    let samples = rig.rec.samples.lock().unwrap();
    // No sample may step by more than a tone at this pitch could, even where the volume changed.
    let biggest = samples.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
    assert!(biggest < 0.2, "a click of {biggest}");
    // The end is quieter than the beginning.
    let early = rms(&samples[20_000..24_000]);
    let late = rms(&samples[80_000..84_000]);
    assert!(late < early * 0.2, "{late} vs {early}");
}

#[test]
fn every_start_fades_in_instead_of_clicking() {
    let rig = rig();
    rig.play_to_end("chirp.flac", 1);
    let samples = rig.rec.samples.lock().unwrap();
    assert!(samples[0].abs() < 0.01, "{}", samples[0]);
    // The first millisecond is still well below full level.
    assert!(rms(&samples[..40]) < 0.05, "{}", rms(&samples[..40]));
}

#[test]
fn switching_files_quickly_ends_on_the_last_one() {
    let rig = rig_with(Options { speed: 30.0, ..Options::default() });
    for (i, name) in FORMATS.iter().filter(|name| !skip(name)).cycle().take(18).enumerate() {
        rig.player.play(fixture(name), i as u64 + 1);
    }
    rig.player.play(fixture("chirp.wav"), 99);
    rig.wait(|e| matches!(e, Event::Ended { generation: 99 }));
    assert_eq!(rig.player.generation(), 99);
    assert_eq!(rig.player.state(), State::Ended);
}

#[test]
fn stopping_and_starting_in_a_row_does_not_deadlock() {
    let rig = rig_with(Options { speed: 30.0, ..Options::default() });
    for i in 0..30u64 {
        rig.player.play(fixture("chirp.mp3"), i);
        rig.player.seek(Duration::from_millis(i * 40));
        rig.player.pause();
        rig.player.toggle();
        rig.player.stop();
    }
    rig.player.play(fixture("chirp.wav"), 1000);
    rig.wait(|e| matches!(e, Event::Ended { generation: 1000 }));
}

#[test]
fn commands_with_nothing_loaded_are_harmless() {
    let rig = rig();
    rig.player.pause();
    rig.player.resume();
    rig.player.toggle();
    rig.player.seek(Duration::from_secs(5));
    rig.player.seek_by(-5);
    rig.player.stop();
    rig.play_to_end("chirp.wav", 1);
    assert_eq!(rig.player.state(), State::Ended);
}

#[test]
fn a_damaged_file_ends_or_fails_cleanly_and_the_player_survives() {
    let dir = std::env::temp_dir().join(format!("rust-commander-engine-bad-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut bytes = std::fs::read(fixture("chirp.mp3")).unwrap();
    for i in (200..bytes.len()).step_by(97) {
        bytes[i] ^= 0xa5;
    }
    let path = dir.join("damaged.mp3");
    std::fs::write(&path, bytes).unwrap();
    let rig = rig();
    rig.player.play(path, 1);
    rig.wait(|e| matches!(e, Event::Ended { .. } | Event::Failed { .. }));
    rig.play_to_end("chirp.wav", 2);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dropping_the_player_stops_its_thread() {
    let rec = Record::default();
    let made = rec.clone();
    let factory: SinkFactory = Arc::new(move |rate, channels| {
        Ok(Box::new(FakeSink { rec: made.clone(), rate, channels, speed: 4.0, hardware_pause: true, fail_after: None, queued: 0 }))
    });
    let player = Player::spawn(factory, 1.0, |_| {});
    player.play(fixture("chirp.wav"), 1);
    until("started", || rec.len() > 1_000);
    drop(player);
    std::thread::sleep(Duration::from_millis(200));
    let held = rec.len();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(rec.len(), held, "the thread kept writing after the player was dropped");
}

#[test]
fn the_gain_ramp_reaches_its_target_and_clamps() {
    let mut gain = 0.0;
    let mut chunk = vec![1.0f32; 8];
    apply_gain(&mut chunk, 2, &mut gain, 1.0);
    assert!((gain - 1.0).abs() < 1e-6);
    assert!(chunk[0] < chunk[6], "{chunk:?}");
    assert!((chunk[6] - 1.0).abs() < 1e-6);
    let mut loud = vec![2.0f32, -2.0];
    let mut steady = 1.0;
    apply_gain(&mut loud, 1, &mut steady, 1.0);
    assert_eq!(loud, vec![1.0, -1.0]);
}

#[test]
fn the_whole_chain_plays_every_format_through_the_alsa_null_device() {
    if !crate::audio::alsa_available() || super::super::alsa::AlsaSink::open("null", 48_000, 2).is_err() {
        eprintln!("skipped: no ALSA library or no null device");
        return;
    }
    let factory: SinkFactory = Arc::new(|rate, channels| {
        super::super::alsa::AlsaSink::open("null", rate, channels).map(|sink| Box::new(sink) as Box<dyn Sink>)
    });
    let (tx, events) = channel();
    let tx = Mutex::new(tx);
    let player = Player::spawn(factory, 1.0, move |event| {
        let _ = tx.lock().unwrap().send(event);
    });
    for (i, name) in FORMATS.iter().chain(["stereo.opus", "surround.opus", "tagged.flac"].iter()).enumerate() {
        if skip(name) {
            continue;
        }
        let generation = i as u64 + 1;
        player.play(fixture(name), generation);
        loop {
            match events.recv_timeout(Duration::from_secs(20)).unwrap_or_else(|_| panic!("{name}: timed out")) {
                Event::Ended { generation: g } if g == generation => break,
                Event::Failed { message, .. } => panic!("{name}: {message}"),
                _ => {}
            }
        }
        assert_eq!(player.state(), State::Ended, "{name}");
    }
}

#[test]
fn the_track_ends_when_the_queued_audio_has_had_time_to_play() {
    // The device says 0.4 s is queued and keeps saying so, as PipeWire does.
    let rig = rig_with(Options { queued: 17_640, ..Options::default() });
    let started = Instant::now();
    rig.player.play(fixture("chirp.wav"), 1);
    rig.wait(|e| matches!(e, Event::Ended { .. }));
    let took = started.elapsed();
    assert!(took >= Duration::from_millis(350), "ended after only {took:?}");
    assert!(took < Duration::from_millis(1500), "waited {took:?}");
}

#[test]
fn a_device_that_claims_an_endless_queue_cannot_hold_the_track_up() {
    let rig = rig_with(Options { queued: 44_100 * 600, ..Options::default() });
    let started = Instant::now();
    rig.player.play(fixture("chirp.wav"), 1);
    rig.wait(|e| matches!(e, Event::Ended { .. }));
    assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());
}

#[test]
fn pausing_while_the_last_audio_drains_resumes_into_the_end() {
    let rig = rig_with(Options { queued: 22_050, ..Options::default() });
    rig.player.play(fixture("chirp.wav"), 1);
    until("draining", || rig.rec.len() >= 88_200);
    rig.player.pause();
    until("paused", || rig.player.state() == State::Paused);
    rig.player.resume();
    rig.wait(|e| matches!(e, Event::Ended { .. }));
    assert_eq!(rig.rec.frames(1), 88_200);
}

fn decode_mono(name: &str) -> Vec<f32> {
    let mut source = crate::audio::FileSource::open(&fixture(name)).unwrap();
    let mut out = Vec::new();
    while source.read(&mut out).unwrap() {}
    out
}

#[test]
fn the_scope_hands_out_what_was_just_played_before_the_volume() {
    let rig = rig_with(Options { speed: 12.0, ..Options::default() });
    rig.player.set_volume(0.4);
    rig.player.play(fixture("chirp.wav"), 1);
    until("some audio", || rig.rec.len() > 30_000);
    rig.player.pause();
    until("paused", || rig.player.state() == State::Paused);
    std::thread::sleep(Duration::from_millis(60));
    let mut scope = Vec::new();
    let rate = rig.player.scope(1024, &mut scope);
    assert_eq!(rate, 44_100);
    assert_eq!(scope.len(), 1024);
    // The device heard a quieter copy, and the scope shows the source itself.
    let heard = rig.player.shared.heard_frames.load(Ordering::Relaxed) as usize;
    let source = decode_mono("chirp.wav");
    let want = &source[heard - 1024..heard];
    let worst = scope.iter().zip(want).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(worst < 1e-3, "scope differs from the source by {worst}");
    let played = rig.rec.samples.lock().unwrap();
    let quiet = played[heard - 1024..heard].iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let shown = scope.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(quiet < shown * 0.2, "the device got {quiet}, the scope {shown}");
}

#[test]
fn the_scope_lags_by_what_the_device_still_holds() {
    let rig = rig_with(Options { speed: 12.0, queued: 4_410, ..Options::default() });
    rig.player.play(fixture("chirp.wav"), 1);
    until("some audio", || rig.rec.len() > 30_000);
    rig.player.pause();
    until("paused", || rig.player.state() == State::Paused);
    std::thread::sleep(Duration::from_millis(60));
    let mut scope = Vec::new();
    rig.player.scope(256, &mut scope);
    let heard = rig.player.shared.heard_frames.load(Ordering::Relaxed) as usize;
    let written = rig.rec.len();
    assert_eq!(written - heard, 4_410, "the clock must trail the writes by the queue");
    let source = decode_mono("chirp.wav");
    let worst = scope.iter().zip(&source[heard - 256..heard]).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(worst < 1e-3, "{worst}");
}

#[test]
fn the_scope_is_short_at_the_start_and_empty_when_nothing_plays() {
    let rig = rig();
    let mut scope = vec![1.0; 10];
    assert_eq!(rig.player.scope(512, &mut scope), 0);
    assert!(scope.is_empty());
    let rig = rig_with(Options { speed: 3.0, ..Options::default() });
    rig.player.play(fixture("chirp.wav"), 1);
    until("started", || rig.rec.len() > 1_000);
    rig.player.pause();
    until("paused", || rig.player.state() == State::Paused);
    std::thread::sleep(Duration::from_millis(40));
    let mut scope = Vec::new();
    rig.player.scope(100_000, &mut scope);
    assert!(!scope.is_empty() && scope.len() < 100_000, "{}", scope.len());
    rig.player.stop();
    until("idle", || rig.player.state() == State::Idle);
    assert_eq!(rig.player.scope(512, &mut scope), 0);
    assert!(scope.is_empty());
}

#[test]
fn a_seek_clears_the_scope_until_new_sound_arrives() {
    let rig = rig_with(Options { speed: 12.0, ..Options::default() });
    rig.player.play(fixture("chirp.flac"), 1);
    until("started", || rig.rec.len() > 20_000);
    rig.player.pause();
    until("paused", || rig.player.state() == State::Paused);
    std::thread::sleep(Duration::from_millis(40));
    rig.player.seek(Duration::from_millis(1500));
    std::thread::sleep(Duration::from_millis(80));
    let mut scope = Vec::new();
    rig.player.scope(512, &mut scope);
    assert!(scope.is_empty(), "the scope still shows the old place: {} samples", scope.len());
    rig.player.resume();
    until("new sound", || {
        let mut s = Vec::new();
        rig.player.scope(512, &mut s);
        s.len() == 512
    });
    // What it shows now belongs to the new place, so it matches the source from there.
    rig.player.pause();
    until("paused again", || rig.player.state() == State::Paused);
    std::thread::sleep(Duration::from_millis(40));
    rig.player.scope(256, &mut scope);
    let heard = rig.player.shared.heard_frames.load(Ordering::Relaxed) as usize;
    assert!(heard >= 66_150, "{heard}");
    let source = decode_mono("chirp.flac");
    let worst = scope.iter().zip(&source[heard - 256..heard]).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(worst < 1e-3, "{worst}");
}

#[test]
fn the_scope_mixes_stereo_down_to_mono() {
    let rig = rig_with(Options { speed: 12.0, ..Options::default() });
    rig.player.play(fixture("stereo.flac"), 1);
    until("started", || rig.rec.len() > 20_000);
    rig.player.pause();
    until("paused", || rig.player.state() == State::Paused);
    std::thread::sleep(Duration::from_millis(40));
    let mut scope = Vec::new();
    assert_eq!(rig.player.scope(512, &mut scope), 16_000);
    assert_eq!(scope.len(), 512);
    let mut source = crate::audio::FileSource::open(&fixture("stereo.flac")).unwrap();
    let mut stereo = Vec::new();
    while source.read(&mut stereo).unwrap() {}
    let heard = rig.player.shared.heard_frames.load(Ordering::Relaxed) as usize;
    let worst = (0..512).map(|i| {
        let at = heard - 512 + i;
        ((stereo[at * 2] + stereo[at * 2 + 1]) / 2.0 - scope[i]).abs()
    }).fold(0.0f32, f32::max);
    assert!(worst < 1e-3, "{worst}");
}

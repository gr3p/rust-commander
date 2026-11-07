use std::panic::{AssertUnwindSafe, catch_unwind};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::{Duration, Instant};

use super::sink::{Sink, SinkFactory};
use super::source::{FileSource, OpenError, TrackInfo};

/// Frames written between two looks at the command queue: about 21 ms at 48 kHz.
const CHUNK_FRAMES: usize = 1024;
/// The most a finished track waits for its last audio to leave the device. A device that
/// reports a huge queue must not hold the track up for long.
const DRAIN_LIMIT: Duration = Duration::from_secs(2);
const NO_DURATION: u64 = u64::MAX;
pub const DEFAULT_VOLUME: f32 = 0.8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Nothing loaded.
    Idle,
    Playing,
    Paused,
    /// The track played to its end. Play starts it again.
    Ended,
    /// The file or the device failed. The reason went out as an event.
    Failed,
}

impl State {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Playing,
            2 => Self::Paused,
            3 => Self::Ended,
            4 => Self::Failed,
            _ => Self::Idle,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Event {
    /// The file opened. Tags and cover are ready, and playing starts at once.
    Loaded { generation: u64, info: Arc<TrackInfo> },
    Failed { generation: u64, message: String, unsupported: bool },
    Ended { generation: u64 },
}

enum Command {
    Load { path: PathBuf, generation: u64 },
    Pause,
    Resume,
    Toggle,
    Stop,
    Seek(Duration),
    SeekBy(i64),
}

struct Shared {
    state: AtomicU8,
    generation: AtomicU64,
    position_ms: AtomicU64,
    // The same moment as position_ms, in frames, for lining the scope up with what is heard.
    heard_frames: AtomicU64,
    duration_ms: AtomicU64,
    volume: AtomicU32,
    muted: AtomicBool,
}

/// The newest decoded sound, mixed to mono and taken before the volume, for the visualizer.
struct Tap {
    inner: Mutex<TapInner>,
}

struct TapInner {
    samples: VecDeque<f32>,
    // Timeline frame just after the newest sample.
    end: u64,
    rate: u32,
}

/// Enough for the longest lag a sound card reports, plus the window the analyzer reads.
const TAP_FRAMES: usize = 16_384;

impl Tap {
    fn new() -> Self {
        Self { inner: Mutex::new(TapInner { samples: VecDeque::with_capacity(TAP_FRAMES), end: 0, rate: 0 }) }
    }

    /// A new track, a seek or a stop: what was in there no longer leads up to the next sound.
    fn reset(&self, rate: u32, at: u64) {
        if let Ok(mut tap) = self.inner.lock() {
            tap.samples.clear();
            tap.end = at;
            tap.rate = rate;
        }
    }

    /// `chunk` is interleaved in `width` channels, `end` the timeline frame after its last frame.
    fn push(&self, chunk: &[f32], width: usize, end: u64) {
        let Ok(mut tap) = self.inner.lock() else { return };
        for frame in chunk.chunks_exact(width) {
            let mono = frame.iter().sum::<f32>() / width as f32;
            if tap.samples.len() == TAP_FRAMES {
                tap.samples.pop_front();
            }
            tap.samples.push_back(mono);
        }
        tap.end = end;
    }

    /// The `want` samples up to frame `until`, or as many of them as there are.
    fn window(&self, until: u64, want: usize, out: &mut Vec<f32>) -> u32 {
        out.clear();
        let Ok(tap) = self.inner.lock() else { return 0 };
        let first = tap.end.saturating_sub(tap.samples.len() as u64);
        let until = until.clamp(first, tap.end);
        let from = until.saturating_sub(want as u64).max(first);
        let (skip, take) = ((from - first) as usize, (until - from) as usize);
        out.extend(tap.samples.iter().skip(skip).take(take));
        tap.rate
    }
}

/// The player handle. Cheap to call from the UI thread: every call only queues a command.
pub struct Player {
    tx: Sender<Command>,
    shared: Arc<Shared>,
    tap: Arc<Tap>,
}

impl Player {
    /// `events` runs on the player thread, so it should only hand the event on.
    pub fn spawn(factory: SinkFactory, volume: f32, events: impl Fn(Event) + Send + 'static) -> Player {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            state: AtomicU8::new(State::Idle as u8),
            generation: AtomicU64::new(0),
            position_ms: AtomicU64::new(0),
            heard_frames: AtomicU64::new(0),
            duration_ms: AtomicU64::new(NO_DURATION),
            volume: AtomicU32::new(volume.clamp(0.0, 1.0).to_bits()),
            muted: AtomicBool::new(false),
        });
        let tap = Arc::new(Tap::new());
        let engine = Engine {
            rx,
            shared: Arc::clone(&shared),
            tap: Arc::clone(&tap),
            factory,
            emit: Box::new(events),
            play: None,
            last: None,
            gain: 0.0,
        };
        std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || engine.run())
            .expect("the audio thread starts");
        Player { tx, shared, tap }
    }

    pub fn play(&self, path: PathBuf, generation: u64) {
        // Stale events are told apart by generation, so claim it before the thread gets to it.
        self.shared.generation.store(generation, Ordering::Relaxed);
        let _ = self.tx.send(Command::Load { path, generation });
    }

    pub fn pause(&self) {
        let _ = self.tx.send(Command::Pause);
    }

    pub fn resume(&self) {
        let _ = self.tx.send(Command::Resume);
    }

    /// Pauses a playing track, resumes a paused one, restarts one that ended.
    pub fn toggle(&self) {
        let _ = self.tx.send(Command::Toggle);
    }

    pub fn stop(&self) {
        let _ = self.tx.send(Command::Stop);
    }

    pub fn seek(&self, to: Duration) {
        let _ = self.tx.send(Command::Seek(to));
    }

    pub fn seek_by(&self, seconds: i64) {
        let _ = self.tx.send(Command::SeekBy(seconds * 1000));
    }

    /// 0.0 to 1.0, on a perceptual curve.
    pub fn set_volume(&self, volume: f32) {
        self.shared.volume.store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn set_muted(&self, muted: bool) {
        self.shared.muted.store(muted, Ordering::Relaxed);
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.shared.volume.load(Ordering::Relaxed))
    }

    pub fn muted(&self) -> bool {
        self.shared.muted.load(Ordering::Relaxed)
    }

    pub fn state(&self) -> State {
        State::from_u8(self.shared.state.load(Ordering::Relaxed))
    }

    /// The newest `want` samples that have reached the speakers, mixed to mono and taken before the
    /// volume, so the picture does not shrink with it. Returns the sample rate, or 0 when nothing
    /// plays. Fewer samples than asked for come back at the start of a track.
    pub fn scope(&self, want: usize, out: &mut Vec<f32>) -> u32 {
        if self.state() != State::Playing && self.state() != State::Paused {
            out.clear();
            return 0;
        }
        self.tap.window(self.shared.heard_frames.load(Ordering::Relaxed), want, out)
    }

    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Relaxed)
    }

    pub fn position(&self) -> Duration {
        Duration::from_millis(self.shared.position_ms.load(Ordering::Relaxed))
    }

    pub fn duration(&self) -> Option<Duration> {
        match self.shared.duration_ms.load(Ordering::Relaxed) {
            NO_DURATION => None,
            ms => Some(Duration::from_millis(ms)),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Playing,
    /// The last audio is still leaving the device. PipeWire never reports an empty queue, so the
    /// time left is taken from what was queued when the file ended.
    Draining { since: Instant, wait: Duration },
    Paused {
        /// The device holds its queue. Otherwise the queue was flushed, and resume seeks back.
        hardware: bool,
    },
}

struct Playback {
    generation: u64,
    source: FileSource,
    sink: Box<dyn Sink>,
    rate: u32,
    channels: usize,
    // Decoded frames not yet handed to the device.
    pending: Vec<f32>,
    cursor: usize,
    eof: bool,
    // Timeline position of the first frame written since the last restart, and how many followed.
    start_frames: u64,
    written: u64,
    phase: Phase,
    // Where to seek on resume when the device could not hold its queue.
    resync: Option<u64>,
}

impl Playback {
    fn heard_frames(&mut self) -> u64 {
        let queued = self.sink.delay();
        (self.start_frames + self.written).saturating_sub(queued).max(self.start_frames)
    }

    fn restart_clock(&mut self) {
        self.pending.clear();
        self.cursor = 0;
        self.eof = false;
        self.start_frames = self.source.position_frames();
        self.written = 0;
    }
}

struct Engine {
    rx: Receiver<Command>,
    shared: Arc<Shared>,
    tap: Arc<Tap>,
    factory: SinkFactory,
    emit: Box<dyn Fn(Event) + Send>,
    play: Option<Playback>,
    // The track that last played, so Play can start it over once it has ended.
    last: Option<(PathBuf, u64)>,
    // Where the volume ramp stands. A restart begins at silence and fades in over one chunk.
    gain: f32,
}

enum Wait {
    None,
    Block,
    Poll(Duration),
}

impl Engine {
    fn run(mut self) {
        loop {
            let wait = match self.play.as_ref().map(|p| p.phase) {
                None => Wait::Block,
                Some(Phase::Playing) => Wait::None,
                Some(Phase::Draining { .. }) => Wait::Poll(Duration::from_millis(20)),
                Some(Phase::Paused { .. }) => Wait::Block,
            };
            let command = match wait {
                Wait::None => match self.rx.try_recv() {
                    Ok(command) => Some(command),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => return,
                },
                Wait::Block => match self.rx.recv() {
                    Ok(command) => Some(command),
                    Err(_) => return,
                },
                Wait::Poll(timeout) => match self.rx.recv_timeout(timeout) {
                    Ok(command) => Some(command),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                },
            };
            match command {
                Some(command) => self.handle(command),
                None => self.step(),
            }
        }
    }

    fn set_state(&self, state: State) {
        self.shared.state.store(state as u8, Ordering::Relaxed);
    }

    fn set_position(&self, frames: u64, rate: u32) {
        let ms = frames.saturating_mul(1000) / u64::from(rate.max(1));
        self.shared.position_ms.store(ms, Ordering::Relaxed);
        self.shared.heard_frames.store(frames, Ordering::Relaxed);
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Load { path, generation } => self.load(path, generation, None),
            Command::Stop => {
                self.play = None;
                self.last = None;
                self.tap.reset(0, 0);
                self.shared.position_ms.store(0, Ordering::Relaxed);
                self.shared.duration_ms.store(NO_DURATION, Ordering::Relaxed);
                self.set_state(State::Idle);
            }
            Command::Pause => self.pause(),
            Command::Resume => self.resume(),
            Command::Toggle => match State::from_u8(self.shared.state.load(Ordering::Relaxed)) {
                State::Playing => self.pause(),
                State::Paused | State::Ended => self.resume(),
                State::Idle | State::Failed => {}
            },
            Command::Seek(to) => self.seek(to),
            Command::SeekBy(ms) => {
                let now = self.shared.position_ms.load(Ordering::Relaxed) as i64;
                let mut target = (now + ms).max(0) as u64;
                if let Some(total) = self.duration_ms() {
                    target = target.min(total);
                }
                self.seek(Duration::from_millis(target));
            }
        }
    }

    fn duration_ms(&self) -> Option<u64> {
        match self.shared.duration_ms.load(Ordering::Relaxed) {
            NO_DURATION => None,
            ms => Some(ms),
        }
    }

    fn fail(&mut self, generation: u64, message: String, unsupported: bool) {
        self.play = None;
        self.tap.reset(0, 0);
        self.set_state(State::Failed);
        (self.emit)(Event::Failed { generation, message, unsupported });
    }

    // `then` seeks right after opening, for a seek on a track that had ended.
    fn load(&mut self, path: PathBuf, generation: u64, then: Option<Duration>) {
        let old = self.play.take();
        self.shared.generation.store(generation, Ordering::Relaxed);
        self.shared.position_ms.store(0, Ordering::Relaxed);
        self.shared.duration_ms.store(NO_DURATION, Ordering::Relaxed);
        self.set_state(State::Idle);
        self.last = Some((path.clone(), generation));

        let opened = catch_unwind(AssertUnwindSafe(|| FileSource::open(&path)));
        let source = match opened {
            Ok(Ok(source)) => source,
            Ok(Err(err)) => {
                let unsupported = matches!(err, OpenError::Unsupported(_));
                self.fail(generation, err.to_string(), unsupported);
                return;
            }
            Err(_) => {
                self.fail(generation, "The player could not make sense of this file".into(), false);
                return;
            }
        };
        let info = Arc::new(source.info().clone());
        let total = info.duration.map_or(NO_DURATION, |d| d.as_millis() as u64);
        self.shared.duration_ms.store(total, Ordering::Relaxed);
        (self.emit)(Event::Loaded { generation, info });

        let (rate, channels) = (source.sample_rate(), source.channels());
        // A device that already fits is kept, which saves reopening it for every track.
        let reused = old.and_then(|p| (p.rate == rate && p.channels == channels).then_some(p.sink));
        let sink = match reused {
            Some(mut sink) => {
                sink.flush();
                sink
            }
            None => match (self.factory)(rate, channels) {
                Ok(sink) => sink,
                Err(message) => {
                    self.fail(generation, message, false);
                    return;
                }
            },
        };
        self.play = Some(Playback {
            generation,
            source,
            sink,
            rate,
            channels,
            pending: Vec::new(),
            cursor: 0,
            eof: false,
            start_frames: 0,
            written: 0,
            phase: Phase::Playing,
            resync: None,
        });
        self.gain = 0.0;
        self.tap.reset(rate, 0);
        self.set_state(State::Playing);
        if let Some(to) = then {
            self.seek(to);
        }
    }

    fn pause(&mut self) {
        let Some(play) = self.play.as_mut() else { return };
        if matches!(play.phase, Phase::Paused { .. }) {
            return;
        }
        let heard = play.heard_frames();
        let hardware = play.sink.pause(true);
        if !hardware {
            play.sink.flush();
            play.pending.clear();
            play.cursor = 0;
            play.resync = Some(heard);
        }
        play.phase = Phase::Paused { hardware };
        let rate = play.rate;
        self.set_state(State::Paused);
        self.set_position(heard, rate);
    }

    fn resume(&mut self) {
        let Some(play) = self.play.as_mut() else {
            // Nothing is loaded, but a track that ended plays again.
            if State::from_u8(self.shared.state.load(Ordering::Relaxed)) == State::Ended
                && let Some((path, generation)) = self.last.clone()
            {
                self.load(path, generation, None);
            }
            return;
        };
        let Phase::Paused { hardware } = play.phase else { return };
        if hardware {
            play.sink.pause(false);
        } else if let Some(frames) = play.resync.take() {
            let to = Duration::from_secs_f64(frames as f64 / f64::from(play.rate.max(1)));
            let _ = catch_unwind(AssertUnwindSafe(|| play.source.seek(to)));
            play.restart_clock();
            self.gain = 0.0;
            self.tap.reset(play.rate, play.start_frames);
        }
        play.phase = Phase::Playing;
        self.set_state(State::Playing);
    }

    fn seek(&mut self, to: Duration) {
        let Some(play) = self.play.as_mut() else {
            if State::from_u8(self.shared.state.load(Ordering::Relaxed)) == State::Ended
                && let Some((path, generation)) = self.last.clone()
            {
                self.load(path, generation, Some(to));
            }
            return;
        };
        play.sink.flush();
        let outcome = catch_unwind(AssertUnwindSafe(|| play.source.seek(to)));
        play.resync = None;
        play.restart_clock();
        self.gain = 0.0;
        self.tap.reset(play.rate, play.start_frames);
        let paused = matches!(play.phase, Phase::Paused { .. });
        // Whatever the device held is gone, so a resume has nothing to release.
        play.phase = if paused { Phase::Paused { hardware: true } } else { Phase::Playing };
        let (frames, rate, generation) = (play.start_frames, play.rate, play.generation);
        self.set_position(frames, rate);
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(message)) => self.fail(generation, message, false),
            Err(_) => self.fail(generation, "The decoder crashed on this file".into(), false),
        }
    }

    fn target_gain(&self) -> f32 {
        if self.shared.muted.load(Ordering::Relaxed) {
            return 0.0;
        }
        let volume = f32::from_bits(self.shared.volume.load(Ordering::Relaxed));
        volume * volume * volume
    }

    fn step(&mut self) {
        // The playback is taken out while it runs, so a failure can reset the engine.
        let Some(mut play) = self.play.take() else { return };
        match self.advance(&mut play) {
            Step::Continue => self.play = Some(play),
            Step::Finished => self.finish(play.generation),
            Step::Failed(message) => self.fail(play.generation, message, false),
        }
    }

    fn advance(&mut self, play: &mut Playback) -> Step {
        let rate = play.rate;
        match play.phase {
            Phase::Paused { .. } => return Step::Continue,
            Phase::Draining { since, wait } => {
                let left = wait.saturating_sub(since.elapsed());
                if left.is_zero() {
                    return Step::Finished;
                }
                let queued = (left.as_secs_f64() * f64::from(rate)) as u64;
                let heard = (play.start_frames + play.written).saturating_sub(queued).max(play.start_frames);
                self.set_position(heard, rate);
                return Step::Continue;
            }
            Phase::Playing => {}
        }

        if play.cursor >= play.pending.len() {
            if play.eof {
                play.phase = drain_phase(play);
                return Step::Continue;
            }
            play.pending.clear();
            play.cursor = 0;
            let read = catch_unwind(AssertUnwindSafe(|| play.source.read(&mut play.pending)));
            match read {
                Ok(Ok(true)) => {}
                Ok(Ok(false)) => {
                    play.eof = true;
                    play.phase = drain_phase(play);
                    return Step::Continue;
                }
                Ok(Err(message)) => return Step::Failed(message),
                Err(_) => return Step::Failed("The decoder crashed on this file".into()),
            }
        }

        let width = play.channels;
        let end = (play.cursor + CHUNK_FRAMES * width).min(play.pending.len());
        let end = end - (end - play.cursor) % width;
        if end == play.cursor {
            // A stray partial frame. There is nothing to play in it.
            play.cursor = play.pending.len();
            return Step::Continue;
        }
        let target = self.target_gain();
        let chunk = &mut play.pending[play.cursor..end];
        let frames = (chunk.len() / width) as u64;
        // The visualizer sees the sound before the volume, so a quiet setting does not flatten it.
        self.tap.push(chunk, width, play.start_frames + play.written + frames);
        apply_gain(chunk, width, &mut self.gain, target);
        if let Err(message) = play.sink.write(chunk) {
            return Step::Failed(message);
        }
        play.cursor = end;
        play.written += frames;
        let heard = play.heard_frames();
        self.set_position(heard, rate);
        Step::Continue
    }

    fn finish(&mut self, generation: u64) {
        // The device closes with the playback. A new track opens it again.
        if let Some(ms) = self.duration_ms() {
            self.shared.position_ms.store(ms, Ordering::Relaxed);
        }
        self.set_state(State::Ended);
        (self.emit)(Event::Ended { generation });
    }
}

enum Step {
    Continue,
    Finished,
    Failed(String),
}

fn drain_phase(play: &mut Playback) -> Phase {
    let queued = play.sink.delay();
    let wait = Duration::from_secs_f64(queued as f64 / f64::from(play.rate.max(1))).min(DRAIN_LIMIT);
    Phase::Draining { since: Instant::now(), wait }
}

/// Steps the volume across the chunk instead of at its edge, so a change makes no click.
fn apply_gain(chunk: &mut [f32], width: usize, gain: &mut f32, target: f32) {
    let start = *gain;
    if (start - target).abs() < 1e-5 {
        if (target - 1.0).abs() > 1e-5 {
            for sample in chunk.iter_mut() {
                *sample *= target;
            }
        }
    } else {
        let frames = (chunk.len() / width).max(1) as f32;
        for (i, frame) in chunk.chunks_exact_mut(width).enumerate() {
            let g = start + (target - start) * ((i + 1) as f32 / frames);
            for sample in frame {
                *sample *= g;
            }
        }
        *gain = target;
    }
    for sample in chunk.iter_mut() {
        *sample = sample.clamp(-1.0, 1.0);
    }
}

#[cfg(test)]
mod tests;

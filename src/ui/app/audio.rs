use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use relm4::gtk;
use relm4::prelude::*;

use rust_commander::audio::{self, DEFAULT_VOLUME, Event, Player, State, TrackInfo};
use rust_commander::imageview;
use rust_commander::lister::PreviewBody;

use crate::ui::msg::Msg;
use crate::ui::player::Look;

use super::App;

/// The quick view waits this long on a file before playing it, so scrolling through a folder is quiet.
const START_DELAY: Duration = Duration::from_millis(220);
/// Seeks from the slider go out at most this often while it is dragged.
const SEEK_GAP: Duration = Duration::from_millis(90);
/// After a seek the slider shows where it was put, until the player has caught up.
const SCRUB_HOLD: Duration = Duration::from_millis(500);
const KEY_SEEK: i64 = 5;
const VOLUME_STEP: f32 = 0.05;
const COVER_EDGE: u32 = 640;

pub(in crate::ui::app) struct AudioState {
    player: Option<Player>,
    volume: f32,
    muted: bool,
    /// The file the quick view wants played. None when it shows something else.
    target: Option<PathBuf>,
    /// Counts every change of target, so a late event for an old file is recognised and dropped.
    generation: u64,
    started: bool,
    info: Option<Arc<TrackInfo>>,
    /// What went wrong, and whether the format is simply one the player does not read.
    problem: Option<(String, bool)>,
    cover: Option<(u32, u32, Vec<u8>)>,
    cover_rev: u64,
    seek_pending: Option<Duration>,
    seek_sent: Instant,
    scrub: Option<(Duration, Instant)>,
}

impl AudioState {
    pub(in crate::ui::app) fn new() -> Self {
        Self {
            player: None,
            volume: DEFAULT_VOLUME,
            muted: false,
            target: None,
            generation: 0,
            started: false,
            info: None,
            problem: None,
            cover: None,
            cover_rev: 0,
            seek_pending: None,
            seek_sent: Instant::now(),
            scrub: None,
        }
    }
}

impl App {
    /// The audio file the quick view is on, if it is on one.
    fn audio_wanted(&self) -> Option<PathBuf> {
        if !self.state.quick_view || self.shell.editing.get() || self.state.panel_terminal {
            return None;
        }
        let path = self.file_under_cursor()?;
        let name = path.file_name()?.to_str()?;
        audio::can_try(name).then_some(path)
    }

    /// Whether the player pane is up and owns Space, the arrows and the volume keys.
    pub(in crate::ui::app) fn audio_keys(&self) -> bool {
        self.audio.target.is_some()
    }

    /// Follows the cursor: a new audio file starts after a moment, anything else stops the music.
    pub(in crate::ui::app) fn reconcile_audio(&mut self, sender: &ComponentSender<Self>) {
        let want = self.audio_wanted();
        if want == self.audio.target {
            return;
        }
        self.audio.generation += 1;
        self.audio.target = want.clone();
        self.audio.started = false;
        self.audio.info = None;
        self.audio.problem = None;
        self.audio.seek_pending = None;
        self.audio.scrub = None;
        self.set_cover(None);
        if let Some(player) = &self.audio.player {
            player.stop();
        }
        if want.is_some() {
            let generation = self.audio.generation;
            let tx = sender.input_sender().clone();
            gtk::glib::timeout_add_local_once(START_DELAY, move || tx.emit(Msg::AudioStart(generation)));
        }
    }

    /// Starts the wanted file at once, without the pause that scrolling gets.
    pub(in crate::ui::app) fn start_audio_now(&mut self, sender: &ComponentSender<Self>) {
        self.start_audio(self.audio.generation, sender);
    }

    pub(in crate::ui::app) fn start_audio(&mut self, generation: u64, sender: &ComponentSender<Self>) {
        if generation != self.audio.generation || self.audio.started {
            return;
        }
        let Some(path) = self.audio.target.clone() else { return };
        self.audio.started = true;
        self.audio.problem = None;
        let (volume, muted) = (self.audio.volume, self.audio.muted);
        let player = self.audio.player.get_or_insert_with(|| {
            let tx = sender.input_sender().clone();
            let player = Player::spawn(audio::system_sink(), volume, move |event| tx.emit(Msg::AudioEvent(event)));
            player.set_muted(muted);
            player
        });
        player.play(path, generation);
    }

    pub(in crate::ui::app) fn on_audio_event(&mut self, event: Event, sender: &ComponentSender<Self>) {
        match event {
            Event::Loaded { generation, info } if generation == self.audio.generation => {
                self.audio.problem = None;
                let cover = info.cover.clone();
                self.audio.info = Some(info);
                if let Some(cover) = cover {
                    // A large cover takes a moment to decode, so it does not hold up the music.
                    let tx = sender.input_sender().clone();
                    std::thread::spawn(move || {
                        let image = imageview::decode_bytes(&cover.data, COVER_EDGE).ok().map(|img| (img.width, img.height, img.pixels));
                        tx.emit(Msg::AudioCover { generation, image });
                    });
                }
            }
            Event::Failed { generation, message, unsupported } if generation == self.audio.generation => {
                self.audio.problem = Some((message, unsupported));
            }
            _ => {}
        }
    }

    pub(in crate::ui::app) fn on_audio_cover(&mut self, generation: u64, image: Option<(u32, u32, Vec<u8>)>) {
        if generation == self.audio.generation {
            self.set_cover(image);
        }
    }

    fn set_cover(&mut self, image: Option<(u32, u32, Vec<u8>)>) {
        self.audio.cover = image;
        self.audio.cover_rev += 1;
    }

    fn audio_look(&self) -> Look {
        let Some(path) = &self.audio.target else { return Look::default() };
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let stem = path.file_stem().and_then(|n| n.to_str()).unwrap_or(name);
        let info = self.audio.info.as_deref();
        let (state, position, duration) = match &self.audio.player {
            Some(player) if self.audio.started && info.is_some() => {
                let held = self.audio.scrub.filter(|(_, until)| Instant::now() < *until).map(|(to, _)| to);
                (player.state(), held.unwrap_or_else(|| player.position()), player.duration())
            }
            _ => (State::Idle, Duration::ZERO, info.and_then(|i| i.duration)),
        };
        let message = match &self.audio.problem {
            Some((_, true)) => "The built-in player cannot read this format. Press Enter to open it with the default player.".to_string(),
            Some((text, false)) => text.clone(),
            None => String::new(),
        };
        Look {
            title: info.and_then(|i| i.title.clone()).unwrap_or_else(|| stem.to_string()),
            artist: info.and_then(|i| i.artist.clone()).unwrap_or_default(),
            album: info.and_then(|i| i.album.clone()).unwrap_or_default(),
            detail: info.map(audio::describe).unwrap_or_default(),
            message,
            playing: state == State::Playing,
            can_toggle: matches!(state, State::Playing | State::Paused | State::Ended),
            position_ms: position.as_millis() as u64,
            duration_ms: duration.map(|d| d.as_millis() as u64),
            volume: self.audio.volume,
            muted: self.audio.muted,
        }
    }

    pub(in crate::ui::app) fn audio_player_state(&self) -> Option<State> {
        self.audio.player.as_ref().map(Player::state)
    }

    pub(in crate::ui::app) fn audio_volume(&self) -> f32 {
        self.audio.volume
    }

    /// Paints the player into the pane that shows the preview. Cheap when nothing changed.
    pub(in crate::ui::app) fn paint_audio(&self) {
        if !matches!(self.preview.body, PreviewBody::Audio) || self.audio.target.is_none() {
            return;
        }
        let pane = self.state.active.other().index();
        let view = &self.shell.panes[pane].player;
        view.set_cover(self.audio.cover_rev, self.audio.cover.as_ref().map(|(w, h, p)| (*w, *h, p.as_slice())));
        view.paint(&self.audio_look());
    }

    /// Runs on the 50 ms poll: sends a held-back seek and keeps the clock moving.
    pub(in crate::ui::app) fn tick_audio(&mut self) {
        if self.audio.target.is_none() {
            return;
        }
        if let Some(to) = self.audio.seek_pending
            && self.audio.seek_sent.elapsed() >= SEEK_GAP
        {
            self.audio.seek_pending = None;
            self.audio.seek_sent = Instant::now();
            if let Some(player) = &self.audio.player {
                player.seek(to);
            }
        }
        self.paint_audio();
    }

    /// Space, the Play button and the media key: pause, resume, start over, or try again.
    pub(in crate::ui::app) fn audio_toggle(&mut self, sender: &ComponentSender<Self>) {
        let Some(player) = &self.audio.player else {
            self.start_audio_now(sender);
            return;
        };
        if !self.audio.started {
            self.start_audio_now(sender);
            return;
        }
        match player.state() {
            State::Playing | State::Paused | State::Ended => player.toggle(),
            // A failed or idle player runs the file again, which is also what retrying means.
            State::Idle | State::Failed => {
                if let Some(path) = self.audio.target.clone() {
                    self.audio.problem = None;
                    player.play(path, self.audio.generation);
                }
            }
        }
    }

    pub(in crate::ui::app) fn audio_stop(&mut self) {
        if let Some(player) = &self.audio.player {
            player.stop();
        }
        self.audio.started = true;
    }

    pub(in crate::ui::app) fn audio_seek_by(&mut self, seconds: i64) {
        if let Some(player) = &self.audio.player {
            player.seek_by(seconds);
        }
    }

    /// The slider moved to a fraction of the track.
    pub(in crate::ui::app) fn audio_seek_to(&mut self, fraction: f64) {
        let Some(total) = self.audio.player.as_ref().and_then(|p| p.duration()) else { return };
        let to = total.mul_f64(fraction.clamp(0.0, 1.0));
        self.audio.scrub = Some((to, Instant::now() + SCRUB_HOLD));
        if self.audio.seek_sent.elapsed() >= SEEK_GAP {
            self.audio.seek_sent = Instant::now();
            self.audio.seek_pending = None;
            if let Some(player) = &self.audio.player {
                player.seek(to);
            }
        } else {
            self.audio.seek_pending = Some(to);
        }
    }

    pub(in crate::ui::app) fn audio_set_volume(&mut self, volume: f32) {
        self.audio.volume = volume.clamp(0.0, 1.0);
        // Moving the volume up from silence is a wish to hear it.
        if self.audio.volume > 0.0 {
            self.audio.muted = false;
        }
        if let Some(player) = &self.audio.player {
            player.set_volume(self.audio.volume);
            player.set_muted(self.audio.muted);
        }
    }

    pub(in crate::ui::app) fn audio_volume_step(&mut self, delta: f32) {
        // Rounded to the step, so repeated presses land on even numbers.
        let next = ((self.audio.volume + delta) / VOLUME_STEP).round() * VOLUME_STEP;
        self.audio_set_volume(next);
    }

    pub(in crate::ui::app) fn audio_toggle_mute(&mut self) {
        self.audio.muted = !self.audio.muted;
        if let Some(player) = &self.audio.player {
            player.set_muted(self.audio.muted);
        }
    }

    pub(in crate::ui::app) fn audio_key_seek(&mut self, forward: bool, big: bool) {
        let step = if big { KEY_SEEK * 6 } else { KEY_SEEK };
        self.audio_seek_by(if forward { step } else { -step });
    }
}

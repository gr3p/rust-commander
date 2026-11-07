use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use relm4::gtk;
use relm4::gtk::gdk::{Key, ModifierType};
use relm4::gtk::prelude::*;
use relm4::prelude::*;

use rust_commander::audio::{self, DEFAULT_VOLUME, Event, Player, Playlist, Repeat, State, TrackInfo, VisualStyle};
use rust_commander::config;
use rust_commander::imageview;
use rust_commander::listing;
use rust_commander::lister::PreviewBody;
use rust_commander::state::PaneId;

use crate::ui::msg::Msg;
use crate::ui::player::Look;

use super::App;

/// The quick view waits this long on a file before playing it, so scrolling through a folder is quiet.
const START_DELAY: Duration = Duration::from_millis(220);
/// Seeks from the slider go out at most this often while it is dragged.
const SEEK_GAP: Duration = Duration::from_millis(90);
/// After a seek the slider shows where it was put, until the player has caught up.
const SCRUB_HOLD: Duration = Duration::from_millis(500);
/// Previous inside this time goes to the track before. Later it plays the track again.
const RESTART_AFTER: Duration = Duration::from_secs(3);
/// Expand gives up waiting for the folder to load after this long.
const EXPAND_WAIT: Duration = Duration::from_secs(4);
const KEY_SEEK: i64 = 5;
const VOLUME_STEP: f32 = 0.05;
const COVER_EDGE: u32 = 640;

/// Full lives in the quick view and follows the cursor. Mini is a bar under a file list and plays on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui::app) enum Mode {
    Full,
    Mini,
}

/// What plays and in which order: the music files of the folder where the song started.
struct Session {
    list: Playlist,
    folder: PathBuf,
    source: PaneId,
    mode: Mode,
}

pub(in crate::ui::app) struct AudioState {
    player: Option<Player>,
    volume: f32,
    muted: bool,
    session: Option<Session>,
    /// The track that plays or is about to. None when there is no session.
    target: Option<PathBuf>,
    /// Counts every change of track, so a late event for an old file is recognised and dropped.
    generation: u64,
    started: bool,
    info: Option<Arc<TrackInfo>>,
    /// What went wrong, and whether the format is simply one the player does not read.
    problem: Option<(String, bool)>,
    cover: Option<(u32, u32, Vec<u8>)>,
    cover_rev: u64,
    /// The picture file of a folder cover. None for a cover inside the track.
    cover_key: Option<PathBuf>,
    seek_pending: Option<Duration>,
    seek_sent: Instant,
    scrub: Option<(Duration, Instant)>,
    /// The mini player has the keyboard.
    focus: bool,
    /// Expand is waiting for the folder to show the track.
    expanding: Option<Instant>,
    /// The track came from next or from the end of the one before, so one that cannot play is skipped.
    skipping: bool,
    /// Tracks in a row that failed. A folder of broken files must not loop for ever.
    failures: usize,
}

impl AudioState {
    pub(in crate::ui::app) fn new() -> Self {
        Self {
            player: None,
            volume: DEFAULT_VOLUME,
            muted: false,
            session: None,
            target: None,
            generation: 0,
            started: false,
            info: None,
            problem: None,
            cover: None,
            cover_rev: 0,
            cover_key: None,
            seek_pending: None,
            seek_sent: Instant::now(),
            scrub: None,
            focus: false,
            expanding: None,
            skipping: false,
            failures: 0,
        }
    }
}

fn seed() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(1, |time| time.as_nanos() as u64)
}

fn is_modifier(key: Key) -> bool {
    matches!(
        key,
        Key::Shift_L
            | Key::Shift_R
            | Key::Control_L
            | Key::Control_R
            | Key::Alt_L
            | Key::Alt_R
            | Key::Meta_L
            | Key::Meta_R
            | Key::Super_L
            | Key::Super_R
            | Key::ISO_Level3_Shift
            | Key::Caps_Lock
    )
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

    fn audio_mode(&self) -> Option<Mode> {
        self.audio.session.as_ref().map(|session| session.mode)
    }

    /// Whether the player pane is up and owns Space, the arrows and the volume keys.
    pub(in crate::ui::app) fn audio_keys(&self) -> bool {
        self.audio_mode() == Some(Mode::Full)
    }

    /// The cursor is on a file the player may open.
    pub(in crate::ui::app) fn cursor_is_audio(&self) -> bool {
        self.state.cursor_entry(self.state.active).is_some_and(|entry| !entry.is_dir() && audio::can_try(&entry.name))
    }

    pub(in crate::ui::app) fn audio_is_mini(&self) -> bool {
        self.audio_mode() == Some(Mode::Mini)
    }

    /// There is a player, big or small, so the media keys have something to run.
    pub(in crate::ui::app) fn audio_alive(&self) -> bool {
        self.audio.session.is_some()
    }

    pub(in crate::ui::app) fn audio_focused(&self) -> bool {
        self.audio.focus && self.audio_is_mini()
    }

    /// The music files of a pane in the order the pane shows them.
    fn audio_siblings(&self, id: PaneId) -> Vec<PathBuf> {
        let tab = self.state.tab(id);
        self.state
            .visible(id)
            .into_iter()
            .filter_map(|at| tab.entries.get(at))
            .filter(|entry| !entry.is_dir() && audio::can_try(&entry.name))
            .map(|entry| listing::entry_path(&tab.path, entry))
            .collect()
    }

    fn audio_cursor_on(&self, path: &Path) -> bool {
        let Some(session) = &self.audio.session else { return false };
        let id = session.source;
        self.state.cursor_entry(id).is_some_and(|entry| listing::entry_path(&self.state.tab(id).path, entry) == path)
    }

    /// Puts the cursor of a pane on a file of its folder, without changing which pane is active.
    fn audio_point(&mut self, id: PaneId, path: &Path) -> bool {
        let pos = {
            let tab = self.state.tab(id);
            self.state.visible(id).iter().position(|&at| listing::entry_path(&tab.path, &tab.entries[at]) == path)
        };
        let Some(pos) = pos else { return false };
        if self.state.tab(id).cursor != pos {
            self.state.tab_mut(id).cursor = pos;
            if id == self.state.active {
                self.state.preview_gen = self.state.preview_gen.wrapping_add(1);
            }
        }
        true
    }

    /// The cursor goes with the music, if the pane where it started still shows that folder.
    fn audio_follow(&mut self, path: &Path) {
        let Some(session) = &self.audio.session else { return };
        let (id, folder) = (session.source, session.folder.clone());
        if self.state.tab(id).path == folder {
            self.audio_point(id, path);
        }
    }

    /// Follows the cursor: a new audio file starts after a moment, anything else stops the music.
    pub(in crate::ui::app) fn reconcile_audio(&mut self, sender: &ComponentSender<Self>) {
        self.settle_expand(sender);
        // The small player plays on its own. The cursor and the quick view do not steer it.
        if self.audio_is_mini() {
            return;
        }
        let want = self.audio_wanted();
        if want == self.audio.target {
            return;
        }
        let Some(path) = want else {
            self.end_session();
            return;
        };
        self.join_session(&path);
        self.begin_track(Some(path));
        self.audio.skipping = false;
        let generation = self.audio.generation;
        let tx = sender.input_sender().clone();
        gtk::glib::timeout_add_local_once(START_DELAY, move || tx.emit(Msg::AudioStart(generation)));
    }

    /// The track the cursor reached belongs to the playing folder, or starts a list of its own.
    fn join_session(&mut self, path: &Path) {
        let id = self.state.active;
        let folder = self.state.tab(id).path.clone();
        let same = self.audio.session.as_ref().is_some_and(|session| session.source == id && session.folder == folder);
        if same
            && let Some(session) = self.audio.session.as_mut()
            && session.list.set_current(path)
        {
            session.mode = Mode::Full;
            return;
        }
        let list = Playlist::new(self.audio_siblings(id), path, self.cfg.player_shuffle, self.cfg.repeat(), seed());
        self.audio.session = Some(Session { list, folder, source: id, mode: Mode::Full });
    }

    /// A new track begins: what belonged to the old one goes. A cover from the folder stays for its neighbours.
    fn begin_track(&mut self, track: Option<PathBuf>) {
        self.audio.generation += 1;
        let keep = match (&self.audio.cover_key, &track) {
            (Some(key), Some(track)) => key.parent() == track.parent(),
            _ => false,
        };
        self.audio.target = track;
        self.audio.started = false;
        self.audio.info = None;
        self.audio.problem = None;
        self.audio.seek_pending = None;
        self.audio.scrub = None;
        if !keep {
            self.set_cover(None, None);
        }
        if let Some(player) = &self.audio.player {
            player.stop();
        }
    }

    fn end_session(&mut self) {
        self.begin_track(None);
        self.audio.session = None;
        self.audio.focus = false;
        self.audio.expanding = None;
        self.audio.skipping = false;
        self.audio.failures = 0;
        self.hide_minis();
    }

    fn hide_minis(&self) {
        for pane in &self.shell.panes {
            pane.mini.set_visible(false);
            pane.mini.set_focused(false);
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
        if self.audio.player.is_none() {
            let tx = sender.input_sender().clone();
            let player = Player::spawn(audio::system_sink(), self.audio.volume, move |event| tx.emit(Msg::AudioEvent(event)));
            player.set_muted(self.audio.muted);
            let scope = player.scope_handle();
            for pane in &self.shell.panes {
                pane.player.set_scope(scope.clone());
            }
            self.audio.player = Some(player);
        }
        if let Some(player) = &self.audio.player {
            player.play(path, generation);
        }
    }

    /// Plays another track of the list at once, and takes the cursor along when it may.
    fn go_to(&mut self, path: PathBuf, follow: bool, skipping: bool, sender: &ComponentSender<Self>) {
        self.begin_track(Some(path.clone()));
        self.audio.skipping = skipping;
        if follow {
            self.audio_follow(&path);
        }
        self.start_audio_now(sender);
    }

    /// The big player plays what the list shows. If a filter or hidden files changed that, the playlist is made again.
    fn refresh_playlist(&mut self) {
        let Some(session) = &self.audio.session else { return };
        if session.mode != Mode::Full || self.state.tab(session.source).path != session.folder {
            return;
        }
        let Some(track) = self.audio.target.clone() else { return };
        let (source, same) = (session.source, session.list.items() == self.audio_siblings(session.source).as_slice());
        if same {
            return;
        }
        let list = Playlist::new(self.audio_siblings(source), &track, self.cfg.player_shuffle, self.cfg.repeat(), seed());
        if let Some(session) = self.audio.session.as_mut() {
            session.list = list;
        }
    }

    pub(in crate::ui::app) fn audio_next(&mut self, sender: &ComponentSender<Self>) {
        self.refresh_playlist();
        let Some(session) = self.audio.session.as_mut() else { return };
        match session.list.next(false).map(Path::to_path_buf) {
            Some(path) => self.go_to(path, true, true, sender),
            None => self.status = "Last track".into(),
        }
    }

    pub(in crate::ui::app) fn audio_prev(&mut self, sender: &ComponentSender<Self>) {
        let played = self.audio.player.as_ref().map_or(Duration::ZERO, Player::position);
        if played > RESTART_AFTER {
            self.audio_restart();
            return;
        }
        self.refresh_playlist();
        let Some(session) = self.audio.session.as_mut() else { return };
        match session.list.previous().map(Path::to_path_buf) {
            Some(path) => self.go_to(path, true, false, sender),
            None => self.audio_restart(),
        }
    }

    /// The same track again from its start.
    fn audio_restart(&mut self) {
        self.audio.scrub = Some((Duration::ZERO, Instant::now() + SCRUB_HOLD));
        self.audio.seek_pending = None;
        self.audio.seek_sent = Instant::now();
        if let Some(player) = &self.audio.player {
            player.seek(Duration::ZERO);
        }
    }

    /// A track ended by itself. The next one follows, and the cursor with it unless it was moved away.
    fn audio_advance(&mut self, sender: &ComponentSender<Self>) {
        let ended = self.audio.target.clone();
        self.refresh_playlist();
        let Some(session) = self.audio.session.as_mut() else { return };
        let Some(path) = session.list.next(true).map(Path::to_path_buf) else { return };
        let follow = ended.as_deref().is_some_and(|ended| self.audio_cursor_on(ended));
        self.go_to(path, follow, true, sender);
    }

    pub(in crate::ui::app) fn on_audio_event(&mut self, event: Event, sender: &ComponentSender<Self>) {
        match event {
            Event::Loaded { generation, info } if generation == self.audio.generation => {
                self.audio.problem = None;
                self.audio.failures = 0;
                let embedded = info.cover.clone();
                self.audio.info = Some(info);
                self.spawn_cover(generation, embedded, sender);
            }
            Event::Failed { generation, message, unsupported } if generation == self.audio.generation => {
                self.audio.problem = Some((message, unsupported));
                self.audio.failures += 1;
                let more = self.audio.session.as_ref().is_some_and(|session| self.audio.failures < session.list.len());
                if self.audio.skipping && more {
                    self.skip_failed(sender);
                }
            }
            Event::Ended { generation } if generation == self.audio.generation => self.audio_advance(sender),
            _ => {}
        }
    }

    /// A track that cannot play is passed over, the way a player does with a broken file in a list.
    fn skip_failed(&mut self, sender: &ComponentSender<Self>) {
        let failed = self.audio.target.clone();
        let Some(session) = self.audio.session.as_mut() else { return };
        let Some(path) = session.list.next(false).map(Path::to_path_buf) else { return };
        let follow = failed.as_deref().is_some_and(|failed| self.audio_cursor_on(failed));
        self.go_to(path, follow, true, sender);
    }

    /// The cover is decoded off the main thread: the track's own first, then a picture in its folder.
    fn spawn_cover(&self, generation: u64, embedded: Option<Arc<audio::Cover>>, sender: &ComponentSender<Self>) {
        let Some(track) = self.audio.target.clone() else { return };
        let showing = self.audio.cover_key.clone();
        let tx = sender.input_sender().clone();
        std::thread::spawn(move || {
            if let Some(cover) = embedded
                && let Ok(image) = imageview::decode_bytes(&cover.data, COVER_EDGE)
            {
                tx.emit(Msg::AudioCover { generation, image: Some((image.width, image.height, image.pixels)), key: None });
                return;
            }
            let found = track.parent().and_then(audio::folder_cover);
            let Some(file) = found else {
                tx.emit(Msg::AudioCover { generation, image: None, key: None });
                return;
            };
            // The next track of an album shows the picture it already has.
            if showing.as_ref() == Some(&file) {
                return;
            }
            let image = imageview::decode(&file, COVER_EDGE).ok().map(|image| (image.width, image.height, image.pixels));
            tx.emit(Msg::AudioCover { generation, image, key: Some(file) });
        });
    }

    pub(in crate::ui::app) fn on_audio_cover(&mut self, generation: u64, image: Option<(u32, u32, Vec<u8>)>, key: Option<PathBuf>) {
        if generation == self.audio.generation {
            self.set_cover(image, key);
        }
    }

    fn set_cover(&mut self, image: Option<(u32, u32, Vec<u8>)>, key: Option<PathBuf>) {
        self.audio.cover = image;
        self.audio.cover_key = key;
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
            Some((_, true)) => "The built-in player cannot read this format. Press Ctrl+Enter to Open In External Player.".to_string(),
            Some((text, false)) => text.clone(),
            None => String::new(),
        };
        let tracks = self.audio.session.as_ref().map_or(0, |session| session.list.len());
        Look {
            title: info.and_then(|i| i.title.clone()).unwrap_or_else(|| stem.to_string()),
            artist: info.and_then(|i| i.artist.clone()).unwrap_or_default(),
            album: info.and_then(|i| i.album.clone()).unwrap_or_default(),
            detail: info.map(audio::describe).unwrap_or_default(),
            message,
            playing: state == State::Playing,
            can_toggle: matches!(state, State::Playing | State::Paused | State::Ended),
            can_skip: tracks > 1,
            position_ms: position.as_millis() as u64,
            duration_ms: duration.map(|d| d.as_millis() as u64),
            volume: self.audio.volume,
            muted: self.audio.muted,
            shuffle: self.cfg.player_shuffle,
            repeat: self.cfg.repeat(),
            visual_on: self.cfg.player_visual_on,
            visual: self.cfg.visual_style(),
        }
    }

    pub(in crate::ui::app) fn audio_player_state(&self) -> Option<State> {
        self.audio.player.as_ref().map(Player::state)
    }

    pub(in crate::ui::app) fn audio_volume(&self) -> f32 {
        self.audio.volume
    }

    /// How many tracks the playlist holds.
    pub(in crate::ui::app) fn audio_list_len(&self) -> usize {
        self.audio.session.as_ref().map_or(0, |session| session.list.len())
    }

    /// The track that plays or is about to.
    pub(in crate::ui::app) fn audio_track(&self) -> Option<&Path> {
        self.audio.target.as_deref()
    }

    /// Paints the player where it lives. Cheap when nothing changed.
    pub(in crate::ui::app) fn paint_audio(&self) {
        let Some(session) = &self.audio.session else { return };
        if self.audio.target.is_none() {
            return;
        }
        let image = self.audio.cover.as_ref().map(|(w, h, p)| (*w, *h, p.as_slice()));
        match session.mode {
            Mode::Full => {
                for pane in &self.shell.panes {
                    pane.mini.set_visible(false);
                }
                if !matches!(self.preview.body, PreviewBody::Audio) {
                    return;
                }
                let view = &self.shell.panes[self.state.active.other().index()].player;
                view.set_cover(self.audio.cover_rev, image);
                view.paint(&self.audio_look());
            }
            Mode::Mini => {
                for (index, pane) in self.shell.panes.iter().enumerate() {
                    pane.mini.set_visible(index == session.source.index());
                }
                let mini = &self.shell.panes[session.source.index()].mini;
                mini.set_cover(self.audio.cover_rev, image);
                mini.paint(&self.audio_look());
                mini.set_focused(self.audio.focus);
            }
        }
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

    /// The default player takes over, so the built-in one steps aside.
    pub(in crate::ui::app) fn audio_pause_for_external(&mut self) {
        if let Some(player) = &self.audio.player
            && player.state() == State::Playing
        {
            player.pause();
        }
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

    pub(in crate::ui::app) fn audio_toggle_shuffle(&mut self) {
        self.cfg.player_shuffle = !self.cfg.player_shuffle;
        if let Some(session) = self.audio.session.as_mut() {
            session.list.set_shuffle(self.cfg.player_shuffle);
        }
        let _ = config::save(&self.cfg);
        self.status = format!("Shuffle {}", if self.cfg.player_shuffle { "on" } else { "off" });
        self.paint_music_settings();
    }

    pub(in crate::ui::app) fn audio_cycle_repeat(&mut self) {
        let repeat = self.cfg.repeat().cycle();
        self.audio_set_repeat(repeat);
    }

    pub(in crate::ui::app) fn audio_set_repeat(&mut self, repeat: Repeat) {
        self.cfg.player_repeat = repeat.name().to_string();
        if let Some(session) = self.audio.session.as_mut() {
            session.list.set_repeat(repeat);
        }
        let _ = config::save(&self.cfg);
        self.status = format!("Repeat {}", repeat.name());
        self.paint_music_settings();
    }

    pub(in crate::ui::app) fn audio_set_visual_on(&mut self, on: bool) {
        if self.cfg.player_visual_on != on {
            self.cfg.player_visual_on = on;
            let _ = config::save(&self.cfg);
        }
        self.paint_music_settings();
    }

    pub(in crate::ui::app) fn audio_set_visual_style(&mut self, style: VisualStyle) {
        if self.cfg.visual_style() != style {
            self.cfg.player_visual = style.name().to_string();
            let _ = config::save(&self.cfg);
        }
        self.paint_music_settings();
    }

    /// The player folds into a bar under the list it started from. Both lists work as usual again.
    pub(in crate::ui::app) fn audio_minimize(&mut self, sender: &ComponentSender<Self>) {
        let Some(session) = self.audio.session.as_mut() else { return };
        if session.mode != Mode::Full {
            return;
        }
        session.mode = Mode::Mini;
        if self.state.quick_view {
            self.toggle_quick_view(sender);
        }
        self.focus_list();
        self.status = "Player minimized, Ctrl+M jumps to it".into();
    }

    /// Back to the full player: the list shows the folder with the track under the cursor, and the quick view opens.
    pub(in crate::ui::app) fn audio_expand(&mut self, sender: &ComponentSender<Self>) {
        let Some(session) = &self.audio.session else { return };
        let Some(track) = self.audio.target.clone() else { return };
        if session.mode != Mode::Mini {
            return;
        }
        let (source, folder) = (session.source, session.folder.clone());
        let Some(name) = track.file_name().map(|name| name.to_string_lossy().into_owned()) else { return };
        self.audio.focus = false;
        self.audio.expanding = Some(Instant::now());
        if name.starts_with('.') && !self.state.show_hidden {
            self.state.show_hidden = true;
            self.touch_both();
        }
        self.state.pane_mut(source).filter.clear();
        self.shell.panes[source.index()].filter.set_text("");
        self.touch(source);
        if self.state.tab(source).path == folder {
            self.state.active = source;
            self.audio_point(source, &track);
            self.focus_list();
            return;
        }
        let token = self.state.navigate(source, folder.clone(), true);
        let tab = self.state.pane(source).active;
        self.state.active = source;
        self.shell.active.set(source.index());
        self.search_target = Some((source, tab, token, name));
        self.spawn_load(sender, source, tab, folder, token);
        self.focus_list();
    }

    /// Finishes an expand once the cursor stands on the track.
    fn settle_expand(&mut self, sender: &ComponentSender<Self>) {
        let Some(since) = self.audio.expanding else { return };
        if since.elapsed() > EXPAND_WAIT {
            self.audio.expanding = None;
            return;
        }
        let Some(track) = self.audio.target.clone() else {
            self.audio.expanding = None;
            return;
        };
        let Some(source) = self.audio.session.as_ref().map(|session| session.source) else { return };
        if self.state.active != source || !self.audio_cursor_on(&track) {
            return;
        }
        self.audio.expanding = None;
        if let Some(session) = self.audio.session.as_mut() {
            session.mode = Mode::Full;
        }
        if self.state.quick_view {
            self.note_preview_target();
        } else {
            self.toggle_quick_view(sender);
        }
    }

    pub(in crate::ui::app) fn audio_close(&mut self) {
        self.end_session();
        // A quick view on a music file shows the full player again, and the cursor starts it.
        self.note_preview_target();
        self.focus_list();
    }

    /// Ctrl+M: the big player folds away. The small one gets the keyboard, or gives it back.
    pub(in crate::ui::app) fn audio_jump(&mut self, sender: &ComponentSender<Self>) {
        match self.audio_mode() {
            Some(Mode::Mini) => {
                self.audio.focus = !self.audio.focus;
                if !self.audio.focus {
                    self.focus_list();
                }
            }
            Some(Mode::Full) => self.audio_minimize(sender),
            None => self.status = "No music is playing".into(),
        }
    }

    /// Ctrl+Enter: the default player opens the file, and the built-in one pauses.
    pub(in crate::ui::app) fn audio_open_external(&mut self) {
        if self.cursor_is_audio() {
            self.audio_pause_for_external();
            self.open_external(true);
        }
    }

    pub(in crate::ui::app) fn audio_unfocus(&mut self) {
        self.audio.focus = false;
    }

    /// Tab goes left panel, right panel, small player, and round again.
    pub(in crate::ui::app) fn audio_tab(&mut self, back: bool) {
        if self.audio.focus {
            self.audio.focus = false;
            if !self.state.quick_view {
                self.state.active = if back { PaneId::Right } else { PaneId::Left };
                self.note_preview_target();
            }
            self.focus_list();
            return;
        }
        let edge = self.state.quick_view || if back { self.state.active == PaneId::Left } else { self.state.active == PaneId::Right };
        if edge {
            self.audio.focus = true;
            return;
        }
        self.state.active = self.state.active.other();
        self.note_preview_target();
        self.focus_list();
    }

    /// Keys while the small player has the keyboard. False lets the key through to the list, and the player lets go.
    pub(in crate::ui::app) fn audio_focus_key(&mut self, key: Key, mods: ModifierType, sender: &ComponentSender<Self>) -> bool {
        if is_modifier(key) {
            return false;
        }
        let ctrl = mods.contains(ModifierType::CONTROL_MASK);
        let alt = mods.contains(ModifierType::ALT_MASK);
        let shift = mods.contains(ModifierType::SHIFT_MASK);
        // Ctrl+M asks for the player to be left again, and that is handled with the other Ctrl keys.
        if ctrl && !alt && matches!(key, Key::m | Key::M) {
            return false;
        }
        if ctrl || alt {
            self.audio.focus = false;
            return false;
        }
        match key {
            Key::space | Key::KP_Space | Key::AudioPlay | Key::AudioPause => self.audio_toggle(sender),
            Key::Left | Key::KP_Left => self.audio_key_seek(false, shift),
            Key::Right | Key::KP_Right => self.audio_key_seek(true, shift),
            Key::Down | Key::KP_Down | Key::n | Key::N | Key::AudioNext => self.audio_next(sender),
            Key::Up | Key::KP_Up | Key::p | Key::P | Key::AudioPrev => self.audio_prev(sender),
            Key::r | Key::R => self.audio_cycle_repeat(),
            Key::s | Key::S => self.audio_toggle_shuffle(),
            Key::plus | Key::KP_Add | Key::equal => self.audio_volume_step(VOLUME_STEP),
            Key::minus | Key::KP_Subtract => self.audio_volume_step(-VOLUME_STEP),
            Key::m | Key::M => self.audio_toggle_mute(),
            Key::Return | Key::KP_Enter => {}
            Key::Escape => {
                self.audio.focus = false;
                self.focus_list();
            }
            Key::Tab | Key::ISO_Left_Tab => self.audio_tab(key == Key::ISO_Left_Tab || shift),
            _ => {
                self.audio.focus = false;
                return false;
            }
        }
        true
    }
}

use std::time::{Duration, Instant};

use relm4::gtk::gdk::{Key, ModifierType};
use relm4::gtk::prelude::*;
use relm4::prelude::*;

use rust_commander::audio::State;
use rust_commander::state::PaneId;

use super::App;
use crate::ui::msg::Msg;

const NONE: ModifierType = ModifierType::empty();

impl App {
    fn smoke_cursor(&self) -> String {
        self.state.cursor_entry(PaneId::Left).map(|entry| entry.name.to_string()).unwrap_or_default()
    }

    fn smoke_track(&self) -> String {
        self.audio_track().and_then(|path| path.file_name()).map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
    }

    /// The cursor and the player stand on the same file.
    fn smoke_together(&self, name: &str) -> bool {
        self.smoke_cursor() == name && self.smoke_track() == name
    }

    fn smoke_mark(&mut self, id: PaneId, names: &[&str]) {
        let dir = self.state.tab(id).path.clone();
        let marked = &mut self.state.tab_mut(id).marked;
        marked.clear();
        for name in names {
            marked.insert(dir.join(name));
        }
    }

    /// What the quick view, the cursor and the player each show, brought up to date.
    fn smoke_settle(&mut self, sender: &ComponentSender<Self>) {
        self.note_preview_target();
        self.kick_preview(sender);
        self.reconcile_audio(sender);
        self.sync();
        self.paint_audio();
    }

    /// The player, from the idle quick view to the small player and back, send and add, and the keys on the way.
    pub(in crate::ui::app) fn smoke_player(&mut self, errors: &mut Vec<String>, sender: &ComponentSender<Self>) {
        let wait = |cond: &mut dyn FnMut() -> bool| {
            let end = Instant::now() + Duration::from_secs(5);
            while Instant::now() < end {
                if cond() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            false
        };
        let press = |app: &mut Self, key: Key| app.on_key(key, NONE, sender);
        let ctrl = |app: &mut Self, key: Key| app.on_key(key, ModifierType::CONTROL_MASK, sender);
        let ctrl_shift = |app: &mut Self, key: Key| app.on_key(key, ModifierType::CONTROL_MASK | ModifierType::SHIFT_MASK, sender);
        let left_before = self.state.tab(PaneId::Left).path.clone();
        let right_before = self.state.tab(PaneId::Right).path.clone();

        self.reload_pane(PaneId::Left);
        if !self.point_at(PaneId::Left, "tone.wav") {
            errors.push("music: missing tone.wav".into());
            return;
        }

        // The quick view on a song waits for Enter. Nothing plays by itself.
        self.state.quick_view = true;
        self.smoke_settle(sender);
        if self.shell.panes[1].stack.visible_child_name().as_deref() != Some("preview")
            || self.shell.panes[1].preview_kind.visible_child_name().as_deref() != Some("player")
        {
            errors.push("music: player not shown".into());
        }
        let idle = self.shell.panes[1].player.looks();
        if idle.title != "tone" || !idle.detail.contains("Enter") || self.audio_alive() || self.audio_player_state().is_some() {
            errors.push(format!("music: the idle player showed {:?} / {:?}", idle.title, idle.detail));
        }
        // Space marks, as in any list.
        press(self, Key::space);
        if self.state.tab(PaneId::Left).marked.len() != 1 || self.audio_alive() {
            errors.push("music: space did not mark with the player idle".into());
        }
        press(self, Key::space);
        self.state.tab_mut(PaneId::Left).marked.clear();

        // Enter plays it in the big player, through to the end.
        press(self, Key::Return);
        if !self.audio_full() || self.smoke_track() != "tone.wav" {
            errors.push(format!("music: enter in the idle quick view gave full {} and track {}", self.audio_full(), self.smoke_track()));
        }
        if !wait(&mut || self.audio_player_state() == Some(State::Ended)) {
            errors.push(format!("music: did not play to the end, {:?}", self.audio_player_state()));
        }

        // The cursor does not steer the player: not onto another song, not onto a text file.
        self.point_at(PaneId::Left, "tone2.wav");
        self.smoke_settle(sender);
        if self.smoke_track() != "tone.wav" || !self.audio_full() {
            errors.push(format!("music: the cursor changed the song to {}", self.smoke_track()));
        }
        self.point_at(PaneId::Left, "note.txt");
        self.smoke_settle(sender);
        if !self.audio_full() || self.shell.panes[1].preview_kind.visible_child_name().as_deref() != Some("player") {
            errors.push("music: the player gave up the pane on a text file".into());
        }
        // Enter on a song plays that one.
        self.point_at(PaneId::Left, "tone2.wav");
        press(self, Key::Return);
        if !self.smoke_together("tone2.wav") || !self.audio_full() {
            errors.push(format!("music: enter gave cursor {} and track {}", self.smoke_cursor(), self.smoke_track()));
        }

        // Tab takes the keys to the player, and back.
        press(self, Key::Tab);
        self.smoke_settle(sender);
        if !self.audio_focused() || !self.shell.panes[1].player.root.has_css_class("focused") {
            errors.push("music: tab did not reach the big player".into());
        }
        // Space is the player's now, and it marks nothing.
        press(self, Key::space);
        if !self.state.tab(PaneId::Left).marked.is_empty() || !self.audio_focused() {
            errors.push("music: space marked a file while the big player had the keys".into());
        }
        let volume = self.audio_volume();
        press(self, Key::minus);
        if self.audio_volume() >= volume {
            errors.push("music: volume key".into());
        }
        // Ctrl+arrows seek. They do not mirror the panel, and the plain arrows leave the song alone.
        let (cursor, right) = (self.state.tab(PaneId::Left).cursor, self.state.tab(PaneId::Right).path.clone());
        ctrl(self, Key::Right);
        press(self, Key::Left);
        if !self.audio_focused() || self.state.tab(PaneId::Left).cursor != cursor || self.state.tab(PaneId::Right).path != right {
            errors.push("music: arrows in the big player went to the panels".into());
        }
        press(self, Key::Tab);
        self.smoke_settle(sender);
        if self.audio_focused() || self.state.active != PaneId::Left || self.shell.panes[1].player.root.has_css_class("focused") {
            errors.push("music: tab did not come back to the panel".into());
        }
        // With the panel back in charge, Space marks again, and Ctrl+Right is the mirror.
        press(self, Key::space);
        if self.state.tab(PaneId::Left).marked.len() != 1 {
            errors.push("music: space no longer marks".into());
        }
        press(self, Key::space);
        self.state.tab_mut(PaneId::Left).marked.clear();

        // Next and previous move the cursor along.
        press(self, Key::Tab);
        press(self, Key::n);
        if !self.smoke_together("tone3.wav") {
            errors.push(format!("music: next gave cursor {} and track {}", self.smoke_cursor(), self.smoke_track()));
        }
        // The last track of a list that does not repeat stays.
        press(self, Key::n);
        if !self.smoke_together("tone3.wav") {
            errors.push(format!("music: the last track moved on to {}", self.smoke_track()));
        }
        press(self, Key::p);
        if !self.smoke_together("tone2.wav") {
            errors.push(format!("music: previous gave cursor {} and track {}", self.smoke_cursor(), self.smoke_track()));
        }
        for want in ["all", "one", "off"] {
            press(self, Key::r);
            if self.cfg.player_repeat != want {
                errors.push(format!("music: repeat is {} and should be {want}", self.cfg.player_repeat));
            }
        }
        press(self, Key::s);
        let shuffled = self.cfg.player_shuffle;
        press(self, Key::s);
        if !shuffled || self.cfg.player_shuffle {
            errors.push("music: shuffle key".into());
        }
        // Enter is the player's own too, and swallowed: it does not leave for another program.
        press(self, Key::Return);
        if !self.audio_focused() {
            errors.push("music: enter let go of the player".into());
        }
        press(self, Key::Escape);
        if self.audio_focused() {
            errors.push("music: escape kept the keys".into());
        }

        // A filter changes what the list shows, and the playlist follows it at the next step.
        self.point_at(PaneId::Left, "tone3.wav");
        press(self, Key::Return);
        if self.smoke_track() != "tone3.wav" || self.audio_list_len() != 3 {
            errors.push(format!("music: enter on a song gave {} songs and track {}", self.audio_list_len(), self.smoke_track()));
        }
        self.on_msg(Msg::FilterChanged(0, "tone3".into()), sender);
        self.sync();
        self.point_at(PaneId::Left, "tone3.wav");
        self.audio_prev(sender);
        if self.audio_list_len() != 1 {
            errors.push(format!("music: the playlist kept {} tracks under a filter of one", self.audio_list_len()));
        }
        // Expanding with a filter on clears it, and the cursor stays on the track instead of jumping to the top.
        self.audio_minimize(sender);
        self.audio_expand(sender);
        self.on_msg(Msg::FilterChanged(0, String::new()), sender);
        self.smoke_settle(sender);
        if !self.audio_full() || !self.state.quick_view || self.smoke_cursor() != self.smoke_track() {
            errors.push(format!("music: expand under a filter gave cursor {} and track {}", self.smoke_cursor(), self.smoke_track()));
        }
        // The filter box was shown by the test. The checks after this one start from a hidden one.
        self.shell.panes[0].filter.set_visible(false);
        self.sync();

        // Send and replace: marked songs become the playlist, in the order the list shows them.
        self.point_at(PaneId::Left, "tone.wav");
        self.smoke_mark(PaneId::Left, &["tone3.wav", "tone2.wav"]);
        ctrl(self, Key::p);
        if !self.audio_full() || self.audio_list_len() != 2 || self.smoke_track() != "tone2.wav" {
            errors.push(format!("music: ctrl+p gave full {}, {} songs, track {}", self.audio_full(), self.audio_list_len(), self.smoke_track()));
        }
        // Add: after the ones that are there, and not twice.
        self.smoke_mark(PaneId::Left, &["tone.wav"]);
        ctrl_shift(self, Key::p);
        if self.audio_list_len() != 3 || self.smoke_track() != "tone2.wav" {
            errors.push(format!("music: ctrl+shift+p gave {} songs and track {}", self.audio_list_len(), self.smoke_track()));
        }
        ctrl_shift(self, Key::p);
        if self.audio_list_len() != 3 {
            errors.push("music: the same song was added twice".into());
        }
        // The playlist is the picked songs, not the folder: it stays when the list changes.
        self.on_msg(Msg::FilterChanged(0, "tone".into()), sender);
        self.audio_next(sender);
        if self.audio_list_len() != 3 || self.smoke_track() != "tone3.wav" {
            errors.push(format!("music: the picked playlist was remade, {} songs and track {}", self.audio_list_len(), self.smoke_track()));
        }
        self.on_msg(Msg::FilterChanged(0, String::new()), sender);
        self.shell.panes[0].filter.set_visible(false);
        self.sync();
        // A right-click on a row that is not marked sends that row alone and leaves the marks.
        self.smoke_mark(PaneId::Left, &["tone2.wav", "tone3.wav"]);
        self.point_at(PaneId::Left, "tone.wav");
        self.audio_send(true, sender);
        if self.audio_list_len() != 1 || self.smoke_track() != "tone.wav" || self.state.tab(PaneId::Left).marked.len() != 2 {
            errors.push("music: a right-click on an unmarked row did not send only that row".into());
        }
        // On a marked row it sends the marks.
        self.audio_send(false, sender);
        if self.audio_list_len() != 2 {
            errors.push("music: a right-click on a marked row did not send the marks".into());
        }
        self.state.tab_mut(PaneId::Left).marked.clear();

        self.smoke_small(errors, sender);
        self.smoke_hammer(errors, sender);

        // Ctrl+Q starts the big player on a song at once. Ctrl+Q on a text file folds it into the small player,
        // which plays on while the quick view shows the file. Only Ctrl+Shift+Q stops the music.
        self.audio_close(sender);
        if self.state.quick_view {
            ctrl(self, Key::q);
        }
        self.point_at(PaneId::Left, "tone2.wav");
        ctrl(self, Key::q);
        if !self.state.quick_view || !self.audio_full() || self.smoke_track() != "tone2.wav" {
            errors.push(format!("music: ctrl+q on a song gave full {} and track {}", self.audio_full(), self.smoke_track()));
        }
        self.point_at(PaneId::Left, "note.txt");
        self.smoke_settle(sender);
        ctrl(self, Key::q);
        self.smoke_settle(sender);
        if !self.audio_is_mini() || !self.state.quick_view || self.smoke_track() != "tone2.wav" {
            errors.push(format!(
                "music: ctrl+q on a text file gave mini {}, quick view {}, track {}",
                self.audio_is_mini(),
                self.state.quick_view,
                self.smoke_track()
            ));
        }
        if self.shell.panes[1].preview_kind.visible_child_name().as_deref() == Some("player") {
            errors.push("music: the quick view still shows the player after ctrl+q on a text file".into());
        }
        ctrl(self, Key::q);
        self.smoke_settle(sender);
        if self.state.quick_view || !self.audio_is_mini() {
            errors.push("music: the second ctrl+q did not close just the quick view".into());
        }
        ctrl_shift(self, Key::q);
        self.smoke_settle(sender);
        if self.audio_alive() {
            errors.push("music: ctrl+shift+q did not close the small player".into());
        }
        if !wait(&mut || self.audio_player_state() == Some(State::Idle)) {
            errors.push("music: not stopped after ctrl+shift+q".into());
        }

        // Ctrl+Q with the cursor on a song folds the big player and closes the quick view.
        self.point_at(PaneId::Left, "tone2.wav");
        ctrl(self, Key::q);
        ctrl(self, Key::q);
        self.smoke_settle(sender);
        if self.state.quick_view || !self.audio_is_mini() {
            errors.push("music: ctrl+q on a song did not fold the big player".into());
        }
        // Closing the big player, as the cross does, closes the quick view it fills.
        self.audio_expand(sender);
        self.smoke_settle(sender);
        self.smoke_settle(sender);
        if !self.audio_full() {
            errors.push("music: expand did not make the player big".into());
        }
        self.on_msg(Msg::AudioClose, sender);
        self.smoke_settle(sender);
        if self.audio_alive() || self.state.quick_view {
            errors.push("music: the cross did not close the big player and its quick view".into());
        }
        // Escape closes the quick view, and the music plays on in the small player.
        self.point_at(PaneId::Left, "tone2.wav");
        ctrl(self, Key::q);
        self.smoke_settle(sender);
        press(self, Key::Escape);
        self.smoke_settle(sender);
        if self.state.quick_view || !self.audio_is_mini() {
            errors.push("music: escape stopped the big player".into());
        }
        self.audio_close(sender);
        self.smoke_settle(sender);

        // Send goes to the player that is open. None open: the big one, and it opens the quick view.
        self.smoke_mark(PaneId::Left, &["tone2.wav", "tone3.wav"]);
        ctrl(self, Key::p);
        if !self.audio_full() || !self.state.quick_view {
            errors.push("music: ctrl+p with nothing open did not start the big player in the quick view".into());
        }
        // Enter on a song with nothing open starts the big player too, and the quick view comes with it.
        self.audio_close(sender);
        if self.state.quick_view {
            ctrl(self, Key::q);
        }
        self.smoke_settle(sender);
        self.point_at(PaneId::Left, "tone2.wav");
        press(self, Key::Return);
        self.smoke_settle(sender);
        if !self.audio_full() || !self.state.quick_view || self.smoke_track() != "tone2.wav" {
            errors.push(format!("music: enter with nothing open gave full {}, quick view {}", self.audio_full(), self.state.quick_view));
        }
        self.audio_close(sender);
        self.state.quick_view = true;
        self.smoke_settle(sender);
        ctrl(self, Key::p);
        if !self.audio_full() {
            errors.push("music: ctrl+p in the quick view did not start the big player".into());
        }

        // Back to how the rest of the smoke test expects things.
        self.audio_close(sender);
        self.state.quick_view = false;
        self.smoke_settle(sender);
        self.state.tab_mut(PaneId::Left).marked.clear();
        self.state.navigate(PaneId::Left, left_before, true);
        self.reload_pane(PaneId::Left);
        self.state.navigate(PaneId::Right, right_before, true);
        self.reload_pane(PaneId::Right);
        self.state.active = PaneId::Left;
        self.sync();
    }

    /// Tab, Ctrl+M and the list keys pressed in a hurry with the small player up: no step may stall, and the keys never end up half in the player.
    fn smoke_hammer(&mut self, errors: &mut Vec<String>, sender: &ComponentSender<Self>) {
        const SHIFT: ModifierType = ModifierType::SHIFT_MASK;
        const CTRL: ModifierType = ModifierType::CONTROL_MASK;
        let keys = [
            (Key::Tab, NONE),
            (Key::Tab, NONE),
            (Key::ISO_Left_Tab, SHIFT),
            (Key::m, CTRL),
            (Key::Down, NONE),
            (Key::Escape, NONE),
            (Key::Tab, NONE),
            (Key::Page_Down, NONE),
            (Key::Up, NONE),
            (Key::space, NONE),
        ];
        if self.audio_alive() {
            self.audio_close(sender);
        }
        self.state.active = PaneId::Left;
        self.point_at(PaneId::Left, "tone.wav");
        self.on_key(Key::Return, NONE, sender);
        self.audio_minimize(sender);
        self.smoke_settle(sender);
        if !self.audio_is_mini() {
            errors.push("music: hammer: the small player did not start".into());
            return;
        }
        let begun = Instant::now();
        let mut slowest = Duration::ZERO;
        for round in 0..400 {
            // Ctrl+M can make it big. Fold it again so the next keys meet the small one.
            if self.audio_full() {
                self.audio_minimize(sender);
            }
            let (key, mods) = keys[round % keys.len()];
            let at = Instant::now();
            self.on_key(key, mods, sender);
            if round % 5 == 0 {
                self.smoke_settle(sender);
            }
            slowest = slowest.max(at.elapsed());
            let shown = self.shell.panes.iter().filter(|pane| pane.mini.root.is_visible()).count();
            if shown > 1 {
                errors.push(format!("music: hammer round {round}: {shown} small players"));
                break;
            }
        }
        self.smoke_settle(sender);
        if self.audio_is_mini() && !self.shell.panes.iter().any(|pane| pane.mini.root.is_visible()) {
            errors.push("music: hammer: the small player is playing without a bar".into());
        }
        if slowest > Duration::from_millis(500) || begun.elapsed() > Duration::from_secs(10) {
            errors.push(format!("music: hammer was slow, slowest key {slowest:?}, all {:?}", begun.elapsed()));
        }
        self.audio_close(sender);
    }

    /// The small player: minimize, Tab and Ctrl+M, expand, close, and songs sent to it from the other panel.
    fn smoke_small(&mut self, errors: &mut Vec<String>, sender: &ComponentSender<Self>) {
        let press = |app: &mut Self, key: Key| app.on_key(key, NONE, sender);
        let ctrl = |app: &mut Self, key: Key| app.on_key(key, ModifierType::CONTROL_MASK, sender);

        // Minimize: the quick view closes, and the bar sits under the list the music came from.
        self.point_at(PaneId::Left, "tone2.wav");
        self.smoke_settle(sender);
        self.audio_minimize(sender);
        self.smoke_settle(sender);
        if !self.audio_is_mini() || self.state.quick_view || self.audio_full() {
            errors.push("music: minimize did not fold the player".into());
        }
        if !self.shell.panes[0].mini.root.is_visible() || self.shell.panes[1].mini.root.is_visible() {
            errors.push("music: the small player is not under the left list".into());
        }
        let track = self.smoke_track();
        if self.shell.panes[0].mini.looks().title != track.trim_end_matches(".wav") {
            errors.push(format!("music: small player title {:?} for {track}", self.shell.panes[0].mini.looks().title));
        }
        // It plays on, and the lists work: a file that is not music does not stop it.
        self.point_at(PaneId::Left, "note.txt");
        self.smoke_settle(sender);
        if !self.audio_is_mini() || self.smoke_track() != track {
            errors.push("music: the small player followed the cursor".into());
        }
        self.point_at(PaneId::Left, &track);

        // Tab goes left, right, player, left.
        press(self, Key::Tab);
        if self.state.active != PaneId::Right {
            errors.push("music: tab did not reach the right panel".into());
        }
        press(self, Key::Tab);
        if !self.audio_focused() {
            errors.push("music: tab did not reach the small player".into());
        }
        // While the player has the keys, no list has the accent. It blinks when the keys come back.
        self.smoke_settle(sender);
        if self.shell.panes.iter().any(|pane| pane.frame.has_css_class("pane-active")) {
            errors.push("music: a list kept the accent while the small player had the keys".into());
        }
        if !self.shell.panes.iter().any(|pane| pane.mini.root.has_css_class("flash")) {
            errors.push("music: the small player did not blink when it got the keys".into());
        }
        press(self, Key::space);
        if !self.state.tab(PaneId::Left).marked.is_empty() || !self.state.tab(PaneId::Right).marked.is_empty() {
            errors.push("music: space marked a file while the small player had the keys".into());
        }
        // Ctrl+arrows and plain arrows keep the keys with the player.
        let (cursor, right) = (self.state.tab(PaneId::Right).cursor, self.state.tab(PaneId::Right).path.clone());
        ctrl(self, Key::Right);
        press(self, Key::Right);
        if !self.audio_focused() || self.state.tab(PaneId::Right).path != right || self.state.tab(PaneId::Right).cursor != cursor {
            errors.push("music: arrows left the small player".into());
        }
        press(self, Key::Tab);
        if self.audio_focused() || self.state.active != PaneId::Left {
            errors.push("music: tab did not come back to the left panel".into());
        }
        self.smoke_settle(sender);
        if !self.shell.panes[0].frame.has_css_class("pane-blink") {
            errors.push("music: the panel did not blink when the keys came back from the small player".into());
        }
        // Ctrl+M asks for the player, and Escape gives the keys back.
        ctrl(self, Key::m);
        if !self.audio_focused() {
            errors.push("music: ctrl+m did not reach the small player".into());
        }
        press(self, Key::Escape);
        if self.audio_focused() {
            errors.push("music: escape kept the keys".into());
        }
        // Ctrl+M with the keys in the small player makes it big, and Ctrl+M in the big one folds it again.
        ctrl(self, Key::m);
        ctrl(self, Key::m);
        self.smoke_settle(sender);
        if !self.audio_full() || !self.state.quick_view || self.audio_focused() {
            errors.push(format!("music: ctrl+m in the small player gave full {}, quick view {}", self.audio_full(), self.state.quick_view));
        }
        ctrl(self, Key::m);
        self.smoke_settle(sender);
        if !self.audio_is_mini() || self.state.quick_view {
            errors.push("music: ctrl+m in the big player did not fold it".into());
        }
        // A key that is not the player's goes to the list, and the player lets go.
        ctrl(self, Key::m);
        let cursor = self.state.tab(PaneId::Left).cursor;
        press(self, Key::Page_Down);
        if self.audio_focused() || self.state.tab(PaneId::Left).cursor == cursor {
            errors.push("music: a list key was swallowed by the small player".into());
        }
        self.point_at(PaneId::Left, &track);

        // A song in another folder, in the other panel: Ctrl+Q plays it in the small player.
        let sub = self.state.tab(PaneId::Left).path.join("sub");
        self.state.navigate(PaneId::Right, sub, true);
        self.reload_pane(PaneId::Right);
        self.sync();
        self.point_at(PaneId::Right, "other.wav");
        ctrl(self, Key::q);
        self.smoke_settle(sender);
        if self.state.quick_view || !self.audio_is_mini() || self.smoke_track() != "other.wav" {
            errors.push(format!("music: ctrl+q over the small player gave quick view {} and track {}", self.state.quick_view, self.smoke_track()));
        }
        if self.shell.panes[0].mini.root.is_visible() || !self.shell.panes[1].mini.root.is_visible() {
            errors.push("music: the small player did not move under the right list".into());
        }
        // Ctrl+Q on the song that already plays changes nothing.
        let playing = self.audio_track().map(ToOwned::to_owned);
        ctrl(self, Key::q);
        if self.state.quick_view || self.audio_track().map(ToOwned::to_owned) != playing {
            errors.push("music: ctrl+q on the playing song did something".into());
        }
        // Enter on another song of that folder plays it in the small player.
        self.point_at(PaneId::Right, "other2.wav");
        press(self, Key::Return);
        if self.smoke_track() != "other2.wav" || !self.audio_is_mini() || self.state.quick_view {
            errors.push(format!("music: enter gave track {}, mini {}, quick view {}", self.smoke_track(), self.audio_is_mini(), self.state.quick_view));
        }
        // Ctrl+Q on something that is not music is the quick view as ever, and the music plays on.
        self.point_at(PaneId::Right, "b.txt");
        ctrl(self, Key::q);
        if !self.state.quick_view || !self.audio_is_mini() || self.smoke_track() != "other2.wav" {
            errors.push("music: ctrl+q on a text file did not open the quick view next to the small player".into());
        }
        ctrl(self, Key::q);
        if self.state.quick_view {
            errors.push("music: ctrl+q did not close the quick view".into());
        }
        // Send goes to the small player when that is the open one, and starts a list there.
        self.smoke_mark(PaneId::Right, &["other2.wav", "other.wav"]);
        self.point_at(PaneId::Right, "b.txt");
        ctrl(self, Key::p);
        if !self.audio_is_mini() || self.state.quick_view || self.audio_list_len() != 2 || self.smoke_track() != "other.wav" {
            errors.push(format!(
                "music: ctrl+p with the small player open gave mini {}, quick view {}, {} songs, track {}",
                self.audio_is_mini(),
                self.state.quick_view,
                self.audio_list_len(),
                self.smoke_track()
            ));
        }
        self.state.tab_mut(PaneId::Right).marked.clear();

        // A picked playlist can reach into another folder. Expand shows the folder of the song that plays.
        self.state.active = PaneId::Left;
        let left_dir = self.state.tab(PaneId::Left).path.clone();
        self.smoke_mark(PaneId::Left, &["tone.wav"]);
        self.audio_add(false, sender);
        self.audio_next(sender);
        self.audio_next(sender);
        self.state.tab_mut(PaneId::Left).marked.clear();
        if self.smoke_track() != "tone.wav" || self.audio_list_len() != 3 {
            errors.push(format!("music: a playlist across folders gave {} songs and track {}", self.audio_list_len(), self.smoke_track()));
        }
        self.state.navigate(PaneId::Right, left_dir, true);
        self.reload_pane(PaneId::Right);

        // Expand: the list shows the folder with the cursor on the track, and the quick view opens on the big player.
        self.audio_expand(sender);
        self.smoke_settle(sender);
        self.smoke_settle(sender);
        if !self.audio_full() || !self.state.quick_view || self.shell.panes[0].mini.root.is_visible() || self.shell.panes[1].mini.root.is_visible() {
            errors.push(format!("music: expand gave full {}, quick view {}", self.audio_full(), self.state.quick_view));
        }
        let cursor = self.state.cursor_entry(PaneId::Right).map(|entry| entry.name.to_string()).unwrap_or_default();
        if cursor != "tone.wav" {
            errors.push(format!("music: expand left the cursor on {cursor}"));
        }

        // X and Delete close the small player from the keyboard.
        self.state.active = PaneId::Left;
        for (name, key) in [("x", Key::x), ("delete", Key::Delete)] {
            if !self.audio_alive() {
                self.point_at(PaneId::Left, "tone.wav");
                press(self, Key::Return);
            }
            self.audio_minimize(sender);
            self.smoke_settle(sender);
            ctrl(self, Key::m);
            self.smoke_settle(sender);
            if !self.audio_is_mini() || !self.audio_focused() {
                errors.push(format!("music: {name}: the small player did not have the keys"));
                continue;
            }
            press(self, key);
            self.smoke_settle(sender);
            if self.audio_alive() || self.audio_focused() || self.shell.panes.iter().any(|pane| pane.mini.root.is_visible()) {
                errors.push(format!("music: {name} did not close the small player"));
            }
        }
    }
}

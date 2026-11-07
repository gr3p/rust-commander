use relm4::gtk::gdk::{Key, ModifierType};
use relm4::gtk::prelude::*;
use relm4::prelude::*;

use rust_commander::audio::State;
use rust_commander::state::PaneId;

use super::App;
use crate::ui::msg::Msg;

type Wait<'a> = &'a dyn Fn(&mut dyn FnMut() -> bool) -> bool;

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

    /// Next and previous, the modes, the small player and the way back, on three tracks in one folder.
    pub(in crate::ui::app) fn smoke_playlist(&mut self, errors: &mut Vec<String>, sender: &ComponentSender<Self>, wait: Wait) {
        let press = |app: &mut Self, key: Key| app.on_key(key, ModifierType::empty(), sender);

        press(self, Key::n);
        if !self.smoke_together("tone2.wav") {
            errors.push(format!("music: next gave cursor {} and track {}", self.smoke_cursor(), self.smoke_track()));
        }
        // The cursor and the player already agree, so settling them changes nothing.
        self.reconcile_audio(sender);
        if self.smoke_track() != "tone2.wav" {
            errors.push("music: next was undone".into());
        }
        if !wait(&mut || self.audio_player_state() == Some(State::Playing)) {
            errors.push("music: next did not play".into());
        }
        press(self, Key::n);
        // The last track of a list that does not repeat stays.
        press(self, Key::n);
        if !self.smoke_together("tone3.wav") {
            errors.push(format!("music: the last track moved on to {}", self.smoke_track()));
        }
        press(self, Key::p);
        if !self.smoke_together("tone2.wav") {
            errors.push(format!("music: previous gave cursor {} and track {}", self.smoke_cursor(), self.smoke_track()));
        }
        // The other way round: the cursor moves, and the player follows it.
        self.state.move_cursor(1);
        self.note_preview_target();
        self.kick_preview(sender);
        self.reconcile_audio(sender);
        if self.smoke_track() != "tone3.wav" {
            errors.push(format!("music: the cursor did not steer the player, track {}", self.smoke_track()));
        }
        press(self, Key::p);
        if !self.smoke_together("tone2.wav") {
            errors.push(format!("music: previous after a cursor move gave {} and {}", self.smoke_cursor(), self.smoke_track()));
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

        // Minimize: the quick view closes, and the bar sits under the list the music came from.
        self.audio_minimize(sender);
        self.reconcile_audio(sender);
        self.kick_preview(sender);
        self.sync();
        self.paint_audio();
        if !self.audio_is_mini() || self.state.quick_view || self.audio_keys() {
            errors.push("music: minimize did not fold the player".into());
        }
        if !self.shell.panes[0].mini.root.is_visible() || self.shell.panes[1].mini.root.is_visible() {
            errors.push("music: the small player is not under the left list".into());
        }
        if self.shell.panes[0].mini.looks().title != "tone2" {
            errors.push(format!("music: small player title {:?}", self.shell.panes[0].mini.looks().title));
        }
        // The small player plays on, and the lists work: a file that is not music does not stop it.
        if !self.point_at(PaneId::Left, "note.txt") {
            errors.push("music: missing note.txt".into());
        }
        self.note_preview_target();
        self.reconcile_audio(sender);
        if !self.audio_is_mini() || self.smoke_track() != "tone2.wav" {
            errors.push("music: the small player followed the cursor".into());
        }
        self.point_at(PaneId::Left, "tone2.wav");

        // Tab goes left, right, player, left.
        press(self, Key::Tab);
        if self.state.active != PaneId::Right {
            errors.push("music: tab did not reach the right panel".into());
        }
        press(self, Key::Tab);
        if !self.audio_focused() {
            errors.push("music: tab did not reach the small player".into());
        }
        // Keys now belong to the player: Space pauses and does not mark.
        press(self, Key::space);
        if !wait(&mut || self.audio_player_state() == Some(State::Paused)) {
            errors.push("music: space did not pause the small player".into());
        }
        press(self, Key::space);
        if !self.state.tab(PaneId::Left).marked.is_empty() || !self.state.tab(PaneId::Right).marked.is_empty() {
            errors.push("music: space marked a file while the small player had the keys".into());
        }
        press(self, Key::Tab);
        if self.audio_focused() || self.state.active != PaneId::Left {
            errors.push("music: tab did not come back to the left panel".into());
        }
        // Ctrl+M asks for the player, and Escape gives the keys back.
        self.on_key(Key::m, ModifierType::CONTROL_MASK, sender);
        if !self.audio_focused() {
            errors.push("music: ctrl+m did not reach the small player".into());
        }
        press(self, Key::Escape);
        if self.audio_focused() {
            errors.push("music: escape kept the keys".into());
        }
        // A key that is not the player's goes to the list, and the player lets go.
        self.on_key(Key::m, ModifierType::CONTROL_MASK, sender);
        let cursor = self.state.tab(PaneId::Left).cursor;
        press(self, Key::Page_Down);
        if self.audio_focused() || self.state.tab(PaneId::Left).cursor == cursor {
            errors.push("music: a list key was swallowed by the small player".into());
        }
        self.point_at(PaneId::Left, "tone2.wav");

        // Expand: the list shows the track under the cursor, and the quick view opens on it.
        self.audio_expand(sender);
        self.reconcile_audio(sender);
        self.kick_preview(sender);
        self.sync();
        self.paint_audio();
        if !self.audio_keys() || !self.state.quick_view || !self.smoke_together("tone2.wav") {
            errors.push(format!(
                "music: expand gave keys {}, quick view {}, cursor {}, track {}",
                self.audio_keys(),
                self.state.quick_view,
                self.smoke_cursor(),
                self.smoke_track()
            ));
        }
        if self.shell.panes[0].mini.root.is_visible() {
            errors.push("music: the small player stayed after expand".into());
        }

        // A filter changes what the list shows, and the playlist follows it at the next step.
        self.on_msg(Msg::FilterChanged(0, "tone3".into()), sender);
        self.sync();
        // The filter sends the cursor to the top. Back on the track, the player carries on.
        self.point_at(PaneId::Left, "tone3.wav");
        self.reconcile_audio(sender);
        press(self, Key::p);
        if self.audio_list_len() != 1 {
            errors.push(format!("music: the playlist kept {} tracks under a filter of one", self.audio_list_len()));
        }
        // Expanding with a filter on clears it, and the cursor stays on the track instead of jumping to the top.
        self.audio_minimize(sender);
        self.audio_expand(sender);
        self.on_msg(Msg::FilterChanged(0, String::new()), sender);
        self.reconcile_audio(sender);
        self.sync();
        if !self.audio_keys() || !self.smoke_together("tone3.wav") {
            errors.push(format!("music: expand under a filter gave cursor {} and track {}", self.smoke_cursor(), self.smoke_track()));
        }
        // The filter box was shown by the test. The checks after this one start from a hidden one.
        self.shell.panes[0].filter.set_visible(false);
        self.sync();

        self.smoke_mini_play(errors, sender, wait);
    }

    /// Enter and Ctrl+Q play songs in the small player, and Ctrl+arrows seek in both players.
    fn smoke_mini_play(&mut self, errors: &mut Vec<String>, sender: &ComponentSender<Self>, wait: Wait) {
        let press = |app: &mut Self, key: Key| app.on_key(key, ModifierType::empty(), sender);
        let ctrl = |app: &mut Self, key: Key| app.on_key(key, ModifierType::CONTROL_MASK, sender);

        // Enter in the big player keeps the song going. It does not open another program.
        press(self, Key::Return);
        if !self.audio_keys() || self.smoke_track() != "tone3.wav" {
            errors.push(format!("music: enter in the big player changed the track to {}", self.smoke_track()));
        }
        if !wait(&mut || self.audio_player_state() == Some(State::Playing)) {
            errors.push("music: enter in the big player did not play".into());
        }
        // Ctrl+arrows belong to the big player. With the left panel active, Ctrl+Right would have mirrored it.
        let right_before = self.state.tab(PaneId::Right).path.clone();
        let (at, left_cursor) = (self.audio_position(), self.state.tab(PaneId::Left).cursor);
        ctrl(self, Key::Right);
        if self.state.tab(PaneId::Right).path != right_before || self.state.tab(PaneId::Left).cursor != left_cursor {
            errors.push("music: ctrl+right went to the panels while the big player was up".into());
        }
        // Plain arrows leave the song alone.
        press(self, Key::Left);
        if self.audio_position() + std::time::Duration::from_secs(1) < at {
            errors.push("music: a plain arrow seeked".into());
        }

        // A song in another folder, in the other panel.
        let sub = self.state.tab(PaneId::Left).path.join("sub");
        self.audio_minimize(sender);
        self.state.navigate(PaneId::Right, sub, true);
        self.reload_pane(PaneId::Right);
        self.sync();
        self.point_at(PaneId::Right, "other.wav");
        self.reconcile_audio(sender);
        ctrl(self, Key::q);
        self.reconcile_audio(sender);
        self.sync();
        self.paint_audio();
        if self.state.quick_view || !self.audio_is_mini() || self.smoke_track() != "other.wav" {
            errors.push(format!("music: ctrl+q over the small player gave quick view {} and track {}", self.state.quick_view, self.smoke_track()));
        }
        if self.shell.panes[0].mini.root.is_visible() || !self.shell.panes[1].mini.root.is_visible() {
            errors.push("music: the small player did not move under the right list".into());
        }
        // Ctrl+Q on the song that already plays changes nothing.
        let generation = self.audio_track().map(ToOwned::to_owned);
        ctrl(self, Key::q);
        if self.state.quick_view || self.audio_track().map(ToOwned::to_owned) != generation {
            errors.push("music: ctrl+q on the playing song did something".into());
        }
        // Enter on another song of that folder plays it in the small player.
        self.point_at(PaneId::Right, "other2.wav");
        press(self, Key::Return);
        self.reconcile_audio(sender);
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

        // The small player takes Ctrl+arrows too, and keeps the keys.
        ctrl(self, Key::m);
        if !self.audio_focused() {
            errors.push("music: ctrl+m did not reach the small player".into());
        }
        let right_after = self.state.tab(PaneId::Right).path.clone();
        let cursor = self.state.tab(PaneId::Right).cursor;
        ctrl(self, Key::Right);
        press(self, Key::Right);
        if !self.audio_focused() || self.state.tab(PaneId::Right).path != right_after || self.state.tab(PaneId::Right).cursor != cursor {
            errors.push("music: ctrl+right left the small player".into());
        }
        press(self, Key::Escape);

        // Back to the start: the song in the folder of the left panel stops, and the right panel goes home.
        self.audio_close();
        self.state.navigate(PaneId::Right, right_before, true);
        self.reload_pane(PaneId::Right);
        self.state.active = PaneId::Left;
        self.sync();
    }
}

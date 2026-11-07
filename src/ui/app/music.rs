use std::cell::Cell;
use std::rc::Rc;

use relm4::gtk;
use relm4::gtk::prelude::*;

use rust_commander::audio::{Repeat, VisualStyle};
use rust_commander::config::Config;

use crate::ui::msg::Msg;

use super::App;
use super::settings::{note, paint_switch, section, settings_row};

/// The Music page of the settings: the same choices the cogwheel in the player has, and the playback modes.
pub(in crate::ui::app) struct MusicUi {
    visual: gtk::Label,
    style: gtk::DropDown,
    shuffle: gtk::Label,
    repeat: gtk::DropDown,
    // Set while the window sets a choice itself, so that does not come back as a pick.
    guard: Rc<Cell<bool>>,
}

const REPEATS: [(Repeat, &str); 3] = [(Repeat::Off, "Off"), (Repeat::All, "All tracks"), (Repeat::One, "One track")];

fn switch_row(title: &str, on: bool, tx: &relm4::Sender<Msg>, msg: fn() -> Msg) -> (gtk::Box, gtk::Label) {
    let (row, value) = settings_row(title, on);
    let (tx, flash) = (tx.clone(), row.clone());
    let click = gtk::GestureClick::new();
    click.connect_released(move |_, _, _, _| {
        crate::ui::shell::blink_widget(flash.upcast_ref());
        tx.emit(msg());
    });
    row.add_controller(click);
    (row, value)
}

fn choice_row(title: &str, labels: &[&str], selected: usize, guard: &Rc<Cell<bool>>, pick: impl Fn(usize) + 'static) -> (gtk::Box, gtk::DropDown) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("settings-row");
    let name = gtk::Label::new(Some(title));
    name.set_xalign(0.0);
    name.set_width_chars(12);
    let drop = gtk::DropDown::from_strings(labels);
    drop.add_css_class("editor-pick");
    drop.set_hexpand(true);
    drop.set_selected(selected as u32);
    let guard = Rc::clone(guard);
    drop.connect_selected_notify(move |drop| {
        if !guard.get() {
            pick(drop.selected() as usize);
        }
    });
    row.append(&name);
    row.append(&drop);
    (row, drop)
}

fn style_at(style: VisualStyle) -> usize {
    VisualStyle::ALL.iter().position(|one| *one == style).unwrap_or(0)
}

fn repeat_at(repeat: Repeat) -> usize {
    REPEATS.iter().position(|(one, _)| *one == repeat).unwrap_or(0)
}

pub(in crate::ui::app) fn music_page(cfg: &Config, tx: &relm4::Sender<Msg>) -> (gtk::Box, MusicUi) {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let guard = Rc::new(Cell::new(false));
    page.append(&note("The built-in player opens with Ctrl+Q on a music file. The small cogwheel in the player has the visualizer choices too."));
    page.append(&section("Visualizer"));
    let (visual_row, visual) = switch_row("Show the visualizer", cfg.player_visual_on, tx, || Msg::AudioVisualToggle);
    let labels: Vec<&str> = VisualStyle::ALL.iter().map(|style| style.label()).collect();
    let pick = tx.clone();
    let (style_row, style) = choice_row("Style", &labels, style_at(cfg.visual_style()), &guard, move |at| {
        if let Some(style) = VisualStyle::ALL.get(at) {
            pick.emit(Msg::AudioVisualStyle(*style));
        }
    });
    page.append(&visual_row);
    page.append(&style_row);
    page.append(&note("The colors come from the theme. Wave follows the sound itself, Bars, Radial and Ribbon follow its pitch."));
    page.append(&section("Playback"));
    let (shuffle_row, shuffle) = switch_row("Shuffle    S", cfg.player_shuffle, tx, || Msg::AudioShuffle);
    let labels: Vec<&str> = REPEATS.iter().map(|(_, label)| *label).collect();
    let pick = tx.clone();
    let (repeat_row, repeat) = choice_row("Repeat    R", &labels, repeat_at(cfg.repeat()), &guard, move |at| {
        if let Some((repeat, _)) = REPEATS.get(at) {
            pick.emit(Msg::AudioRepeatSet(*repeat));
        }
    });
    page.append(&shuffle_row);
    page.append(&repeat_row);
    page.append(&note("The player plays the music files of the folder it started in. Next and Previous move the cursor along."));
    (page, MusicUi { visual, style, shuffle, repeat, guard })
}

impl App {
    /// Shows the player's modes in the settings window, if it is open.
    pub(in crate::ui::app) fn paint_music_settings(&self) {
        let Some(ui) = self.settings.as_ref().map(|settings| &settings.music) else { return };
        paint_switch(&ui.visual, self.cfg.player_visual_on);
        paint_switch(&ui.shuffle, self.cfg.player_shuffle);
        ui.guard.set(true);
        ui.style.set_selected(style_at(self.cfg.visual_style()) as u32);
        ui.style.set_sensitive(self.cfg.player_visual_on);
        ui.repeat.set_selected(repeat_at(self.cfg.repeat()) as u32);
        ui.guard.set(false);
    }
}

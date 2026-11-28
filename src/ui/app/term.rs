use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;

use relm4::gtk;
use relm4::gtk::prelude::*;
use relm4::prelude::*;
use vte4::prelude::{TerminalExt, TerminalExtManual};
use vte4::Terminal;

use rust_commander::state::PaneId;
use rust_commander::theme;

use super::shell::PanelShell;
use super::App;
use crate::ui::msg::Msg;

const SHELL_LIMIT: usize = 9;

impl App {
    pub(in crate::ui::app) fn toggle_panel_terminal(&mut self, sender: &ComponentSender<Self>) {
        if self.state.panel_terminal {
            self.cancel_shell_hold();
            self.state.panel_terminal = false;
            self.focus_list();
            return;
        }
        if self.shell.shells.is_empty() {
            self.new_shell(sender);
            return;
        }
        if self.shell.editing.get() && !self.leave_quick_edit(sender) {
            return;
        }
        self.open_current_shell();
    }

    pub(in crate::ui::app) fn new_shell(&mut self, sender: &ComponentSender<Self>) {
        if self.shell.shells.len() >= SHELL_LIMIT {
            self.status = "Nine shells already".into();
            return;
        }
        if self.shell.editing.get() && !self.leave_quick_edit(sender) {
            return;
        }
        if self.shell.shell_pane.is_none() {
            self.shell.shell_pane = Some(self.state.active.other().index());
        }
        let cwd = self.state.tab(self.file_pane()).path.clone();
        let id = self.shell.shell_seq;
        self.shell.shell_seq += 1;
        let term = Terminal::new();
        term.add_css_class("panel-shell");
        term.set_focusable(true);
        term.set_hexpand(true);
        term.set_vexpand(true);
        // Right click pastes, as in PuTTY. Capture phase, or VTE claims the click first.
        let paste = gtk::GestureClick::new();
        paste.set_button(gtk::gdk::BUTTON_SECONDARY);
        paste.set_propagation_phase(gtk::PropagationPhase::Capture);
        let target = term.clone();
        paste.connect_pressed(move |gesture, _, _, _| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            target.grab_focus();
            target.paste_clipboard();
        });
        term.add_controller(paste);
        let tx = sender.input_sender().clone();
        let window = self.shell.window.clone();
        let pick = gtk::EventControllerLegacy::new();
        pick.set_propagation_phase(gtk::PropagationPhase::Capture);
        // Capture runs before VTE takes the click, so we still know the file list had the keys. Proceed, or a drag cannot select text.
        pick.connect_event(move |_, event| {
            if event.event_type() != gtk::gdk::EventType::ButtonPress {
                return gtk::glib::Propagation::Proceed;
            }
            let button = event.downcast_ref::<gtk::gdk::ButtonEvent>().map(|event| event.button()).unwrap_or(0);
            if button != gtk::gdk::BUTTON_PRIMARY {
                return gtk::glib::Propagation::Proceed;
            }
            let from_files = gtk::prelude::GtkWindowExt::focus(&window).is_none_or(|widget| !widget.has_css_class("panel-shell"));
            tx.emit(Msg::SelectShell(id, from_files));
            gtk::glib::Propagation::Proceed
        });
        term.add_controller(pick);
        // A finished mouse selection is only on PRIMARY. Put it on the clipboard as well.
        term.connect_selection_changed(|term| {
            if term.has_selection() {
                term.copy_clipboard_format(vte4::Format::Text);
            }
        });
        let name = theme::active_name(&self.cfg.theme, &rust_commander::config::active_theme_path());
        super::shell::paint_terminal(&term, &theme::load_terminal(&name, &self.cfg.background_color));
        let tile = gtk::Box::new(gtk::Orientation::Vertical, 0);
        tile.add_css_class("shell-tile");
        tile.set_hexpand(true);
        tile.set_vexpand(true);
        tile.append(&term);
        let button = gtk::Button::new();
        button.add_css_class("drive");
        button.set_focus_on_click(false);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let number = gtk::Label::new(Some("1"));
        number.add_css_class("drive-num");
        let caption = gtk::Label::new(Some(&dir_caption(&cwd)));
        caption.set_ellipsize(gtk::pango::EllipsizeMode::End);
        caption.set_max_width_chars(12);
        row.append(&number);
        row.append(&caption);
        button.set_child(Some(&row));
        let tx = sender.input_sender().clone();
        let blink = button.clone();
        let window = self.shell.window.clone();
        button.connect_clicked(move |_| {
            let from_files = gtk::prelude::GtkWindowExt::focus(&window).is_none_or(|widget| !widget.has_css_class("panel-shell"));
            let tx = tx.clone();
            let blink = blink.clone();
            super::shell::blink_then(blink.upcast_ref(), move || tx.emit(Msg::SelectShell(id, from_files)));
        });
        let pid = Rc::new(Cell::new(None));
        let tx = sender.input_sender().clone();
        term.connect_child_exited(move |_, _| tx.emit(Msg::ShellExited(id)));
        self.shell.shell_buttons.append(&button);
        spawn_shell(&term, &cwd, id, sender.input_sender().clone(), Rc::clone(&pid));
        self.shell.shells.push(PanelShell {
            id,
            term: term.clone(),
            tile,
            button,
            number,
            caption,
            cwd,
            pid,
        });
        self.shell.shell_current = Some(id);
        self.open_current_shell();
    }

    pub(in crate::ui::app) fn close_current_shell(&mut self) {
        let Some(id) = self.shell.shell_current else { return };
        let Some(pid) = self.shell.shells.iter().find(|shell| shell.id == id).and_then(|shell| shell.pid.get()) else {
            self.on_shell_exited(id);
            return;
        };
        unsafe {
            libc::kill(pid, libc::SIGHUP);
        }
    }

    pub(in crate::ui::app) fn shell_has_focus(&self) -> bool {
        gtk::prelude::GtkWindowExt::focus(&self.shell.window).is_some_and(|widget| widget.has_css_class("panel-shell"))
    }

    // Tab enters the shell from the file list, Ctrl+Tab leaves it. The shells stay on the pane where the first one opened.
    pub(in crate::ui::app) fn focus_across_shell(&mut self) {
        if self.shell_has_focus() {
            self.cancel_shell_hold();
            self.focus_list();
            let pane = self.state.active.index();
            super::shell::blink_frame(&self.shell.panes[pane].frame, pane, false);
            return;
        }
        if self.shell.shells.is_empty() {
            return;
        }
        self.open_current_shell();
        if let Some(pane) = self.shell.shell_pane {
            super::shell::blink_frame(&self.shell.panes[pane].frame, pane, false);
        }
    }

    pub(in crate::ui::app) fn show_shell_index(&mut self, index: usize, sender: &ComponentSender<Self>) {
        let Some(shell) = self.shell.shells.get(index) else { return };
        let id = shell.id;
        let button = shell.button.clone();
        let from_files = !self.shell_has_focus();
        let tx = sender.input_sender().clone();
        super::shell::blink_then(button.upcast_ref(), move || tx.emit(Msg::SelectShell(id, from_files)));
    }

    pub(in crate::ui::app) fn select_shell(&mut self, id: u64, from_files: bool) {
        if !self.shell.shells.iter().any(|shell| shell.id == id) {
            return;
        }
        if self.shell.shell_current == Some(id) && self.state.panel_terminal && !from_files && self.shell_has_focus() {
            return;
        }
        let changed = self.shell.shell_current != Some(id);
        if changed && !from_files {
            if let Some(shell) = self.shell.shells.iter().find(|shell| shell.id == id) {
                super::shell::blink_widget(shell.tile.upcast_ref());
            }
        }
        self.shell.shell_current = Some(id);
        self.open_current_shell();
        if from_files {
            if let Some(pane) = self.shell.shell_pane {
                super::shell::blink_frame(&self.shell.panes[pane].frame, pane, false);
            }
        }
    }

    pub(in crate::ui::app) fn on_shell_exited(&mut self, id: u64) {
        let Some(pos) = self.shell.shells.iter().position(|shell| shell.id == id) else { return };
        let was_current = self.shell.shell_current == Some(id);
        let shell = self.shell.shells.remove(pos);
        if let Some(parent) = shell.tile.parent() {
            if let Ok(grid) = parent.downcast::<gtk::Grid>() {
                grid.remove(&shell.tile);
            }
        }
        self.shell.shell_buttons.remove(&shell.button);
        if self.shell.shells.is_empty() {
            self.cancel_shell_hold();
            self.shell.shell_current = None;
            self.shell.shell_pane = None;
            self.state.panel_terminal = false;
            self.focus_list();
            return;
        }
        if was_current {
            let next = pos.min(self.shell.shells.len() - 1);
            self.shell.shell_current = Some(self.shell.shells[next].id);
            if self.state.panel_terminal {
                self.open_current_shell();
                return;
            }
        }
        self.layout_shells();
    }

    fn open_current_shell(&mut self) {
        if self.shell.shell_current.is_none() {
            self.shell.shell_current = self.shell.shells.first().map(|shell| shell.id);
        }
        self.state.quick_view = false;
        self.state.panel_terminal = true;
        // The shell pane never takes the accent, or the shells would jump.
        if let Some(pane) = self.shell.shell_pane {
            if self.state.active.index() == pane {
                self.state.active = self.state.active.other();
            }
        }
        self.layout_shells();
        self.arm_shell_hold();
        self.grab_shell();
    }

    fn arm_shell_hold(&self) {
        let n = self.shell.shell_epoch.get().wrapping_add(1);
        self.shell.shell_epoch.set(n);
        self.shell.shell_hold.set(true);
    }

    fn cancel_shell_hold(&self) {
        let n = self.shell.shell_epoch.get().wrapping_add(1);
        self.shell.shell_epoch.set(n);
        self.shell.shell_hold.set(false);
    }

    pub(in crate::ui::app) fn layout_shells(&self) {
        let Some(pane) = self.shell.shell_pane else { return };
        if self.shell.shells.is_empty() || pane > 1 {
            return;
        }
        let grid = shell_grid(&self.shell.panes[pane].terminal_slot);
        let (cols, _) = grid_shape(self.shell.shells.len());
        let cols = cols as usize;
        for (index, shell) in self.shell.shells.iter().enumerate() {
            place_tile(&grid, &shell.tile, (index % cols) as i32, (index / cols) as i32);
        }
    }

    pub(in crate::ui::app) fn grab_shell(&self) {
        let id = self.shell.shell_current;
        let Some(shell) = self.shell.shells.iter().find(|shell| Some(shell.id) == id) else { return };
        let term = shell.term.clone();
        let epoch = Rc::clone(&self.shell.shell_epoch);
        let want = epoch.get();
        term.grab_focus();
        gtk::glib::idle_add_local_once(move || {
            if epoch.get() == want {
                term.grab_focus();
            }
        });
    }

    // The file pane opposite the shells. A new shell starts there, as that pane stands right now.
    fn file_pane(&self) -> PaneId {
        self.shell
            .shell_pane
            .map(|pane| PaneId::from_index(pane).other())
            .unwrap_or(self.state.active)
    }

    pub(in crate::ui::app) fn restamp_shells(&self) {
        let current = self.state.panel_terminal.then_some(self.shell.shell_current).flatten();
        for (index, shell) in self.shell.shells.iter().enumerate() {
            let n = (index + 1).to_string();
            if shell.number.text().as_str() != n {
                shell.number.set_text(&n);
            }
            let name = dir_caption(&shell.cwd);
            if shell.caption.text().as_str() != name {
                shell.caption.set_text(&name);
            }
            let tip = rust_commander::commands::format_keys(&format!("Alt+{n}"));
            if shell.button.tooltip_text().as_deref() != Some(tip.as_str()) {
                shell.button.set_tooltip_text(Some(&tip));
            }
            let on = current == Some(shell.id);
            for widget in [shell.button.upcast_ref::<gtk::Widget>(), shell.tile.upcast_ref::<gtk::Widget>()] {
                if widget.has_css_class("current") != on {
                    if on {
                        widget.add_css_class("current");
                    } else {
                        widget.remove_css_class("current");
                    }
                }
            }
        }
        let buttons = !self.shell.shells.is_empty();
        if self.shell.shell_buttons.is_visible() != buttons {
            self.shell.shell_buttons.set_visible(buttons);
        }
        // Mount sits on this row even when no disk is mounted.
        if !self.shell.shell_bar.is_visible() {
            self.shell.shell_bar.set_visible(true);
        }
        let key = if self.state.panel_terminal { "F9 Shell Toggle" } else { "F9 Shell" };
        if self.shell.shell_key.text().as_str() != key {
            self.shell.shell_key.set_text(key);
        }
    }
}

fn spawn_shell(term: &Terminal, cwd: &Path, id: u64, tx: relm4::Sender<Msg>, pid_slot: Rc<Cell<Option<i32>>>) {
    let shell = super::shell_bin();
    let cwd = cwd.to_string_lossy().into_owned();
    let env_owned: Vec<String> = std::env::vars().map(|(key, value)| format!("{key}={value}")).collect();
    let envv: Vec<&str> = env_owned.iter().map(|line| line.as_str()).collect();
    term.spawn_async(
        vte4::PtyFlags::DEFAULT,
        Some(cwd.as_str()),
        &[&shell],
        &envv,
        gtk::glib::SpawnFlags::DEFAULT,
        || {},
        -1,
        None::<&gtk::gio::Cancellable>,
        move |result| match result {
            Ok(pid) => pid_slot.set(Some(pid.0)),
            Err(err) => {
                rust_commander::log::info(&format!("panel terminal {id}: {err}"));
                tx.emit(Msg::ShellExited(id));
            }
        },
    );
}

fn dir_caption(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "/".into())
}

// 2 keeps the full line length. 3–4 is 2×2, 5–6 is 2×3, then 3×3.
fn grid_shape(n: usize) -> (i32, i32) {
    match n {
        0 | 1 => (1, 1),
        2 => (1, 2),
        3 | 4 => (2, 2),
        5 | 6 => (2, 3),
        _ => (3, 3),
    }
}

fn shell_grid(slot: &gtk::Box) -> gtk::Grid {
    if let Some(child) = slot.first_child() {
        if let Ok(grid) = child.downcast::<gtk::Grid>() {
            return grid;
        }
    }
    let grid = gtk::Grid::new();
    grid.set_row_homogeneous(true);
    grid.set_column_homogeneous(true);
    grid.set_hexpand(true);
    grid.set_vexpand(true);
    grid.set_row_spacing(4);
    grid.set_column_spacing(4);
    grid.set_margin_start(4);
    grid.set_margin_end(4);
    grid.set_margin_top(4);
    grid.set_margin_bottom(4);
    slot.append(&grid);
    grid
}

fn place_tile(grid: &gtk::Grid, tile: &impl IsA<gtk::Widget>, col: i32, row: i32) {
    let tile = tile.upcast_ref::<gtk::Widget>();
    if tile.parent().as_ref() == Some(grid.upcast_ref()) {
        let (c, r, w, h) = grid.query_child(tile);
        if c == col && r == row && w == 1 && h == 1 {
            return;
        }
        grid.remove(tile);
    } else if let Some(parent) = tile.parent() {
        if let Ok(old) = parent.downcast::<gtk::Grid>() {
            old.remove(tile);
        }
    }
    grid.attach(tile, col, row, 1, 1);
}

#[cfg(test)]
mod tests {
    use super::grid_shape;

    #[test]
    fn shells_shrink_in_a_fixed_grid() {
        assert_eq!(grid_shape(1), (1, 1));
        assert_eq!(grid_shape(2), (1, 2));
        assert_eq!(grid_shape(3), (2, 2));
        assert_eq!(grid_shape(4), (2, 2));
        assert_eq!(grid_shape(5), (2, 3));
        assert_eq!(grid_shape(6), (2, 3));
        assert_eq!(grid_shape(7), (3, 3));
        assert_eq!(grid_shape(9), (3, 3));
    }
}

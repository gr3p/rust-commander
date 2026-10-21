use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;

use relm4::gtk;
use relm4::gtk::prelude::*;
use relm4::prelude::*;
use vte4::prelude::{TerminalExt, TerminalExtManual};
use vte4::Terminal;

use rust_commander::theme;

use super::shell::PanelShell;
use super::App;
use crate::ui::msg::Msg;

const SHELL_LIMIT: usize = 9;

impl App {
    pub(in crate::ui::app) fn toggle_panel_terminal(&mut self, sender: &ComponentSender<Self>) {
        if self.state.panel_terminal {
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
        let cwd = self.state.tab(self.state.active).path.clone();
        let id = self.shell.shell_seq;
        self.shell.shell_seq += 1;
        let term = Terminal::new();
        term.add_css_class("panel-shell");
        term.set_hexpand(true);
        term.set_vexpand(true);
        term.set_margin_start(8);
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
        let name = theme::active_name(&self.cfg.theme, &rust_commander::config::active_theme_path());
        super::shell::paint_terminal(&term, &theme::load_terminal(&name, &self.cfg.background_color));
        let button = gtk::Button::new();
        button.add_css_class("drive");
        button.set_focus_on_click(false);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let number = gtk::Label::new(Some("1"));
        number.add_css_class("drive-num");
        let caption = gtk::Label::new(Some("sh"));
        row.append(&number);
        row.append(&caption);
        button.set_child(Some(&row));
        let tx = sender.input_sender().clone();
        button.connect_clicked(move |_| tx.emit(Msg::SelectShell(id)));
        let pid = Rc::new(Cell::new(None));
        let tx = sender.input_sender().clone();
        term.connect_child_exited(move |_, _| tx.emit(Msg::ShellExited(id)));
        self.shell.shell_buttons.append(&button);
        spawn_shell(&term, &cwd, id, sender.input_sender().clone(), Rc::clone(&pid));
        self.shell.shells.push(PanelShell { id, term: term.clone(), button, number, pid });
        self.shell.shell_current = Some(id);
        self.state.quick_view = false;
        self.state.panel_terminal = true;
        self.place_panel_terminal(self.state.active.other().index());
        term.grab_focus();
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

    // Tab enters the shell from the file list, Ctrl+Tab leaves it. The shell stays on the pane where it was opened.
    pub(in crate::ui::app) fn focus_across_shell(&mut self) {
        let pane = if self.shell_has_focus() {
            self.focus_list();
            self.state.active.index()
        } else {
            let id = self.shell.shell_current;
            if let Some(shell) = self.shell.shells.iter().find(|shell| Some(shell.id) == id) {
                shell.term.grab_focus();
            }
            self.state.active.other().index()
        };
        // The file pane is already accented, and the shell pane never becomes active, so sync will not blink.
        super::shell::blink_frame(&self.shell.panes[pane].frame, pane, false);
    }

    pub(in crate::ui::app) fn show_shell_index(&mut self, index: usize) {
        let Some(id) = self.shell.shells.get(index).map(|shell| shell.id) else { return };
        self.select_shell(id);
    }

    pub(in crate::ui::app) fn select_shell(&mut self, id: u64) {
        if !self.shell.shells.iter().any(|shell| shell.id == id) {
            return;
        }
        self.shell.shell_current = Some(id);
        self.open_current_shell();
    }

    pub(in crate::ui::app) fn on_shell_exited(&mut self, id: u64) {
        let Some(pos) = self.shell.shells.iter().position(|shell| shell.id == id) else { return };
        let shell = self.shell.shells.remove(pos);
        if let Some(parent) = shell.term.parent() {
            if let Ok(slot) = parent.downcast::<gtk::Box>() {
                slot.remove(&shell.term);
            }
        }
        self.shell.shell_buttons.remove(&shell.button);
        if self.shell.shell_current == Some(id) {
            self.shell.shell_current = None;
            self.state.panel_terminal = false;
            // Shell lived on the other pane; land there after quit.
            self.state.active = self.state.active.other();
            self.focus_list();
        }
    }

    fn open_current_shell(&mut self) {
        if self.shell.shell_current.is_none() {
            self.shell.shell_current = self.shell.shells.first().map(|shell| shell.id);
        }
        self.state.quick_view = false;
        self.state.panel_terminal = true;
        self.place_panel_terminal(self.state.active.other().index());
        let id = self.shell.shell_current;
        if let Some(shell) = self.shell.shells.iter().find(|shell| Some(shell.id) == id) {
            shell.term.grab_focus();
        }
    }

    pub(in crate::ui::app) fn place_panel_terminal(&self, pane: usize) {
        let Some(id) = self.shell.shell_current else { return };
        let Some(shell) = self.shell.shells.iter().find(|shell| shell.id == id) else { return };
        let slot = &self.shell.panes[pane].terminal_slot;
        let term = &shell.term;
        if term.parent().as_ref() == Some(slot.upcast_ref()) {
            return;
        }
        if let Some(parent) = term.parent() {
            if let Ok(box_) = parent.downcast::<gtk::Box>() {
                box_.remove(term);
            }
        }
        slot.append(term);
    }

    pub(in crate::ui::app) fn restamp_shells(&self) {
        let current = self.state.panel_terminal.then_some(self.shell.shell_current).flatten();
        for (index, shell) in self.shell.shells.iter().enumerate() {
            let n = (index + 1).to_string();
            if shell.number.text().as_str() != n {
                shell.number.set_text(&n);
            }
            let tip = rust_commander::commands::format_keys(&format!("Alt+{n}"));
            if shell.button.tooltip_text().as_deref() != Some(tip.as_str()) {
                shell.button.set_tooltip_text(Some(&tip));
            }
            let on = current == Some(shell.id);
            if shell.button.has_css_class("current") != on {
                if on {
                    shell.button.add_css_class("current");
                } else {
                    shell.button.remove_css_class("current");
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

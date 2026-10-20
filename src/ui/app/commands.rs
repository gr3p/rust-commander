use relm4::gtk;
use relm4::gtk::prelude::*;
use relm4::prelude::*;

use rust_commander::commands::{self, Act, Cue, RowAct};
use rust_commander::lister::{Charset, ViewMode};
use rust_commander::listing::SortKey;
use rust_commander::state::PaneId;

use super::{App, Ask};
use crate::ui::msg::Msg;

impl App {
    pub(in crate::ui::app) fn cue(&self) -> Cue {
        let id = self.state.active;
        let entry = self.state.cursor_entry(id);
        let name = entry.map(|entry| entry.name.as_ref()).unwrap_or("");
        let file = entry.is_some_and(|entry| !entry.is_dir() && entry.name.as_ref() != "..");
        let dir = entry.is_some_and(|entry| entry.is_dir() && entry.name.as_ref() != "..");
        let archive = file && rust_commander::archive::is_archive(std::path::Path::new(name));
        let testable = file && rust_commander::archive::is_testable(std::path::Path::new(name));
        let text = file && commands::is_text(name);
        let image = file && rust_commander::imageview::is_image(name);
        let sfv = self.state.tab(id).entries.iter().any(|entry| {
            !entry.is_dir() && super::jobs::is_check_file(std::path::Path::new(entry.name.as_ref()))
        });
        Cue {
            archive,
            text,
            image,
            sfv,
            marks: self.state.tab(id).marked.len(),
            file,
            dir,
            lister: self.lister.is_some(),
            image_view: self.image.is_some() || (self.state.quick_view && self.cursor_is_image()),
            quick_view: self.state.quick_view,
            editing: self.shell.editing.get(),
            drive: self.shell.drives.iter().any(|(path, _)| path == &self.state.tab(id).path),
            testable,
        }
    }

    pub(in crate::ui::app) fn toggle_commands(&mut self, sender: &ComponentSender<Self>) {
        if let Some(win) = self.command_list.take() {
            win.close();
            return;
        }
        let tx = sender.input_sender().clone();
        let win = super::settings::show_commands(&self.shell.window, self.cue(), tx.clone());
        let tx_close = tx;
        win.connect_close_request(move |_| {
            tx_close.emit(Msg::CommandsClosed);
            gtk::glib::Propagation::Proceed
        });
        self.command_list = Some(win);
    }

    pub(in crate::ui::app) fn copy_selected_paths(&mut self) {
        let paths = self.state.selected_paths(self.state.active);
        if paths.is_empty() {
            self.status = "Nothing selected".into();
            return;
        }
        let text = paths.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join("\n");
        let status = if paths.len() == 1 {
            "Path copied".into()
        } else {
            format!("{} paths copied", paths.len())
        };
        self.write_clipboard(&text, &status);
    }

    pub(in crate::ui::app) fn copy_panel_dir(&mut self) {
        let text = self.state.tab(self.state.active).path.display().to_string();
        self.write_clipboard(&text, "Path copied");
    }

    pub(in crate::ui::app) fn copy_shown_path(&mut self, pane: usize) {
        let text = self.shell.panes[pane].path.text().to_string();
        if text.is_empty() {
            return;
        }
        self.write_clipboard(&text, "Path copied");
    }

    fn write_clipboard(&mut self, text: &str, status: &str) {
        let Some(display) = gtk::gdk::Display::default() else { return };
        display.clipboard().set_text(text);
        self.status = status.into();
    }

    pub(in crate::ui::app) fn run_act(&mut self, act: Act, sender: &ComponentSender<Self>) {
        if !commands::applies(act, &self.cue()) {
            return;
        }
        let tx = sender.input_sender().clone();
        match act {
            Act::Refresh => self.refresh_active(sender),
            Act::View => self.open_viewer(sender),
            Act::Edit => self.open_external(false),
            Act::NewFile => {
                self.ask = Some(Ask::CreateFile);
                self.ask_text("New file", "", tx);
            }
            Act::Copy => self.on_fkey(5, false, sender),
            Act::Move => self.on_fkey(6, false, sender),
            Act::Queue => {}
            Act::Mkdir => self.on_fkey(7, false, sender),
            Act::Trash => self.on_fkey(8, false, sender),
            Act::Delete => self.on_fkey(8, true, sender),
            Act::CopyAs => self.on_fkey(5, true, sender),
            Act::Rename => self.on_fkey(6, true, sender),
            Act::SelectAll => {
                self.state.toggle_visible_marks();
                self.touch(self.state.active);
            }
            Act::Properties => self.show_properties(sender),
            Act::Settings => self.toggle_settings(sender),
            Act::Pack => self.start_pack(false, sender),
            Act::PackHere => self.start_pack(true, sender),
            Act::Unpack => self.begin_unpack(true, sender),
            Act::UnpackHere => self.begin_unpack(false, sender),
            Act::NewLink => self.ask_link(sender),
            Act::Shred => self.ask_shred(sender),
            Act::Encode => self.ask_encode(sender),
            Act::TestArchive => self.start_test(sender),
            Act::MakeSfv => self.ask_checksum(sender),
            Act::Verify => self.start_verify(sender),
            Act::OtherPane => {
                self.state.active = self.state.active.other();
                self.note_preview_target();
                self.focus_list();
            }
            Act::NextTab => {
                let id = self.state.active;
                self.state.next_tab(id, false);
                self.touch(id);
                self.note_preview_target();
            }
            Act::NewTab => self.open_new_tab(sender),
            Act::CloseTab => {
                if self.shell_has_focus() {
                    self.close_current_shell();
                } else {
                    let id = self.state.active;
                    self.state.close_tab(id);
                    self.touch(id);
                }
            }
            Act::Up => self.go_up(sender),
            Act::Back => self.history(true, sender),
            Act::Forward => self.history(false, sender),
            Act::Mirror => self.show_active_on_other(sender),
            Act::PlacesLeft => self.show_places(PaneId::Left, sender),
            Act::PlacesRight => self.show_places(PaneId::Right, sender),
            Act::Hotlist => self.toggle_hotlist(sender),
            Act::Path => self.focus_path(),
            Act::Open => self.activate_cursor(sender),
            Act::Mark => {
                let index = self.state.tab(self.state.active).cursor;
                self.state.toggle_mark();
                self.paint_mark(self.state.active, index);
                self.focus_list();
            }
            Act::MarkStay => {
                let index = self.state.tab(self.state.active).cursor;
                self.state.toggle_mark_stay();
                self.paint_mark(self.state.active, index);
                self.focus_list();
            }
            Act::SortName => self.set_sort(SortKey::Name),
            Act::SortExt => self.set_sort(SortKey::Ext),
            Act::SortDate => self.set_sort(SortKey::Time),
            Act::SortSize => self.set_sort(SortKey::Size),
            Act::SortAttr => self.set_sort(SortKey::Attr),
            Act::CopyPaths => self.copy_selected_paths(),
            Act::CopyDir => self.copy_panel_dir(),
            Act::Filter => self.filter_shortcut(),
            Act::Search => self.open_search(sender),
            Act::Hidden => self.toggle_hidden(),
            Act::Syntax => self.toggle_syntax(),
            Act::Thumbs => self.toggle_thumbs(),
            Act::QuickView => self.toggle_quick_view(sender),
            Act::Terminal => self.toggle_panel_terminal(sender),
            Act::NewShell => self.new_shell(sender),
            Act::CloseShell => self.close_current_shell(),
            Act::SwitchShell => self.status = "Press Alt+1 to Alt+9".into(),
            Act::ExternalView => self.open_external(true),
            Act::CloseView => {
                if self.shell.editing.get() {
                    self.leave_quick_edit(sender);
                } else if self.image.is_some() {
                    self.close_image();
                } else if self.lister.is_some() {
                    self.close_lister();
                } else {
                    self.dismiss();
                }
            }
            Act::ImageFit => self.toggle_shown_scale(sender),
            Act::ImageStep => {
                if self.image.is_some() {
                    self.step_image(1, sender);
                } else {
                    self.step_quick_image(1);
                }
            }
            Act::ListerText => self.lister_set_mode(ViewMode::Text, sender),
            Act::ListerBinary => self.lister_set_mode(ViewMode::Binary, sender),
            Act::ListerHex => self.lister_set_mode(ViewMode::Hex, sender),
            Act::ListerAnsi => self.set_charset(Charset::Ansi, sender),
            Act::ListerAscii => self.set_charset(Charset::Ascii, sender),
            Act::ListerJson => self.set_charset(Charset::Json, sender),
            Act::QuickEdit => self.toggle_quick_edit(sender),
            Act::Save => self.save_quick(),
            Act::Discard => self.discard_quick(sender),
            Act::LeaveEdit => {
                self.leave_quick_edit(sender);
            }
            Act::CopyImage => self.copy_preview_image(sender),
            Act::Drive => self.status = "Press Ctrl+1 to Ctrl+9".into(),
            Act::DiskInfo => self.show_disk_info(None),
            Act::Mount => self.open_mount(sender),
            Act::Commands => self.toggle_commands(sender),
        }
    }

    pub(in crate::ui::app) fn note_row_target(&mut self, pane: usize) {
        let id = PaneId::from_index(pane);
        self.row_target = (self.state.active == id)
            .then(|| self.state.cursor_entry(id).map(|entry| rust_commander::listing::entry_path(&self.state.tab(id).path, entry)))
            .flatten();
    }

    pub(in crate::ui::app) fn row_act(&mut self, pane: usize, act: RowAct, sender: &ComponentSender<Self>) {
        if self.hot.is_some() && PaneId::from_index(pane) != self.hot_pane {
            return;
        }
        self.shell.active.set(pane);
        self.state.active = PaneId::from_index(pane);
        let editing = self.shell.editing.get();
        // A reload while the menu was open may have moved the row. Act on the file that was clicked, or not at all.
        let id = self.state.active;
        let target = self.row_target.take();
        let pos = target.and_then(|target| {
            let tab = self.state.tab(id);
            self.state
                .visible(id)
                .iter()
                .position(|&i| rust_commander::listing::entry_path(&tab.path, &tab.entries[i]) == target)
        });
        let Some(pos) = pos else {
            self.status = "The file is no longer in the list".into();
            return;
        };
        self.state.set_cursor(pos);
        // A cursor move reloads quick view. Leave an open edit alone until it is closed.
        if editing {
            self.preview_sent = self.state.preview_gen;
        }
        match act {
            RowAct::Open => self.activate_cursor(sender),
            RowAct::QuickView => self.show_quick_view(sender),
            RowAct::QuickEdit => self.edit_quick_file(sender),
            RowAct::Copy => self.copy_one(sender),
            RowAct::Move => self.move_one(sender),
            RowAct::Rename => self.on_fkey(6, true, sender),
            RowAct::Pack => self.pack_one(sender),
            RowAct::Unpack => self.begin_unpack(true, sender),
            RowAct::Trash => self.trash_one(sender),
            RowAct::CopyPath => self.copy_cursor_path(),
            RowAct::Properties => self.show_properties(sender),
        }
    }

    fn copy_one(&mut self, sender: &ComponentSender<Self>) {
        let Some(src) = self.cursor_path() else {
            self.status = "Nothing selected".into();
            return;
        };
        let title = format!("Copy {} to", super::describe(std::slice::from_ref(&src)));
        let dest = self.state.tab(self.state.active.other()).path.display().to_string();
        self.ask = Some(Ask::Copy(vec![src]));
        self.ask_text(&title, &dest, sender.input_sender().clone());
    }

    fn move_one(&mut self, sender: &ComponentSender<Self>) {
        let Some(src) = self.cursor_path() else {
            self.status = "Nothing selected".into();
            return;
        };
        let title = format!("Move {} to", super::describe(std::slice::from_ref(&src)));
        let dest = self.state.tab(self.state.active.other()).path.display().to_string();
        self.ask = Some(Ask::Move(vec![src]));
        self.ask_text(&title, &dest, sender.input_sender().clone());
    }

    fn pack_one(&mut self, sender: &ComponentSender<Self>) {
        let Some(path) = self.cursor_path() else {
            self.status = "Nothing selected".into();
            return;
        };
        if self.job.is_some() {
            self.status = "Busy".into();
            return;
        }
        let stem = super::jobs::stem_of(&path).unwrap_or("archive");
        let dest = self.state.tab(self.state.active.other()).path.join(format!("{stem}.zip"));
        self.ask_pack(std::slice::from_ref(&path), &dest.display().to_string(), sender.input_sender().clone());
    }

    fn trash_one(&mut self, sender: &ComponentSender<Self>) {
        let Some(src) = self.cursor_path() else {
            self.status = "Nothing selected".into();
            return;
        };
        let name = src.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let trash = rust_commander::ops::on_home_disk(&src);
        self.ask = Some(if trash { Ask::Trash(vec![src]) } else { Ask::Permanent(vec![src]) });
        let title = if trash { format!("Move {name} to trash?") } else { format!("Delete {name} permanently?") };
        self.ask_confirm(&title, sender.input_sender().clone());
    }

    fn copy_cursor_path(&mut self) {
        let Some(path) = self.cursor_path() else {
            self.status = "Nothing selected".into();
            return;
        };
        self.write_clipboard(&path.display().to_string(), "Path copied");
    }
}

use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

use relm4::gtk;
use relm4::gtk::prelude::*;
use relm4::prelude::*;

use rust_commander::encode::Codec;
use rust_commander::sum::Kind;

use super::{App, Ask, Phase};
use crate::ui::msg::Msg;

impl App {
    pub(in crate::ui::app) fn ask_link(&mut self, sender: &ComponentSender<Self>) {
        let Some(target) = self.cursor_path() else {
            self.status = "Nothing selected".into();
            return;
        };
        let place = self.state.tab(self.state.active.other()).path.clone();
        let suggested = suggested_link_name(&target, &place);
        let target_is_file = target.is_file();
        let target_text = target.display().to_string();
        self.close_dialog();
        let win = gtk::Window::new();
        win.set_title(Some("New link"));
        win.set_modal(true);
        win.set_transient_for(Some(&self.shell.window));
        win.set_default_width(520);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        column.set_margin_all(12);
        let symbolic = gtk::CheckButton::with_label("Shortcut (symbolic link)");
        symbolic.set_active(true);
        let hard = gtk::CheckButton::with_label("Second name (hard link)");
        hard.set_group(Some(&symbolic));
        let points = gtk::Label::new(Some(&target_text));
        points.set_xalign(0.0);
        points.set_wrap(true);
        points.set_selectable(true);
        points.set_can_focus(false);
        let name_entry = gtk::Entry::new();
        name_entry.set_text(&suggested);
        let place_entry = gtk::Entry::new();
        place_entry.set_text(place.to_string_lossy().as_ref());
        let story = gtk::Label::new(None);
        story.set_xalign(0.0);
        story.set_wrap(true);
        let error = gtk::Label::new(None);
        error.set_wrap(true);
        error.add_css_class("error");
        error.set_visible(false);
        let refresh = std::rc::Rc::new({
            let target_text = target_text.clone();
            let (symbolic, name_entry, place_entry, story, error) = (symbolic.clone(), name_entry.clone(), place_entry.clone(), story.clone(), error.clone());
            move || {
                let symbolic = symbolic.is_active();
                story.set_text(&link_story(symbolic, name_entry.text().trim(), place_entry.text().trim(), &target_text));
                let problem = (!symbolic && !target_is_file).then_some("A hard link needs a file. Choose Shortcut for a folder.");
                error.set_text(problem.unwrap_or(""));
                error.set_visible(problem.is_some());
            }
        });
        refresh();
        for entry in [&name_entry, &place_entry] {
            let refresh = std::rc::Rc::clone(&refresh);
            entry.connect_changed(move |_| refresh());
        }
        for button in [&symbolic, &hard] {
            let refresh = std::rc::Rc::clone(&refresh);
            button.connect_toggled(move |_| refresh());
        }
        let ok = gtk::Button::with_label("Create");
        let tx = sender.input_sender().clone();
        let (symbolic_ok, name_ok, place_ok, error_ok) = (symbolic.clone(), name_entry.clone(), place_entry.clone(), error.clone());
        let target_ok = target_text.clone();
        crate::ui::shell::on_click(&ok, move || {
            let name = name_ok.text().trim().to_string();
            let place = place_ok.text().trim().to_string();
            if let Some(problem) = link_problem(symbolic_ok.is_active(), &name, &place, target_is_file) {
                error_ok.set_text(problem);
                error_ok.set_visible(true);
                return;
            }
            let link = PathBuf::from(&place).join(&name).display().to_string();
            tx.emit(Msg::MakeLink { symbolic: symbolic_ok.is_active(), target: target_ok.clone(), link });
        });
        let cancel = gtk::Button::with_label("Cancel");
        let tx = sender.input_sender().clone();
        crate::ui::shell::on_escape(&win, {
            let tx = tx.clone();
            move || tx.emit(Msg::CancelAsk)
        });
        crate::ui::shell::on_click(&cancel, move || tx.emit(Msg::CancelAsk));
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.append(&ok);
        row.append(&cancel);
        column.append(&symbolic);
        column.append(&hard);
        column.append(&field_label("Points at"));
        column.append(&points);
        column.append(&field_label("Name"));
        column.append(&name_entry);
        column.append(&field_label("Link location"));
        column.append(&place_entry);
        column.append(&story);
        column.append(&error);
        column.append(&row);
        win.set_child(Some(&column));
        win.present();
        name_entry.grab_focus();
        self.dialog = Some(win);
    }

    pub(in crate::ui::app) fn make_link(&mut self, symbolic: bool, target: String, link: String, sender: &ComponentSender<Self>) {
        self.close_dialog();
        let target = PathBuf::from(target);
        let link = PathBuf::from(link);
        let result = if symbolic {
            std::os::unix::fs::symlink(&target, &link)
        } else if !target.is_file() {
            Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "A hard link needs a file"))
        } else {
            match (std::fs::symlink_metadata(&target), link.parent().and_then(|dir| std::fs::metadata(dir).ok())) {
                (Ok(file), Some(dir)) if file.dev() != dir.dev() => {
                    Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "A hard link must stay on the same filesystem"))
                }
                (Ok(_), Some(_)) => std::fs::hard_link(&target, &link),
                _ => Err(std::io::Error::new(std::io::ErrorKind::NotFound, "Target or folder is missing")),
            }
        };
        match result {
            Ok(()) => {
                self.status = "Link created".into();
                self.refresh_both(sender);
            }
            Err(err) => self.status = err.to_string(),
        }
    }

    pub(in crate::ui::app) fn ask_checksum(&mut self, sender: &ComponentSender<Self>) {
        let sources = self.state.selected_paths(self.state.active);
        if sources.is_empty() {
            self.status = "Nothing selected".into();
            return;
        }
        self.close_dialog();
        let win = gtk::Window::new();
        win.set_title(Some("Checksum"));
        win.set_modal(true);
        win.set_transient_for(Some(&self.shell.window));
        win.set_default_width(360);
        let kinds = gtk::DropDown::from_strings(&["CRC32 (SFV)", "MD5", "SHA1", "SHA256"]);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        column.set_margin_all(12);
        column.append(&kinds);
        let ok = gtk::Button::with_label("Create");
        let tx = sender.input_sender().clone();
        let kinds_ok = kinds.clone();
        crate::ui::shell::on_click(&ok, move || tx.emit(Msg::SumKind(kinds_ok.selected() as u8)));
        let cancel = gtk::Button::with_label("Cancel");
        let tx = sender.input_sender().clone();
        crate::ui::shell::on_escape(&win, {
            let tx = tx.clone();
            move || tx.emit(Msg::CancelAsk)
        });
        crate::ui::shell::on_click(&cancel, move || tx.emit(Msg::CancelAsk));
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.append(&ok);
        row.append(&cancel);
        column.append(&row);
        win.set_child(Some(&column));
        win.present();
        self.dialog = Some(win);
    }

    pub(in crate::ui::app) fn begin_checksum(&mut self, kind: u8, sender: &ComponentSender<Self>) {
        self.close_dialog();
        if kind == 0 {
            self.start_sfv(sender);
            return;
        }
        let kind = match kind {
            1 => Kind::Md5,
            2 => Kind::Sha1,
            _ => Kind::Sha256,
        };
        let sources = self.state.selected_paths(self.state.active);
        let dest = self.sum_dest(&sources, kind);
        if dest.exists() {
            self.queued = Some(super::Queued::Sum(dest, kind));
            self.ask = Some(Ask::Overwrite);
            self.ask_confirm("Overwrite checksum?", sender.input_sender().clone());
            return;
        }
        self.spawn_sum(dest, kind, sender);
    }

    pub(in crate::ui::app) fn sum_dest(&self, sources: &[PathBuf], kind: Kind) -> PathBuf {
        let stem = if sources.len() == 1 {
            super::jobs::stem_of(&sources[0]).unwrap_or("checksum")
        } else {
            self.state.tab(self.state.active).path.file_name().and_then(|name| name.to_str()).unwrap_or("checksum")
        };
        self.state.tab(self.state.active).path.join(format!("{stem}.{}", kind.extension()))
    }

    pub(in crate::ui::app) fn ask_shred(&mut self, sender: &ComponentSender<Self>) {
        let paths = self.state.selected_paths(self.state.active);
        if paths.is_empty() {
            self.status = "Nothing selected".into();
            return;
        }
        let what = match paths.as_slice() {
            [one] => one.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "this".into()),
            many => format!("{} items", many.len()),
        };
        self.ask = Some(Ask::Shred(paths));
        self.ask_confirm_detail(
            &format!("Delete {what} now?"),
            "This removes it directly. It does not go to the trash. The data is overwritten once, then the name is removed. On an SSD, and on btrfs and ZFS, the old data often remains in other blocks.",
            sender.input_sender().clone(),
        );
    }

    pub(in crate::ui::app) fn ask_encode(&mut self, sender: &ComponentSender<Self>) {
        let Some(path) = self.cursor_path() else {
            self.status = "Nothing selected".into();
            return;
        };
        if path.is_dir() {
            self.status = "Encode needs a file".into();
            return;
        }
        self.close_dialog();
        let win = gtk::Window::new();
        win.set_title(Some("Encode"));
        win.set_modal(true);
        win.set_transient_for(Some(&self.shell.window));
        win.set_default_width(360);
        let mode = gtk::DropDown::from_strings(&["Encode", "Decode"]);
        if rust_commander::encode::codec_of(&path).is_some() {
            mode.set_selected(1);
        }
        let codec = gtk::DropDown::from_strings(&["Base64", "UUE", "MIME"]);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        column.set_margin_all(12);
        column.append(&mode);
        column.append(&codec);
        let ok = gtk::Button::with_label("OK");
        let tx = sender.input_sender().clone();
        let (mode_ok, codec_ok) = (mode.clone(), codec.clone());
        crate::ui::shell::on_click(&ok, move || {
            tx.emit(Msg::EncodePick { decode: mode_ok.selected() == 1, codec: codec_ok.selected() as u8 });
        });
        let cancel = gtk::Button::with_label("Cancel");
        let tx = sender.input_sender().clone();
        crate::ui::shell::on_escape(&win, {
            let tx = tx.clone();
            move || tx.emit(Msg::CancelAsk)
        });
        crate::ui::shell::on_click(&cancel, move || tx.emit(Msg::CancelAsk));
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.append(&ok);
        row.append(&cancel);
        column.append(&row);
        win.set_child(Some(&column));
        win.present();
        self.dialog = Some(win);
    }

    pub(in crate::ui::app) fn begin_encode(&mut self, decode: bool, codec: u8, sender: &ComponentSender<Self>) {
        self.close_dialog();
        let codec = match codec {
            1 => Codec::Uue,
            2 => Codec::Mime,
            _ => Codec::Base64,
        };
        let Some(source) = self.cursor_path() else { return };
        let other = self.state.tab(self.state.active.other()).path.clone();
        if decode {
            self.spawn_decode(source, other, sender);
        } else {
            let name = source.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
            let dest = other.join(format!("{name}.{}", codec.extension()));
            self.spawn_encode(codec, source, dest, sender);
        }
    }

    pub(in crate::ui::app) fn start_test(&mut self, sender: &ComponentSender<Self>) {
        // A running unpack owns `pending`.
        if self.job.is_some() {
            self.status = "Busy".into();
            return;
        }
        let Some(path) = self.cursor_path().filter(|path| rust_commander::archive::is_testable(path)) else {
            self.status = "Not an archive".into();
            return;
        };
        self.pending = Some(super::PendingUnpack {
            archive: path,
            dest: PathBuf::new(),
            names: None,
            password: None,
            asked: 0,
            pick: false,
            phase: Phase::Test,
        });
        self.spawn_test(sender);
    }

    pub(in crate::ui::app) fn show_search_feed(&mut self, sender: &ComponentSender<Self>) {
        self.feed_search(sender);
    }

    pub(in crate::ui::app) fn open_feed_hit(&mut self, sender: &ComponentSender<Self>) {
        let id = self.state.active;
        let Some(path) = self.cursor_path() else { return };
        let Some(parent) = path.parent().map(|dir| dir.to_path_buf()) else { return };
        let Some(name) = path.file_name().map(|name| name.to_string_lossy().into_owned()) else { return };
        self.state.tab_mut(id).feed = false;
        if name.starts_with('.') && !self.state.show_hidden {
            self.state.show_hidden = true;
        }
        self.state.pane_mut(id).filter.clear();
        self.shell.panes[id.index()].filter.set_text("");
        let token = self.state.navigate(id, parent.clone(), true);
        let tab = self.state.pane(id).active;
        self.search_target = Some((id, tab, token, name));
        self.spawn_load(sender, id, tab, parent, token);
        self.focus_list();
    }
}

fn field_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label
}

fn suggested_link_name(target: &std::path::Path, place: &std::path::Path) -> String {
    let name = target.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "link".into());
    let path = place.join(&name);
    if path == target || path.exists() { format!("{name} link") } else { name }
}

fn link_story(symbolic: bool, name: &str, place: &str, target: &str) -> String {
    if name.is_empty() {
        return "Type a name for the new link.".into();
    }
    if symbolic {
        format!("A shortcut called {name} will appear in {place} and open {target}.")
    } else {
        format!("A second name, {name}, will appear in {place} for the same file.")
    }
}

fn link_problem(symbolic: bool, name: &str, place: &str, target_is_file: bool) -> Option<&'static str> {
    if name.is_empty() {
        return Some("Type a name for the new link.");
    }
    if name.contains('/') || name.contains('\\') {
        return Some("The name is one part, without a folder.");
    }
    if place.is_empty() {
        return Some("Link location is the folder where the link is created.");
    }
    if !symbolic && !target_is_file {
        return Some("A hard link needs a file. Choose Shortcut for a folder.");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{link_problem, link_story, suggested_link_name};

    #[test]
    fn the_name_stays_free_when_the_target_is_already_there() {
        let dir = std::env::temp_dir().join(format!("rc-link-name-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("Videos");
        std::fs::create_dir(&target).unwrap();
        assert_eq!(suggested_link_name(&target, &dir), "Videos link");
        assert_eq!(suggested_link_name(&target, &dir.join("other")), "Videos");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_story_names_the_shortcut_and_where_it_appears() {
        assert_eq!(
            link_story(true, "Filmer", "/home/marcus", "/home/marcus/Videos"),
            "A shortcut called Filmer will appear in /home/marcus and open /home/marcus/Videos."
        );
        assert_eq!(link_problem(false, "Filmer", "/home/marcus", false), Some("A hard link needs a file. Choose Shortcut for a folder."));
        assert_eq!(link_problem(true, "a/b", "/home/marcus", true), Some("The name is one part, without a folder."));
    }

    #[test]
    fn symbolic_and_hard_links_can_be_created() {
        let dir = std::env::temp_dir().join(format!("rc-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("other")).unwrap();
        let target = dir.join("note.txt");
        std::fs::write(&target, b"hello").unwrap();
        let symbolic = dir.join("other/note.txt");
        std::os::unix::fs::symlink(&target, &symbolic).unwrap();
        assert_eq!(std::fs::read_link(&symbolic).unwrap(), target);
        assert_eq!(std::fs::read(&symbolic).unwrap(), b"hello");
        let hard = dir.join("other/again.txt");
        std::fs::hard_link(&target, &hard).unwrap();
        assert_eq!(std::fs::read(&hard).unwrap(), b"hello");
        let folder = dir.join("folder");
        std::fs::create_dir(&folder).unwrap();
        assert!(std::fs::hard_link(&folder, dir.join("other/folder")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

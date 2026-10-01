use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use relm4::gtk;
use relm4::gtk::prelude::*;
use relm4::prelude::ComponentSender;
use relm4::RelmWidgetExt;

use rust_commander::search::{self, Event, Options};

use super::App;
use crate::ui::msg::Msg;

pub(super) struct SearchUi {
    window: gtk::Window,
    source: rust_commander::state::PaneId,
    query: gtk::Entry,
    root: gtk::Entry,
    recursive: gtk::CheckButton,
    hidden: gtk::CheckButton,
    files_only: gtk::CheckButton,
    case_sensitive: gtk::CheckButton,
    list: gtk::ListBox,
    status: gtk::Label,
    start: gtk::Button,
    cancel: gtk::Button,
    go: gtk::Button,
    results: Vec<PathBuf>,
    stop: Option<Arc<AtomicBool>>,
}

impl App {
    pub(in crate::ui::app) fn open_search(&mut self, sender: &ComponentSender<Self>) {
        if let Some(ui) = &self.search {
            ui.window.present();
            return;
        }
        let window = gtk::Window::new();
        window.set_title(Some("Search files"));
        window.set_transient_for(Some(&self.shell.window));
        window.set_modal(true);
        window.set_default_size(740, 520);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
        content.set_margin_all(12);
        let query = gtk::Entry::new();
        query.set_placeholder_text(Some("File or folder name"));
        let root = gtk::Entry::new();
        root.set_text(&self.state.tab(self.state.active).path.display().to_string());
        let root_row = labeled_row("Search in", &root);
        let choose = gtk::Button::with_label("Choose folder");
        let root_choice = root.clone();
        let window_choice = window.downgrade();
        crate::ui::shell::on_click(&choose, move || {
            let Some(window_choice) = window_choice.upgrade() else { return };
            let dialog = gtk::FileDialog::new();
            dialog.set_title("Search in");
            dialog.set_modal(true);
            let root_choice = root_choice.clone();
            dialog.select_folder(Some(&window_choice), None::<&gtk::gio::Cancellable>, move |result| {
                if let Ok(file) = result {
                    if let Some(path) = file.path() {
                        root_choice.set_text(&path.to_string_lossy());
                    }
                }
            });
        });
        root_row.append(&choose);
        content.append(&labeled_row("Search for", &query));
        content.append(&root_row);

        let options = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        let recursive = gtk::CheckButton::with_label("Subfolders");
        recursive.set_active(true);
        let hidden = gtk::CheckButton::with_label("Hidden files");
        hidden.set_active(self.state.show_hidden);
        let files_only = gtk::CheckButton::with_label("Files only");
        let case_sensitive = gtk::CheckButton::with_label("Case sensitive");
        for check in [&recursive, &hidden, &files_only, &case_sensitive] {
            options.append(check);
        }
        content.append(&options);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let start = gtk::Button::with_label("Search");
        let tx = sender.input_sender().clone();
        crate::ui::shell::on_click(&start, move || tx.emit(Msg::SearchStart));
        let tx = sender.input_sender().clone();
        query.connect_activate(move |_| tx.emit(Msg::SearchStart));
        let cancel = gtk::Button::with_label("Cancel search");
        cancel.set_sensitive(false);
        let tx = sender.input_sender().clone();
        crate::ui::shell::on_click(&cancel, move || tx.emit(Msg::SearchCancel));
        actions.append(&start);
        actions.append(&cancel);
        content.append(&actions);

        let status = gtk::Label::new(Some("Ready"));
        status.set_xalign(0.0);
        content.append(&status);
        let headings = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        headings.set_margin_all(4);
        let name_heading = gtk::Label::new(Some("Name"));
        name_heading.set_xalign(0.0);
        name_heading.set_width_chars(26);
        headings.append(&name_heading);
        let location_heading = gtk::Label::new(Some("Location"));
        location_heading.set_xalign(0.0);
        headings.append(&location_heading);
        content.append(&headings);
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        let tx = sender.input_sender().clone();
        list.connect_row_activated(move |_, row| tx.emit(Msg::SearchGo(row.index() as usize)));
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_child(Some(&list));
        scroll.set_vexpand(true);
        scroll.set_min_content_height(220);
        content.append(&scroll);

        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        footer.set_halign(gtk::Align::End);
        let go = gtk::Button::with_label("Go to result");
        go.set_sensitive(false);
        let tx = sender.input_sender().clone();
        let selection = list.clone();
        crate::ui::shell::on_click(&go, move || {
            if let Some(row) = selection.selected_row() {
                tx.emit(Msg::SearchGo(row.index() as usize));
            }
        });
        let close = gtk::Button::with_label("Close");
        let tx = sender.input_sender().clone();
        crate::ui::shell::on_click(&close, move || tx.emit(Msg::CloseSearch));
        footer.append(&go);
        footer.append(&close);
        content.append(&footer);
        window.set_child(Some(&content));
        let tx = sender.input_sender().clone();
        window.connect_close_request(move |_| {
            tx.emit(Msg::CloseSearch);
            gtk::glib::Propagation::Proceed
        });
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let tx = sender.input_sender().clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            let plain = !mods.contains(gtk::gdk::ModifierType::CONTROL_MASK) && !mods.contains(gtk::gdk::ModifierType::ALT_MASK);
            if key == gtk::gdk::Key::Escape {
                tx.emit(Msg::SearchEscape);
                gtk::glib::Propagation::Stop
            } else if plain && key == gtk::gdk::Key::F1 {
                tx.emit(Msg::ShowCommands);
                gtk::glib::Propagation::Stop
            } else {
                gtk::glib::Propagation::Proceed
            }
        });
        window.add_controller(keys);
        window.present();
        query.grab_focus();
        self.search = Some(SearchUi {
            window, source: self.state.active, query, root, recursive, hidden, files_only,
            case_sensitive, list, status, start, cancel, go, results: Vec::new(), stop: None,
        });
    }

    pub(super) fn start_search(&mut self, sender: &ComponentSender<Self>) {
        let Some(ui) = self.search.as_mut() else { return };
        let query = ui.query.text().trim().to_string();
        let root = PathBuf::from(ui.root.text().trim());
        if query.is_empty() {
            ui.status.set_text("Enter a name to search for");
            return;
        }
        if !root.is_absolute() || !root.is_dir() {
            ui.status.set_text("Choose an existing absolute folder");
            return;
        }
        if let Some(stop) = ui.stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        while let Some(row) = ui.list.first_child() {
            ui.list.remove(&row);
        }
        ui.results.clear();
        ui.go.set_sensitive(false);
        ui.status.set_text("Searching...");
        ui.start.set_sensitive(false);
        ui.cancel.set_sensitive(true);
        let options = Options {
            root, query, recursive: ui.recursive.is_active(), hidden: ui.hidden.is_active(),
            files_only: ui.files_only.is_active(), case_sensitive: ui.case_sensitive.is_active(),
        };
        self.search_gen = self.search_gen.wrapping_add(1);
        let generation = self.search_gen;
        let stop = Arc::new(AtomicBool::new(false));
        ui.stop = Some(Arc::clone(&stop));
        let tx = sender.input_sender().clone();
        std::thread::spawn(move || {
            search::run(options, || stop.load(Ordering::Relaxed), |event| {
                tx.emit(Msg::SearchEvent(generation, event));
            });
        });
    }

    pub(super) fn cancel_search(&mut self) {
        let Some(ui) = self.search.as_mut() else { return };
        if let Some(stop) = &ui.stop {
            stop.store(true, Ordering::Relaxed);
            ui.status.set_text("Cancelling...");
            ui.cancel.set_sensitive(false);
        }
    }

    pub(super) fn escape_search(&mut self) {
        match self.search.as_ref().and_then(|ui| ui.stop.as_ref()) {
            Some(stop) if !stop.load(Ordering::Relaxed) => self.cancel_search(),
            _ => self.close_search(),
        }
    }

    pub(super) fn on_search_event(&mut self, generation: u64, event: Event) {
        if generation != self.search_gen {
            return;
        }
        let Some(ui) = self.search.as_mut() else { return };
        match event {
            Event::Matches(paths) => {
                for path in paths {
                    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                    row.set_margin_all(4);
                    let name = gtk::Label::new(path.file_name().map(|name| name.to_string_lossy()).as_deref());
                    name.set_xalign(0.0);
                    name.set_width_chars(26);
                    name.set_max_width_chars(26);
                    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    let location = gtk::Label::new(Some(&path.parent().unwrap_or(&path).display().to_string()));
                    location.set_xalign(0.0);
                    location.set_hexpand(true);
                    location.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                    row.append(&name);
                    row.append(&location);
                    row.set_tooltip_text(Some(&path.display().to_string()));
                    ui.list.append(&row);
                    ui.results.push(path);
                }
                ui.go.set_sensitive(!ui.results.is_empty());
                ui.status.set_text(&format!("Searching... {} results", ui.results.len()));
            }
            Event::Finished { errors, cancelled } => {
                ui.stop = None;
                ui.start.set_sensitive(true);
                ui.cancel.set_sensitive(false);
                let status = if cancelled {
                    "Cancelled"
                } else if ui.results.len() >= search::MAX_RESULTS {
                    "Stopped at the limit"
                } else {
                    "Done"
                };
                let skipped = if errors == 0 { String::new() } else { format!(", {errors} unreadable") };
                ui.status.set_text(&format!("{status}: {} results{skipped}", ui.results.len()));
            }
        }
    }

    pub(super) fn go_to_search_result(&mut self, index: usize, sender: &ComponentSender<Self>) {
        let Some(ui) = &self.search else { return };
        let Some(path) = ui.results.get(index).cloned() else { return };
        let Some(parent) = path.parent().map(PathBuf::from) else { return };
        let Some(name) = path.file_name().map(|name| name.to_string_lossy().into_owned()) else { return };
        let source = ui.source;
        self.close_search();
        if name.starts_with('.') && !self.state.show_hidden {
            self.state.show_hidden = true;
            self.touch_both();
        }
        self.state.pane_mut(source).filter.clear();
        self.shell.panes[source.index()].filter.set_text("");
        let token = self.state.navigate(source, parent.clone(), true);
        let tab = self.state.pane(source).active;
        self.state.active = source;
        self.shell.active.set(source.index());
        self.search_target = Some((source, tab, token, name));
        self.spawn_load(sender, source, tab, parent, token);
        self.focus_list();
    }

    pub(super) fn close_search(&mut self) {
        if let Some(ui) = self.search.take() {
            if let Some(stop) = ui.stop {
                stop.store(true, Ordering::Relaxed);
            }
            self.search_gen = self.search_gen.wrapping_add(1);
            ui.window.close();
        }
    }
}

fn labeled_row(label: &str, field: &gtk::Entry) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let caption = gtk::Label::new(Some(label));
    caption.set_width_chars(12);
    caption.set_xalign(0.0);
    field.set_hexpand(true);
    row.append(&caption);
    row.append(field);
    row
}
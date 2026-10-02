use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
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
    model: gtk::StringList,
    status: gtk::Label,
    start: gtk::Button,
    cancel: gtk::Button,
    go: gtk::Button,
    results: Vec<PathBuf>,
    stop: Option<Arc<AtomicBool>>,
}

impl App {
    pub(in crate::ui::app) fn open_search(&mut self, sender: &ComponentSender<Self>) {
        // The window is only hidden when closed, so the last query, options and results are still there.
        if let Some(ui) = self.search.as_mut() {
            ui.source = self.state.active;
            ui.window.present();
            ui.query.grab_focus();
            ui.query.select_region(0, -1);
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
        query.set_placeholder_text(Some("File or folder name, or a pattern like *.jpg"));
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
        let model = gtk::StringList::new(&[]);
        let selection = gtk::SingleSelection::new(Some(model.clone()));
        selection.set_autoselect(false);
        selection.set_can_unselect(true);
        // Rows are made only for what is on screen, so ten thousand results cost no more than twenty.
        let shown: Shown = Rc::new(RefCell::new(HashMap::new()));
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else { return };
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.set_margin_all(4);
            let name = gtk::Label::new(None);
            name.set_xalign(0.0);
            name.set_width_chars(26);
            name.set_max_width_chars(26);
            name.set_ellipsize(gtk::pango::EllipsizeMode::End);
            let location = gtk::Label::new(None);
            location.set_xalign(0.0);
            location.set_hexpand(true);
            location.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            row.append(&name);
            row.append(&location);
            item.set_child(Some(&row));
        });
        let shown_bind = Rc::clone(&shown);
        factory.connect_bind(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else { return };
            let Some(path) = item.item().and_then(|object| object.downcast::<gtk::StringObject>().ok()) else { return };
            let Some(row) = item.child() else { return };
            let path = path.string();
            let (dir, name) = path.rsplit_once('/').unwrap_or(("", path.as_str()));
            let labels = (row.first_child().and_downcast::<gtk::Label>(), row.last_child().and_downcast::<gtk::Label>());
            if let (Some(name_label), Some(dir_label)) = labels {
                name_label.set_text(name);
                dir_label.set_text(if dir.is_empty() { "/" } else { dir });
            }
            row.set_tooltip_text(Some(path.as_str()));
            shown_bind.borrow_mut().insert(item.position(), row.downgrade());
        });
        let shown_unbind = Rc::clone(&shown);
        factory.connect_unbind(move |_, item| {
            if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
                shown_unbind.borrow_mut().remove(&item.position());
            }
        });
        let list = gtk::ListView::new(Some(selection.clone()), Some(factory));
        list.add_css_class("search-results");
        list.set_single_click_activate(true);
        let tx = sender.input_sender().clone();
        let picking = Rc::new(Cell::new(false));
        let (picking_row, selection_row, shown_row) = (Rc::clone(&picking), selection.clone(), Rc::clone(&shown));
        list.connect_activate(move |_, position| pick_result(&selection_row, &shown_row, position, &tx, &picking_row));
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
        go.connect_clicked(move |go| {
            crate::ui::shell::blink_button(go);
            let position = selection.selected();
            if position != gtk::INVALID_LIST_POSITION {
                pick_result(&selection, &shown, position, &tx, &picking);
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
            gtk::glib::Propagation::Stop
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
            case_sensitive, model, status, start, cancel, go, results: Vec::new(), stop: None,
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
        ui.model.splice(0, ui.model.n_items(), &[]);
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
                let texts: Vec<String> = paths.iter().map(|path| path.display().to_string()).collect();
                let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
                ui.model.splice(ui.model.n_items(), 0, &texts);
                ui.results.extend(paths);
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

    // (window shown, kept results)
    pub(super) fn search_state(&self) -> Option<(bool, usize)> {
        self.search.as_ref().map(|ui| (ui.window.is_visible(), ui.results.len()))
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

    // Hides the window and keeps query, options and results for the next time. A running search is stopped.
    pub(super) fn close_search(&mut self) {
        self.cancel_search();
        if let Some(ui) = &self.search {
            ui.window.set_visible(false);
        }
    }
}

// Row widgets that exist right now, by list position. Only the visible ones do.
type Shown = Rc<RefCell<HashMap<u32, gtk::glib::WeakRef<gtk::Widget>>>>;

// The row blinks first, so the pick is seen before the window goes away.
fn pick_result(selection: &gtk::SingleSelection, shown: &Shown, position: u32, tx: &relm4::Sender<Msg>, picking: &Rc<Cell<bool>>) {
    if picking.replace(true) {
        return;
    }
    selection.set_selected(position);
    let (tx, picking) = (tx.clone(), Rc::clone(picking));
    let go = move || {
        picking.set(false);
        tx.emit(Msg::SearchGo(position as usize));
    };
    let row = shown.borrow().get(&position).and_then(|child| child.upgrade()).and_then(|child| child.parent());
    match row {
        Some(row) => crate::ui::shell::blink_then(&row, go),
        None => go(),
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
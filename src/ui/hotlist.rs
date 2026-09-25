use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

use relm4::gtk;
use relm4::gtk::prelude::*;
use relm4::RelmWidgetExt;

use comarchy::config::Bookmark;
use comarchy::hotlist::{self, Row};

use super::msg::Msg;

pub fn popup(
    parent: &gtk::Window,
    marks: &[Bookmark],
    menu: Option<usize>,
    current: &Path,
    sender: &relm4::Sender<Msg>,
) -> gtk::Window {
    let win = gtk::Window::new();
    win.set_decorated(false);
    win.set_modal(true);
    win.set_transient_for(Some(parent));
    win.set_default_width(320);
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let rows = hotlist::rows(marks, menu);
    let mut accels: Vec<(char, bool, usize)> = Vec::new();
    let mut first: Option<gtk::Button> = None;
    for row in &rows {
        match row {
            Row::Sep => column.append(&gtk::Separator::new(gtk::Orientation::Horizontal)),
            Row::Dir { index, accel } => {
                let button = item_button(&hotlist::accel_markup(&marks[*index].name));
                let path = marks[*index].path.display().to_string();
                if !path.is_empty() {
                    button.set_tooltip_text(Some(&path));
                }
                let tx = sender.clone();
                let index = *index;
                button.connect_clicked(move |_| tx.emit(Msg::HotlistPick(index)));
                if let Some(ch) = accel {
                    accels.push((*ch, false, index));
                }
                if first.is_none() {
                    first = Some(button.clone());
                }
                column.append(&button);
            }
            Row::Menu { index, accel } => {
                let markup = format!("{}  ›", hotlist::accel_markup(&marks[*index].name));
                let button = item_button(&markup);
                let tx = sender.clone();
                let index = *index;
                button.connect_clicked(move |_| tx.emit(Msg::HotlistMenu(index)));
                if let Some(ch) = accel {
                    accels.push((*ch, true, index));
                }
                if first.is_none() {
                    first = Some(button.clone());
                }
                column.append(&button);
            }
        }
    }
    column.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    if hotlist::dir_index(marks, current).is_none() {
        let button = item_button("Add current dir    +");
        let tx = sender.clone();
        button.connect_clicked(move |_| tx.emit(Msg::HotlistAdd));
        column.append(&button);
    } else {
        let button = item_button("Remove current dir    -");
        let tx = sender.clone();
        button.connect_clicked(move |_| tx.emit(Msg::HotlistRemove));
        column.append(&button);
    }
    let configure = item_button("Configure    *");
    let tx = sender.clone();
    configure.connect_clicked(move |_| tx.emit(Msg::HotlistConfigure));
    column.append(&configure);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_propagate_natural_height(true);
    scroll.set_max_content_height(480);
    scroll.set_child(Some(&column));
    win.set_child(Some(&scroll));
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let tx = sender.clone();
    keys.connect_key_pressed(move |_, key, _, mods| {
        let ctrl = mods.contains(gtk::gdk::ModifierType::CONTROL_MASK);
        if ctrl && matches!(key, gtk::gdk::Key::d | gtk::gdk::Key::D) {
            tx.emit(Msg::Hotlist);
            return gtk::glib::Propagation::Stop;
        }
        if let Some(ch) = key.to_unicode() {
            let ch = ch.to_ascii_lowercase();
            if let Some((_, menu, index)) = accels.iter().find(|(accel, _, _)| *accel == ch) {
                if *menu {
                    tx.emit(Msg::HotlistMenu(*index));
                } else {
                    tx.emit(Msg::HotlistPick(*index));
                }
                return gtk::glib::Propagation::Stop;
            }
            match ch {
                '+' => tx.emit(Msg::HotlistAdd),
                '-' => tx.emit(Msg::HotlistRemove),
                '*' => tx.emit(Msg::HotlistConfigure),
                _ => {}
            }
        }
        if matches!(key, gtk::gdk::Key::Escape) {
            tx.emit(Msg::HotlistBack);
            return gtk::glib::Propagation::Stop;
        }
        if matches!(
            key,
            gtk::gdk::Key::Up
                | gtk::gdk::Key::Down
                | gtk::gdk::Key::KP_Up
                | gtk::gdk::Key::KP_Down
                | gtk::gdk::Key::Return
                | gtk::gdk::Key::KP_Enter
                | gtk::gdk::Key::Tab
                | gtk::gdk::Key::ISO_Left_Tab
        ) {
            return gtk::glib::Propagation::Proceed;
        }
        gtk::glib::Propagation::Stop
    });
    win.add_controller(keys);
    if let Some(button) = first {
        button.grab_focus();
    }
    win
}

pub fn add_dialog(parent: &gtk::Window, title: &str, path: &str, sender: &relm4::Sender<Msg>) -> gtk::Window {
    let win = gtk::Window::new();
    win.set_title(Some("Add current dir"));
    win.set_modal(true);
    win.set_transient_for(Some(parent));
    win.set_default_width(420);
    let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
    column.set_margin_all(12);
    let entry = gtk::Entry::new();
    entry.set_text(title);
    entry.set_position(-1);
    let path_entry = gtk::Entry::new();
    path_entry.set_text(path);
    path_entry.set_placeholder_text(Some("/home/marcus/Dropbox"));
    let hint = shortcut_hint();
    let save_target = gtk::CheckButton::with_label("Also save target dir");
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let ok = gtk::Button::with_label("OK");
    let cancel = gtk::Button::with_label("Cancel");
    let tx_ok = sender.clone();
    let entry_ok = entry.clone();
    let path_ok = path_entry.clone();
    let target_ok = save_target.clone();
    let emit = move || {
        tx_ok.emit(Msg::HotlistAdded(
            entry_ok.text().to_string(),
            path_ok.text().to_string(),
            target_ok.is_active(),
        ));
    };
    let emit_ok = emit.clone();
    ok.connect_clicked(move |_| emit_ok());
    entry.connect_activate(move |_| emit());
    let tx_cancel = sender.clone();
    cancel.connect_clicked(move |_| tx_cancel.emit(Msg::CancelAsk));
    row.append(&ok);
    row.append(&cancel);
    column.append(&entry);
    column.append(&path_entry);
    column.append(&hint);
    column.append(&save_target);
    column.append(&row);
    win.set_child(Some(&column));
    entry.grab_focus();
    win
}

pub fn configure(parent: &gtk::Window, marks: Vec<Bookmark>, sender: &relm4::Sender<Msg>) -> gtk::Window {
    let win = gtk::Window::new();
    win.set_title(Some("Directory hotlist"));
    win.set_modal(true);
    win.set_transient_for(Some(parent));
    win.set_default_width(520);
    win.set_default_height(460);
    let draft = Rc::new(RefCell::new(marks));
    let current = Rc::new(Cell::new(None::<usize>));
    let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
    column.set_margin_all(12);
    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_child(Some(&list));
    let title = gtk::Entry::new();
    title.set_placeholder_text(Some("Dropbox &d"));
    let path = gtk::Entry::new();
    path.set_placeholder_text(Some("/home/marcus/Dropbox"));
    let target = gtk::Entry::new();
    target.set_placeholder_text(Some("Target path"));
    let tools = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let buttons = [
        ("Add", Edit::Add),
        ("Submenu", Edit::Sub),
        ("Separator", Edit::Sep),
        ("Delete", Edit::Delete),
        ("Up", Edit::Up),
        ("Down", Edit::Down),
    ];
    let state = EditState {
        draft: draft.clone(),
        current: current.clone(),
        list: list.clone(),
        title: title.clone(),
        path: path.clone(),
        target: target.clone(),
        sender: sender.clone(),
    };
    for (label, edit) in buttons {
        let button = gtk::Button::with_label(label);
        let state = state.clone();
        button.connect_clicked(move |_| apply(&state, edit));
        tools.append(&button);
    }
    let save_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let ok = gtk::Button::with_label("OK");
    let cancel = gtk::Button::with_label("Cancel");
    let state_ok = state.clone();
    ok.connect_clicked(move |_| {
        write_back(&state_ok);
        state_ok.sender.emit(Msg::HotlistStored(state_ok.draft.borrow().clone()));
    });
    let tx_cancel = sender.clone();
    cancel.connect_clicked(move |_| tx_cancel.emit(Msg::CancelAsk));
    save_row.append(&ok);
    save_row.append(&cancel);
    column.append(&scroll);
    column.append(&shortcut_hint());
    column.append(&title);
    column.append(&path);
    column.append(&target);
    column.append(&tools);
    column.append(&save_row);
    win.set_child(Some(&column));
    if !draft.borrow().is_empty() {
        current.set(Some(0));
    }
    rebuild(&state);
    win
}

#[derive(Clone, Copy)]
enum Edit {
    Add,
    Sub,
    Sep,
    Delete,
    Up,
    Down,
}

#[derive(Clone)]
struct EditState {
    draft: Rc<RefCell<Vec<Bookmark>>>,
    current: Rc<Cell<Option<usize>>>,
    list: gtk::Box,
    title: gtk::Entry,
    path: gtk::Entry,
    target: gtk::Entry,
    sender: relm4::Sender<Msg>,
}

fn apply(state: &EditState, edit: Edit) {
    write_back(state);
    let mut marks = state.draft.borrow_mut();
    let current = state.current.get();
    match edit {
        Edit::Add => {
            marks.push(dir_mark("New"));
            state.current.set(Some(marks.len() - 1));
        }
        Edit::Sub => {
            marks.push(mark("Menu", "menu"));
            marks.push(mark("--", "end"));
            state.current.set(Some(marks.len() - 2));
        }
        Edit::Sep => {
            marks.push(mark("-", "sep"));
            state.current.set(Some(marks.len() - 1));
        }
        Edit::Delete => {
            if let Some(index) = current {
                hotlist::delete_at(&mut marks, index);
                state.current.set(None);
            }
        }
        Edit::Up => {
            if let Some(index) = current.filter(|i| *i > 0) {
                marks.swap(index, index - 1);
                state.current.set(Some(index - 1));
            }
        }
        Edit::Down => {
            if let Some(index) = current.filter(|i| *i + 1 < marks.len()) {
                marks.swap(index, index + 1);
                state.current.set(Some(index + 1));
            }
        }
    }
    drop(marks);
    rebuild(state);
}

fn rebuild(state: &EditState) {
    while let Some(child) = state.list.first_child() {
        state.list.remove(&child);
    }
    let marks = state.draft.borrow();
    let selected = state.current.get();
    for (index, mark) in marks.iter().enumerate() {
        let button = item_button(&glib_escape(&hotlist::edit_label(mark)));
        if Some(index) == selected {
            button.add_css_class("hot-on");
        }
        let row_state = state.clone();
        button.connect_clicked(move |_| {
            write_back(&row_state);
            row_state.current.set(Some(index));
            rebuild(&row_state);
        });
        state.list.append(&button);
    }
    drop(marks);
    load_fields(state);
}

fn load_fields(state: &EditState) {
    let marks = state.draft.borrow();
    let Some(mark) = state.current.get().and_then(|i| marks.get(i)) else {
        state.title.set_text("");
        state.path.set_text("");
        state.target.set_text("");
        return;
    };
    state.title.set_text(&mark.name);
    state.path.set_text(&mark.path.display().to_string());
    state.target.set_text(&mark.target.display().to_string());
    let dir = hotlist::is_dir(mark);
    state.path.set_sensitive(dir);
    state.target.set_sensitive(dir);
}

fn write_back(state: &EditState) {
    let Some(index) = state.current.get() else { return };
    let mut marks = state.draft.borrow_mut();
    let Some(mark) = marks.get_mut(index) else { return };
    mark.name = state.title.text().to_string();
    if hotlist::is_dir(mark) {
        mark.path = state.path.text().to_string().into();
        mark.target = state.target.text().to_string().into();
    }
}

fn dir_mark(name: &str) -> Bookmark {
    mark(name, "")
}

fn mark(name: &str, kind: &str) -> Bookmark {
    Bookmark {
        name: name.into(),
        path: std::path::PathBuf::new(),
        target: std::path::PathBuf::new(),
        kind: kind.into(),
    }
}

fn shortcut_hint() -> gtk::Label {
    let hint = gtk::Label::new(Some("Example: Dropbox &d opens the path. Ctrl+D, then D."));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint
}

fn item_button(markup: &str) -> gtk::Button {
    let label = gtk::Label::new(None);
    label.set_markup(markup);
    label.set_xalign(0.0);
    label.set_hexpand(true);
    let button = gtk::Button::new();
    button.set_child(Some(&label));
    button.add_css_class("hot");
    button.set_hexpand(true);
    button
}

fn glib_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

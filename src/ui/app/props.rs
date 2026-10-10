use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use relm4::gtk;
use relm4::gtk::prelude::*;
use relm4::prelude::*;

use rust_commander::props::{self, Facts};

use super::App;
use crate::ui::msg::Msg;

impl App {
    pub(in crate::ui::app) fn show_properties(&mut self, sender: &ComponentSender<Self>) {
        let Some(path) = self.cursor_path() else {
            self.status = "Nothing selected".into();
            return;
        };
        let facts = match props::read(&path) {
            Ok(facts) => facts,
            Err(err) => {
                self.status = err.to_string();
                return;
            }
        };
        self.close_dialog();
        let tx = sender.input_sender().clone();
        let win = gtk::Window::new();
        win.set_title(Some("Properties"));
        win.set_modal(true);
        win.set_transient_for(Some(&self.shell.window));
        win.set_default_width(520);
        win.set_default_height(560);
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let heading = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let kind = gtk::Label::new(Some(facts.kind));
        kind.set_xalign(0.0);
        kind.add_css_class("dim-label");
        let title = gtk::Label::new(Some(&name));
        title.set_xalign(0.0);
        title.add_css_class("dialog-title");
        heading.append(&kind);
        heading.append(&title);
        let (mime, _) = gtk::gio::content_type_guess(Some(&path), None::<&[u8]>);
        let notebook = gtk::Notebook::new();
        notebook.set_vexpand(true);
        let (facts_grid, modified, size_cancel) = facts_page(&path, &facts, mime.as_str());
        notebook.append_page(&scrolled(&facts_grid), Some(&gtk::Label::new(Some("Properties"))));
        let (attr_page, octal, owner, group, uids, gids) = attr_page(&facts);
        notebook.append_page(&scrolled(&attr_page), Some(&gtk::Label::new(Some("Attributes"))));
        notebook.append_page(&scrolled(&details_page(&path, &name, mime.as_str())), Some(&gtk::Label::new(Some("Details"))));
        notebook.connect_switch_page(|notebook, page, _| {
            if let Some(tab) = notebook.tab_label(page) {
                crate::ui::shell::blink_tab(&tab);
            }
        });
        let error = gtk::Label::new(None);
        error.set_xalign(0.0);
        error.set_wrap(true);
        error.add_css_class("error");
        error.set_visible(false);
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.set_halign(gtk::Align::End);
        let ok = gtk::Button::with_label("OK");
        ok.add_css_class("fkey");
        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("fkey");
        buttons.append(&ok);
        buttons.append(&cancel);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        column.set_margin_all(12);
        column.append(&heading);
        column.append(&notebook);
        column.append(&error);
        column.append(&buttons);
        win.set_child(Some(&column));
        win.set_default_widget(Some(&ok));
        let win_cancel = win.clone();
        let win_esc = win.clone();
        crate::ui::shell::on_escape(&win, move || win_esc.close());
        crate::ui::shell::on_click(&cancel, move || win_cancel.close());
        let error_ok = error.clone();
        let tx_ok = tx.clone();
        let win_ok = win.clone();
        crate::ui::shell::on_click(&ok, move || {
            if facts.symlink {
                win_ok.close();
                return;
            }
            let Some(mode) = props::parse_mode(&octal.text()) else {
                error_ok.set_text("Octal needs 3 or 4 digits from 0 to 7");
                error_ok.set_visible(true);
                return;
            };
            let uid = uids.get(owner.selected() as usize).copied().unwrap_or(facts.uid);
            let gid = gids.get(group.selected() as usize).copied().unwrap_or(facts.gid);
            let stamp_text = modified.text().to_string();
            let stamp_changed = stamp_text != props::format_stamp(facts.modified);
            let stamp = if stamp_changed {
                match props::parse_stamp(&stamp_text) {
                    Some(secs) => Some(secs),
                    None => {
                        error_ok.set_text("Modified time uses the form shown, for example Mon, 10 October 2026 08:41:00 UT+0200");
                        error_ok.set_visible(true);
                        return;
                    }
                }
            } else {
                None
            };
            if mode == facts.mode && uid == facts.uid && gid == facts.gid && stamp.is_none() {
                win_ok.close();
                return;
            }
            if mode != facts.mode || uid != facts.uid || gid != facts.gid {
                if let Err(err) = props::apply(&path, &facts, mode, uid, gid) {
                    error_ok.set_text(&err.to_string());
                    error_ok.set_visible(true);
                    tx_ok.emit(Msg::RefreshActive);
                    return;
                }
            }
            if let Some(secs) = stamp {
                if let Err(err) = props::set_modified(&path, secs) {
                    error_ok.set_text(&err.to_string());
                    error_ok.set_visible(true);
                    tx_ok.emit(Msg::RefreshActive);
                    return;
                }
            }
            tx_ok.emit(Msg::PropsSaved);
        });
        if let Some(cancel) = size_cancel {
            win.connect_close_request(move |_| {
                cancel.store(true, Ordering::Relaxed);
                gtk::glib::Propagation::Proceed
            });
        }
        win.present();
        self.dialog = Some(win);
    }
}

// A tiling window manager may give the dialog less height than the pages need; the buttons must stay visible.
fn scrolled(page: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroll.set_propagate_natural_height(true);
    scroll.set_vexpand(true);
    page.set_margin_start(14);
    page.set_margin_end(14);
    page.set_margin_bottom(12);
    scroll.set_child(Some(page));
    scroll
}

fn facts_page(path: &std::path::Path, facts: &Facts, mime: &str) -> (gtk::Grid, gtk::Entry, Option<Arc<AtomicBool>>) {
    let grid = gtk::Grid::new();
    grid.set_column_spacing(18);
    grid.set_row_spacing(6);
    grid.set_margin_top(12);
    let mut row = 0;
    let add = |grid: &gtk::Grid, row: i32, name: &str, value: &str| {
        let name_label = gtk::Label::new(Some(name));
        name_label.set_xalign(0.0);
        name_label.add_css_class("dim-label");
        let value_label = gtk::Label::new(Some(value));
        value_label.set_xalign(0.0);
        value_label.set_wrap(true);
        value_label.set_hexpand(true);
        read_only(&value_label);
        grid.attach(&name_label, 0, row, 1, 1);
        grid.attach(&value_label, 1, row, 1, 1);
    };
    add(&grid, row, "Path", &folder_of(path));
    row += 1;
    add(&grid, row, "Type", facts.kind);
    row += 1;
    if let Some(target) = &facts.target {
        add(&grid, row, "Target", &target.display().to_string());
        row += 1;
    }
    add(&grid, row, "Media type", mime);
    row += 1;
    add(&grid, row, "Size", &props::size_line(facts.len));
    row += 1;
    add(&grid, row, "Size on disk", &props::size_line(facts.disk));
    row += 1;
    let size_cancel = (facts.kind == "Directory").then(|| folder_size_row(&grid, &mut row, path));
    add(&grid, row, "Created", &props::format_stamp(facts.created.unwrap_or(0)));
    row += 1;
    let modified = gtk::Entry::new();
    modified.set_text(&props::format_stamp(facts.modified));
    modified.set_hexpand(true);
    let modified_name = gtk::Label::new(Some("Modified"));
    modified_name.set_xalign(0.0);
    modified_name.add_css_class("dim-label");
    grid.attach(&modified_name, 0, row, 1, 1);
    grid.attach(&modified, 1, row, 1, 1);
    row += 1;
    add(&grid, row, "Accessed", &props::format_stamp(facts.accessed));
    row += 1;
    add(&grid, row, "Status changed", &props::format_stamp(facts.changed));
    (grid, modified, size_cancel)
}

fn folder_size_row(grid: &gtk::Grid, row: &mut i32, path: &std::path::Path) -> Arc<AtomicBool> {
    let cancel = Arc::new(AtomicBool::new(false));
    let name = gtk::Label::new(Some("Folder size"));
    name.set_xalign(0.0);
    name.add_css_class("dim-label");
    let button = gtk::Button::with_label("Calculate folder size");
    let spinner = gtk::Spinner::new();
    spinner.set_visible(false);
    let result = gtk::Label::new(None);
    result.set_xalign(0.0);
    result.set_hexpand(true);
    let path = path.to_path_buf();
    let (button_run, spinner_run, result_run, cancel_run) = (button.clone(), spinner.clone(), result.clone(), Arc::clone(&cancel));
    crate::ui::shell::on_click(&button, move || {
        button_run.set_sensitive(false);
        result_run.set_text("");
        spinner_run.set_visible(true);
        spinner_run.start();
        cancel_run.store(false, Ordering::Relaxed);
        let (tx, rx) = std::sync::mpsc::channel();
        let (path, cancel) = (path.clone(), Arc::clone(&cancel_run));
        std::thread::spawn(move || {
            let _ = tx.send(rust_commander::sizes::folder_bytes(&path, &cancel));
        });
        let (button_ui, spinner_ui, result_ui, cancel_ui) = (button_run.clone(), spinner_run.clone(), result_run.clone(), Arc::clone(&cancel_run));
        gtk::glib::timeout_add_local(std::time::Duration::from_millis(100), move || match rx.try_recv() {
            Ok(counted) => {
                spinner_ui.stop();
                spinner_ui.set_visible(false);
                button_ui.set_sensitive(true);
                if !cancel_ui.load(Ordering::Relaxed) {
                    match counted {
                        Ok(bytes) => result_ui.set_text(&rust_commander::sizes::folder_size_text(bytes)),
                        Err(err) => result_ui.set_text(&err.to_string()),
                    }
                }
                gtk::glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => gtk::glib::ControlFlow::Break,
        });
    });
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    line.append(&button);
    line.append(&spinner);
    line.append(&result);
    grid.attach(&name, 0, *row, 1, 1);
    grid.attach(&line, 1, *row, 1, 1);
    *row += 1;
    cancel
}

fn details_page(path: &std::path::Path, name: &str, mime: &str) -> gtk::Grid {
    let grid = gtk::Grid::new();
    grid.set_column_spacing(18);
    grid.set_row_spacing(6);
    grid.set_margin_top(12);
    let mut rows = vec![("Media type".to_string(), mime.to_string())];
    if rust_commander::imageview::is_image(name) {
        if let Some((width, height)) = props::dimensions(path) {
            rows.push(("Width".into(), width.to_string()));
            rows.push(("Height".into(), height.to_string()));
        }
    }
    for (row, (name, value)) in rows.iter().enumerate() {
        let name_label = gtk::Label::new(Some(name));
        name_label.set_xalign(0.0);
        name_label.add_css_class("dim-label");
        let value_label = gtk::Label::new(Some(value));
        value_label.set_xalign(0.0);
        value_label.set_hexpand(true);
        read_only(&value_label);
        grid.attach(&name_label, 0, row as i32, 1, 1);
        grid.attach(&value_label, 1, row as i32, 1, 1);
    }
    grid
}

fn attr_page(facts: &Facts) -> (gtk::Box, gtk::Entry, gtk::DropDown, gtk::DropDown, Vec<u32>, Vec<u32>) {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    page.set_margin_top(12);
    let grid = gtk::Grid::new();
    grid.set_column_spacing(16);
    grid.set_row_spacing(4);
    for (col, title) in [(1, "Read"), (2, "Write"), (3, "Execute")] {
        let label = gtk::Label::new(Some(title));
        grid.attach(&label, col, 0, 1, 1);
    }
    for (row, title) in [(1, "Owner"), (2, "Group"), (3, "Other")] {
        let label = gtk::Label::new(Some(title));
        label.set_xalign(0.0);
        grid.attach(&label, 0, row, 1, 1);
    }
    let boxes: Vec<gtk::CheckButton> = (0..12).map(|_| gtk::CheckButton::new()).collect();
    for i in 0..9 {
        grid.attach(&boxes[i], (i % 3) as i32 + 1, (i / 3) as i32 + 1, 1, 1);
    }
    let bits = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    for (i, title) in [(9, "SUID"), (10, "SGID"), (11, "Sticky")] {
        boxes[i].set_label(Some(title));
        bits.append(&boxes[i]);
    }
    let exec = gtk::CheckButton::with_label("Allow executing file as program");
    let octal_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    octal_row.append(&gtk::Label::new(Some("Octal")));
    let octal = gtk::Entry::new();
    octal.set_max_width_chars(6);
    octal.set_width_chars(6);
    octal_row.append(&octal);
    let boxes = Rc::new(boxes);
    let guard = Rc::new(Cell::new(true));
    for (i, bit) in props::BITS.iter().enumerate() {
        boxes[i].set_active(facts.mode & bit != 0);
    }
    exec.set_active(facts.mode & 0o100 != 0);
    octal.set_text(&props::mode_text(facts.mode));
    guard.set(false);
    for i in 0..12 {
        let button = boxes[i].clone();
        let boxes = Rc::clone(&boxes);
        let octal = octal.clone();
        let exec = exec.clone();
        let guard = Rc::clone(&guard);
        button.connect_toggled(move |_| {
            if guard.get() {
                return;
            }
            guard.set(true);
            if i == 2 {
                exec.set_active(boxes[2].is_active());
            }
            octal.set_text(&props::mode_text(mode_of(&boxes)));
            guard.set(false);
        });
    }
    let boxes_exec = Rc::clone(&boxes);
    let octal_exec = octal.clone();
    let guard_exec = Rc::clone(&guard);
    exec.connect_toggled(move |exec| {
        if guard_exec.get() {
            return;
        }
        guard_exec.set(true);
        boxes_exec[2].set_active(exec.is_active());
        octal_exec.set_text(&props::mode_text(mode_of(&boxes_exec)));
        guard_exec.set(false);
    });
    let boxes_octal = Rc::clone(&boxes);
    let exec_octal = exec.clone();
    let guard_octal = Rc::clone(&guard);
    octal.connect_changed(move |entry| {
        if guard_octal.get() {
            return;
        }
        let Some(mode) = props::parse_mode(&entry.text()) else { return };
        guard_octal.set(true);
        for (i, bit) in props::BITS.iter().enumerate() {
            boxes_octal[i].set_active(mode & bit != 0);
        }
        exec_octal.set_active(mode & 0o100 != 0);
        guard_octal.set(false);
    });
    let mut user_rows = users();
    let mut group_rows = groups();
    ensure(&mut user_rows, facts.uid);
    ensure(&mut group_rows, facts.gid);
    let (owner, uids) = account_drop(&user_rows, facts.uid);
    let (group, gids) = account_drop(&group_rows, facts.gid);
    let owner_row = labeled("Owner", &owner);
    let group_row = labeled("Group", &group);
    if facts.symlink {
        let note = gtk::Label::new(Some("A link has no permissions of its own."));
        note.set_xalign(0.0);
        page.append(&note);
        grid.set_sensitive(false);
        bits.set_sensitive(false);
        exec.set_sensitive(false);
        octal_row.set_sensitive(false);
        owner_row.set_sensitive(false);
        group_row.set_sensitive(false);
    }
    page.append(&grid);
    page.append(&bits);
    page.append(&exec);
    page.append(&octal_row);
    page.append(&owner_row);
    page.append(&group_row);
    (page, octal, owner, group, uids, gids)
}

fn mode_of(boxes: &[gtk::CheckButton]) -> u32 {
    let mut mode = 0;
    for (i, bit) in props::BITS.iter().enumerate() {
        if boxes.get(i).is_some_and(|box_| box_.is_active()) {
            mode |= bit;
        }
    }
    mode
}

fn labeled(title: &str, drop: &gtk::DropDown) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let label = gtk::Label::new(Some(title));
    label.set_xalign(0.0);
    label.set_width_chars(8);
    drop.set_hexpand(true);
    row.append(&label);
    row.append(drop);
    row
}

fn account_drop(rows: &[(u32, String)], current: u32) -> (gtk::DropDown, Vec<u32>) {
    let labels: Vec<&str> = rows.iter().map(|(_, name)| name.as_str()).collect();
    let drop = gtk::DropDown::from_strings(&labels);
    let index = rows.iter().position(|(id, _)| *id == current).unwrap_or(0);
    drop.set_selected(index as u32);
    let ids = rows.iter().map(|(id, _)| *id).collect();
    (drop, ids)
}

fn ensure(rows: &mut Vec<(u32, String)>, id: u32) {
    if !rows.iter().any(|(have, _)| *have == id) {
        rows.push((id, id.to_string()));
    }
}

fn users() -> Vec<(u32, String)> {
    read_accounts(
        || unsafe { libc::setpwent() },
        || unsafe { libc::getpwent() },
        || unsafe { libc::endpwent() },
        |pwd| unsafe { ((*pwd).pw_uid, std::ffi::CStr::from_ptr((*pwd).pw_name)) },
    )
}

fn groups() -> Vec<(u32, String)> {
    read_accounts(
        || unsafe { libc::setgrent() },
        || unsafe { libc::getgrent() },
        || unsafe { libc::endgrent() },
        |grp| unsafe { ((*grp).gr_gid, std::ffi::CStr::from_ptr((*grp).gr_name)) },
    )
}

fn read_accounts<T>(
    start: impl Fn(),
    next: impl Fn() -> *mut T,
    end: impl Fn(),
    read: impl Fn(*mut T) -> (u32, &'static std::ffi::CStr),
) -> Vec<(u32, String)> {
    let mut rows = Vec::new();
    start();
    loop {
        let item = next();
        if item.is_null() {
            break;
        }
        let (id, name) = read(item);
        let name = name.to_string_lossy().into_owned();
        if !name.is_empty() {
            rows.push((id, name));
        }
    }
    end();
    rows.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    rows
}

// Shown as text. A selectable label joins the tab order and looks like a field.
fn read_only(label: &gtk::Label) {
    label.set_selectable(true);
    label.set_can_focus(false);
    label.set_focus_on_click(false);
}

fn folder_of(path: &std::path::Path) -> String {
    let parent = path.parent().unwrap_or(path);
    let mut text = parent.display().to_string();
    if !text.ends_with('/') {
        text.push('/');
    }
    text
}

use std::path::PathBuf;

use relm4::gtk;
use relm4::gtk::prelude::*;
use relm4::prelude::*;

use rust_commander::places::{self, DiskInfo};

use super::App;

impl App {
    // `None` is the disk under the active panel, which is what the hotkey asks for.
    pub(in crate::ui::app) fn show_disk_info(&mut self, path: Option<PathBuf>) {
        let dir = path.unwrap_or_else(|| self.state.tab(self.state.active).path.clone());
        let info = match places::disk_info(&dir) {
            Ok(info) => info,
            Err(err) => {
                self.status = format!("Disk info: {err}");
                return;
            }
        };
        self.close_dialog();
        let win = gtk::Window::new();
        win.set_title(Some("Disk info"));
        win.set_modal(true);
        win.set_transient_for(Some(&self.shell.window));
        win.set_default_width(480);
        let heading = gtk::Label::new(Some(&heading_of(&info)));
        heading.set_xalign(0.0);
        heading.add_css_class("cmd-head");
        let bar = gtk::ProgressBar::new();
        bar.set_fraction(f64::from(info.used_percent()) / 100.0);
        let summary = gtk::Label::new(Some(&places::summary(&info)));
        summary.set_xalign(0.0);
        let grid = rows(&info);
        let ok = gtk::Button::with_label("OK");
        ok.add_css_class("fkey");
        ok.set_halign(gtk::Align::End);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        column.set_margin_all(12);
        column.append(&heading);
        column.append(&bar);
        column.append(&summary);
        column.append(&grid);
        column.append(&ok);
        win.set_child(Some(&column));
        win.set_default_widget(Some(&ok));
        let win_ok = win.clone();
        let win_esc = win.clone();
        crate::ui::shell::on_escape(&win, move || win_esc.close());
        crate::ui::shell::on_click(&ok, move || win_ok.close());
        win.present();
        ok.grab_focus();
        self.dialog = Some(win);
    }
}

fn heading_of(info: &DiskInfo) -> String {
    match &info.label {
        Some(label) => format!("{label}  ({})", info.mount.display()),
        None => info.mount.display().to_string(),
    }
}

fn rows(info: &DiskInfo) -> gtk::Grid {
    let size = |n: u64| rust_commander::props::size_line(n);
    let mut rows = vec![
        ("Mount point", info.mount.display().to_string()),
        ("Device", info.device.clone()),
        ("File system", info.fstype.clone()),
    ];
    if let Some(label) = &info.label {
        rows.push(("Label", label.clone()));
    }
    if let Some(uuid) = &info.uuid {
        rows.push(("UUID", uuid.clone()));
    }
    rows.push(("Mount options", info.options.replace(',', ", ")));
    rows.push(("Size", size(info.total)));
    rows.push(("Used", format!("{}  ({}%)", size(info.used()), info.used_percent())));
    rows.push(("Free", size(info.free)));
    if info.avail != info.free {
        rows.push(("Available to you", size(info.avail)));
    }
    if info.files > 0 {
        rows.push(("Inodes", format!("{} used of {}", info.files - info.files_free.min(info.files), info.files)));
    }
    let grid = gtk::Grid::new();
    grid.set_column_spacing(18);
    grid.set_row_spacing(6);
    grid.set_margin_top(8);
    for (row, (name, value)) in rows.iter().enumerate() {
        let name_label = gtk::Label::new(Some(name));
        name_label.set_xalign(0.0);
        name_label.set_yalign(0.0);
        name_label.add_css_class("dim-label");
        let value_label = gtk::Label::new(Some(value));
        value_label.set_xalign(0.0);
        value_label.set_wrap(true);
        value_label.set_hexpand(true);
        value_label.set_selectable(true);
        grid.attach(&name_label, 0, row as i32, 1, 1);
        grid.attach(&value_label, 1, row as i32, 1, 1);
    }
    grid
}

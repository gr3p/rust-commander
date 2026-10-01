use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::Ordering;

use relm4::gtk;
use relm4::gtk::gdk::Key;
use relm4::gtk::prelude::*;
use relm4::prelude::*;

use rust_commander::config::{self};
use rust_commander::entry::Entry;
use rust_commander::state::PaneId;
use rust_commander::theme::{self, Palette};

use crate::ui::msg::Msg;

use super::{App, SettingsUi, SIZING, home_dir};

impl App {
    pub(in crate::ui::app) fn apply_css(&self, palette: &Palette) {
        let image = self.background_path();
        let sheet = format!(
            "window, label, entry, button {{ font-family: \"JetBrainsMono Nerd Font\", \"JetBrains Mono\", monospace; font-size: 13px; }}\ntextview, textview text {{ font-family: \"DejaVu Sans Mono\", \"JetBrains Mono\", monospace; font-size: 13px; }}\n{}{}",
            theme::stylesheet(palette),
            theme::list_background(
                palette,
                &self.cfg.background_color,
                &image,
                self.cfg.background_alpha,
                self.cfg.background_stretch,
            ),
        );
        self.shell.css.load_from_data(&sheet);
        let name = theme::active_name(&self.cfg.theme, &config::active_theme_path());
        let terminal = theme::load_terminal(&name, &self.cfg.background_color);
        for shell in &self.shell.shells {
            crate::ui::shell::paint_terminal(&shell.term, &terminal);
        }
    }

    pub(in crate::ui::app) fn watch_columns(&self, sender: &ComponentSender<Self>) {
        for pane in 0..2 {
            let cols = self.shell.panes[pane].list.columns();
            for i in 0..cols.n_items() {
                let Some(col) = cols.item(i).and_then(|obj| obj.downcast::<gtk::ColumnViewColumn>().ok()) else { continue };
                let tx = sender.input_sender().clone();
                col.connect_fixed_width_notify(move |_| {
                    if SIZING.load(Ordering::Relaxed) {
                        return;
                    }
                    tx.emit(Msg::ColumnSized(pane));
                });
            }
        }
    }

    pub(in crate::ui::app) fn char_unit(&self) -> i32 {
        let layout = self.shell.panes[0].path.create_pango_layout(Some("00000000"));
        (layout.pixel_size().0 / 8).max(7)
    }

    pub(in crate::ui::app) fn layout_columns(&mut self, pane: usize, entries: &[Entry]) {
        let widths = if self.cfg.auto_columns {
            let unit = self.char_unit();
            let mut px = rust_commander::listing::column_pixels(rust_commander::listing::column_chars(entries), unit);
            if self.cfg.file_icons {
                px[0] += unit;
            }
            [px[0], px[1], px[2], px[3], px[4]]
        } else if self.cfg.column_widths.len() == 5 {
            let w = &self.cfg.column_widths;
            [w[0], w[1], w[2], w[3], w[4]]
        } else {
            return;
        };
        self.apply_widths(pane, widths);
    }

    pub(in crate::ui::app) fn apply_widths(&mut self, pane: usize, widths: [i32; 5]) {
        SIZING.store(true, Ordering::Relaxed);
        let cols = self.shell.panes[pane].list.columns();
        let mut data = 0;
        for i in 0..cols.n_items() {
            let Some(col) = cols.item(i).and_then(|obj| obj.downcast::<gtk::ColumnViewColumn>().ok()) else { continue };
            if col.title().map(|title| title.is_empty()).unwrap_or(true) {
                continue;
            }
            let Some(width) = widths.get(data) else { break };
            data += 1;
            if col.fixed_width() != *width {
                col.set_fixed_width(*width);
            }
        }
        SIZING.store(false, Ordering::Relaxed);
    }

    pub(in crate::ui::app) fn pane_widths(&self, pane: usize) -> Vec<i32> {
        let cols = self.shell.panes[pane].list.columns();
        let mut out = Vec::new();
        for i in 0..cols.n_items() {
            if let Some(col) = cols.item(i).and_then(|obj| obj.downcast::<gtk::ColumnViewColumn>().ok()) {
                if col.title().map(|title| title.is_empty()).unwrap_or(true) {
                    continue;
                }
                out.push(col.fixed_width());
            }
        }
        out
    }

    pub(in crate::ui::app) fn on_column_sized(&mut self, pane: usize) {
        let widths = self.pane_widths(pane);
        if widths == self.cfg.column_widths {
            return;
        }
        let turned_off = self.cfg.auto_columns;
        self.cfg.auto_columns = false;
        self.cfg.column_widths = widths.clone();
        let _ = config::save(&self.cfg);
        if widths.len() == 5 {
            let other = 1 - pane;
            self.apply_widths(other, [widths[0], widths[1], widths[2], widths[3], widths[4]]);
        }
        if turned_off {
            self.status = "Column widths kept".into();
        }
        self.paint_settings_switch();
    }

    pub(in crate::ui::app) fn toggle_settings(&mut self, sender: &ComponentSender<Self>) {
        if self.settings.is_some() {
            self.close_settings();
            return;
        }
        self.open_settings(sender);
    }

    pub(in crate::ui::app) fn open_settings(&mut self, sender: &ComponentSender<Self>) {
        let tx = sender.input_sender().clone();
        let win = gtk::Window::new();
        win.set_title(Some("Settings"));
        win.set_modal(true);
        win.set_transient_for(Some(&self.shell.window));
        win.set_default_width(540);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_propagate_natural_height(true);
        scroll.set_max_content_height(640);
        let (editor_row, editor, editor_ids, editor_guard) = editor_picker(&self.cfg.editor, tx.clone());
        column.append(&editor_row);
        let active_theme = theme::active_name(&self.cfg.theme, &config::active_theme_path());
        let (theme_row, theme, theme_ids, theme_guard) = theme_picker(&active_theme, tx.clone());
        column.append(&theme_row);
        let (icon_row, icon_pack, icon_pack_ids, icon_pack_guard) = icon_picker(&self.cfg.icon_pack, tx.clone());
        column.append(&icon_row);
        let (columns_row, columns) = settings_row("Auto column size", self.cfg.auto_columns);
        let tx_columns = tx.clone();
        let flash = columns_row.clone();
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| {
            crate::ui::shell::blink_widget(flash.upcast_ref());
            tx_columns.emit(Msg::ToggleAutoColumns);
        });
        columns_row.add_controller(click);
        let (hidden_row, hidden) = settings_row("Show hidden files    Ctrl+H", self.state.show_hidden);
        let tx_hidden = tx.clone();
        let flash = hidden_row.clone();
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| {
            crate::ui::shell::blink_widget(flash.upcast_ref());
            tx_hidden.emit(Msg::ToggleHidden);
        });
        hidden_row.add_controller(click);
        let (hints_row, hints) = settings_row("Mode buttons", self.cfg.mode_buttons);
        let tx_hints = tx.clone();
        let flash = hints_row.clone();
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| {
            crate::ui::shell::blink_widget(flash.upcast_ref());
            tx_hints.emit(Msg::ToggleModeButtons);
        });
        hints_row.add_controller(click);
        let (icons_row, icons) = settings_row("File icons", self.cfg.file_icons);
        let tx_icons = tx.clone();
        let flash = icons_row.clone();
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| {
            crate::ui::shell::blink_widget(flash.upcast_ref());
            tx_icons.emit(Msg::ToggleFileIcons);
        });
        icons_row.add_controller(click);
        let (flash_row, flash_switch) = settings_row("Confirmation flash", self.cfg.flash_feedback);
        let tx_flash = tx.clone();
        let flash_widget = flash_row.clone();
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| {
            crate::ui::shell::blink_widget(flash_widget.upcast_ref());
            tx_flash.emit(Msg::ToggleFlashFeedback);
        });
        flash_row.add_controller(click);
        column.append(&columns_row);
        column.append(&hidden_row);
        column.append(&hints_row);
        column.append(&icons_row);
        column.append(&flash_row);
        let palette = theme::load_named(&theme::active_name(&self.cfg.theme, &config::active_theme_path()));
        let color_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        color_row.add_css_class("settings-row");
        let color_name = gtk::Label::new(Some("Background color"));
        color_name.set_xalign(0.0);
        color_name.set_hexpand(true);
        let color_btn = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::new()));
        let color_flash = color_btn.clone();
        let shown = if self.cfg.background_color.is_empty() { palette.background.as_str() } else { self.cfg.background_color.as_str() };
        if let Ok(rgba) = gtk::gdk::RGBA::parse(shown) {
            color_btn.set_rgba(&rgba);
        }
        let tx_color = tx.clone();
        color_btn.connect_rgba_notify(move |btn| {
            crate::ui::shell::blink_widget(color_flash.upcast_ref());
            tx_color.emit(Msg::BackgroundColor(hex_rgb(&btn.rgba())));
        });
        color_row.append(&color_name);
        color_row.append(&color_btn);
        let image_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        image_row.add_css_class("settings-row");
        let pick = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        pick.set_hexpand(true);
        let image_name = gtk::Label::new(Some("Background image"));
        image_name.set_xalign(0.0);
        image_name.set_hexpand(true);
        let image = gtk::Label::new(Some(&image_caption(&self.cfg.background)));
        image.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        image.set_max_width_chars(18);
        pick.append(&image_name);
        pick.append(&image);
        let tx_image = tx.clone();
        let flash = pick.clone();
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| {
            crate::ui::shell::blink_widget(flash.upcast_ref());
            tx_image.emit(Msg::PickBackground);
        });
        pick.add_controller(click);
        let clear = gtk::Button::with_label("Remove");
        clear.set_sensitive(!self.cfg.background.trim().is_empty());
        let tx_clear = tx.clone();
        crate::ui::shell::on_click(&clear, move || tx_clear.emit(Msg::ClearBackground));
        image_row.append(&pick);
        image_row.append(&clear);
        let (stretch_row, stretch) = settings_row("Stretch background", self.cfg.background_stretch);
        let tx_stretch = tx.clone();
        let flash = stretch_row.clone();
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| {
            crate::ui::shell::blink_widget(flash.upcast_ref());
            tx_stretch.emit(Msg::ToggleBackgroundStretch);
        });
        stretch_row.add_controller(click);
        let strength_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        strength_row.add_css_class("settings-row");
        let strength_label = if self.background_path().is_empty() { "Opacity" } else { "Image strength" };
        let strength_name = gtk::Label::new(Some(strength_label));
        strength_name.set_xalign(0.0);
        let strength = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
        strength.set_value(f64::from(self.cfg.background_alpha));
        strength.set_digits(0);
        strength.set_hexpand(true);
        strength.set_draw_value(true);
        strength.set_value_pos(gtk::PositionType::Right);
        let tx_strength = tx.clone();
        strength.connect_value_changed(move |scale| tx_strength.emit(Msg::BackgroundAlpha(scale.value().round() as u8)));
        strength_row.append(&strength_name);
        strength_row.append(&strength);
        column.append(&color_row);
        column.append(&image_row);
        column.append(&stretch_row);
        column.append(&strength_row);
        column.append(&tool_row("7-Zip", rust_commander::pack::SEVEN_ZIP_HINT, &self.cfg.seven_zip, "/usr/bin/7z", tx.clone(), true));
        column.append(&tool_row("RAR", rust_commander::pack::RAR_HINT, &self.cfg.rar, "/usr/bin/rar", tx.clone(), false));
        column.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        column.append(&commands_row(&win));
        scroll.set_child(Some(&column));
        win.set_child(Some(&scroll));
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let keys_win = win.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            let on_control = focused_control(&keys_win);
            match key {
                Key::Escape | Key::F12 => tx.emit(Msg::CloseSettings),
                Key::Return | Key::KP_Enter | Key::space | Key::KP_Space if !on_control => tx.emit(Msg::ToggleAutoColumns),
                Key::h | Key::H if !on_control => tx.emit(Msg::ToggleHidden),
                _ if on_control => return gtk::glib::Propagation::Proceed,
                _ => {}
            }
            gtk::glib::Propagation::Stop
        });
        win.add_controller(keys);
        win.present();
        win.grab_focus();
        self.settings = Some(SettingsUi {
            window: win,
            columns,
            hidden,
            hints,
            icons,
            flash: flash_switch,
            image,
            clear,
            stretch,
            strength_name,
            strength,
            editor,
            editor_ids,
            editor_guard,
            theme,
            theme_ids,
            theme_guard,
            icon_pack,
            icon_pack_ids,
            icon_pack_guard,
        });
    }

    pub(in crate::ui::app) fn set_editor(&mut self, id: String) {
        if id == rust_commander::external::EDITOR_BROWSE {
            return;
        }
        if self.cfg.editor != id {
            self.cfg.editor = id;
            let _ = config::save(&self.cfg);
        }
        self.paint_editor();
    }

    pub(in crate::ui::app) fn paint_editor(&self) {
        let Some(ui) = &self.settings else { return };
        fill_editor(&ui.editor, &ui.editor_ids, &ui.editor_guard, &self.cfg.editor);
    }

    // Written next to the config so an external `omarchy/theme-set` and this picker agree on the last choice.
    pub(in crate::ui::app) fn set_theme(&mut self, name: String) {
        if self.cfg.theme != name {
            self.cfg.theme = name.clone();
            let _ = config::save(&self.cfg);
        }
        let _ = std::fs::write(config::active_theme_path(), format!("{name}\n"));
        self.restyle();
        self.paint_theme();
    }

    pub(in crate::ui::app) fn paint_theme(&self) {
        let Some(ui) = &self.settings else { return };
        let active = theme::active_name(&self.cfg.theme, &config::active_theme_path());
        fill_theme(&ui.theme, &ui.theme_ids, &ui.theme_guard, &active);
    }

    pub(in crate::ui::app) fn set_icon_pack(&mut self, id: String) {
        if self.cfg.icon_pack != id {
            self.cfg.icon_pack = id;
            let _ = config::save(&self.cfg);
        }
        crate::ui::shell::repaint_drives(&self.shell.drives, &self.cfg.icon_pack);
        self.touch_both();
        self.paint_icon_pack();
    }

    pub(in crate::ui::app) fn paint_icon_pack(&self) {
        let Some(ui) = &self.settings else { return };
        fill_icon_pack(&ui.icon_pack, &ui.icon_pack_ids, &ui.icon_pack_guard, &self.cfg.icon_pack);
    }

    pub(in crate::ui::app) fn pick_editor(&mut self, sender: &ComponentSender<Self>) {
        let Some(parent) = self.settings.as_ref().map(|ui| ui.window.clone()) else { return };
        let dialog = gtk::FileDialog::new();
        dialog.set_title("Editor program");
        dialog.set_modal(true);
        dialog.set_accept_label(Some("Choose"));
        let bin = PathBuf::from("/usr/bin");
        if bin.is_dir() {
            dialog.set_initial_folder(Some(&gtk::gio::File::for_path(&bin)));
        }
        let tx = sender.input_sender().clone();
        dialog.open(Some(&parent), None::<&gtk::gio::Cancellable>, move |result| match result {
            Ok(file) => {
                if let Some(path) = file.path() {
                    tx.emit(Msg::SetEditor(rust_commander::external::editor_id_for_path(&path)));
                }
            }
            Err(_) => tx.emit(Msg::EditorPickCancelled),
        });
    }

    pub(in crate::ui::app) fn close_settings(&mut self) {
        if let Some(ui) = self.settings.take() {
            ui.window.close();
        }
    }

    pub(in crate::ui::app) fn toggle_auto_columns(&mut self) {
        self.cfg.auto_columns = !self.cfg.auto_columns;
        if self.cfg.auto_columns {
            self.cfg.column_widths.clear();
            self.relayout_columns();
        } else {
            self.cfg.column_widths = self.pane_widths(self.state.active.index());
        }
        let _ = config::save(&self.cfg);
        self.paint_settings_switch();
    }

    pub(in crate::ui::app) fn toggle_hidden(&mut self) {
        self.state.show_hidden = !self.state.show_hidden;
        self.cfg.show_hidden = self.state.show_hidden;
        let _ = config::save(&self.cfg);
        self.touch_both();
        self.paint_settings_switch();
    }

    pub(in crate::ui::app) fn toggle_file_icons(&mut self) {
        self.cfg.file_icons = !self.cfg.file_icons;
        let _ = config::save(&self.cfg);
        self.touch_both();
        self.paint_settings_switch();
    }

    pub(in crate::ui::app) fn toggle_mode_buttons(&mut self) {
        self.cfg.mode_buttons = !self.cfg.mode_buttons;
        let _ = config::save(&self.cfg);
        if let Some(ui) = &self.image {
            ui.bar.set_visible(self.cfg.mode_buttons);
        }
        if let Some(ui) = &self.lister {
            ui.modes.bar.set_visible(self.cfg.mode_buttons);
        }
        self.paint_settings_switch();
    }

    pub(in crate::ui::app) fn toggle_flash_feedback(&mut self) {
        self.cfg.flash_feedback = !self.cfg.flash_feedback;
        crate::ui::shell::set_flash_enabled(self.cfg.flash_feedback);
        let _ = config::save(&self.cfg);
        self.paint_settings_switch();
    }

    pub(in crate::ui::app) fn toggle_background_stretch(&mut self) {
        self.cfg.background_stretch = !self.cfg.background_stretch;
        let _ = config::save(&self.cfg);
        self.restyle();
        self.paint_settings_switch();
    }

    pub(in crate::ui::app) fn relayout_columns(&mut self) {
        for pane in 0..2 {
            let id = PaneId::from_index(pane);
            let entries: Vec<Entry> = self
                .state
                .visible(id)
                .into_iter()
                .map(|i| self.state.tab(id).entries[i].clone())
                .collect();
            self.layout_columns(pane, &entries);
        }
    }

    pub(in crate::ui::app) fn paint_settings_switch(&self) {
        let Some(ui) = &self.settings else { return };
        paint_switch(&ui.columns, self.cfg.auto_columns);
        paint_switch(&ui.hidden, self.state.show_hidden);
        paint_switch(&ui.hints, self.cfg.mode_buttons);
        paint_switch(&ui.icons, self.cfg.file_icons);
        paint_switch(&ui.flash, self.cfg.flash_feedback);
        paint_switch(&ui.stretch, self.cfg.background_stretch);
        ui.image.set_text(&image_caption(&self.cfg.background));
        ui.clear.set_sensitive(!self.cfg.background.trim().is_empty());
        let label = if self.background_path().is_empty() { "Opacity" } else { "Image strength" };
        ui.strength_name.set_text(label);
    }

    pub(in crate::ui::app) fn background_path(&self) -> String {
        let path = self.cfg.background.trim();
        if Path::new(path).is_file() { path.to_string() } else { String::new() }
    }

    pub(in crate::ui::app) fn restyle(&self) {
        let name = theme::active_name(&self.cfg.theme, &config::active_theme_path());
        self.apply_css(&theme::load_named(&name));
    }

    pub(in crate::ui::app) fn pick_background(&mut self, sender: &ComponentSender<Self>) {
        let Some(parent) = self.settings.as_ref().map(|ui| ui.window.clone()) else { return };
        let dialog = gtk::FileDialog::new();
        dialog.set_title("Background image");
        dialog.set_modal(true);
        dialog.set_accept_label(Some("Open"));
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Images"));
        filter.add_mime_type("image/png");
        filter.add_mime_type("image/jpeg");
        filter.add_mime_type("image/webp");
        filter.add_mime_type("image/gif");
        dialog.set_default_filter(Some(&filter));
        let path = PathBuf::from(&self.cfg.background);
        if path.is_file() {
            dialog.set_initial_file(Some(&gtk::gio::File::for_path(&path)));
        } else {
            let pictures = home_dir().join("Pictures");
            if pictures.is_dir() {
                dialog.set_initial_folder(Some(&gtk::gio::File::for_path(&pictures)));
            }
        }
        let tx = sender.input_sender().clone();
        dialog.open(Some(&parent), None::<&gtk::gio::Cancellable>, move |result| {
            if let Ok(file) = result {
                if let Some(path) = file.path() {
                    tx.emit(Msg::BackgroundImage(path));
                }
            }
        });
    }

    pub(in crate::ui::app) fn clear_background(&mut self) {
        if self.cfg.background.is_empty() {
            return;
        }
        self.cfg.background.clear();
        // Image strength does not map to opacity. Land on a solid color.
        self.cfg.background_alpha = 100;
        if let Some(ui) = &self.settings {
            ui.strength.set_value(100.0);
        }
        let _ = config::save(&self.cfg);
        self.restyle();
        self.paint_settings_switch();
    }

    pub(in crate::ui::app) fn set_background_image(&mut self, path: PathBuf) {
        self.cfg.background = path.display().to_string();
        if self.cfg.background_alpha == 0 {
            self.cfg.background_alpha = 70;
            if let Some(ui) = &self.settings {
                ui.strength.set_value(70.0);
            }
        }
        let _ = config::save(&self.cfg);
        self.restyle();
        self.paint_settings_switch();
    }

    pub(in crate::ui::app) fn set_background_color(&mut self, color: String) {
        if self.cfg.background_color == color {
            return;
        }
        self.cfg.background_color = color;
        let _ = config::save(&self.cfg);
        self.restyle();
    }

    pub(in crate::ui::app) fn set_background_alpha(&mut self, alpha: u8) {
        let alpha = alpha.min(100);
        if self.cfg.background_alpha == alpha {
            return;
        }
        self.cfg.background_alpha = alpha;
        let _ = config::save(&self.cfg);
        self.restyle();
    }
}

pub(in crate::ui::app) fn image_caption(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() {
        return "none".into();
    }
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

pub(in crate::ui::app) fn hex_rgb(rgba: &gtk::gdk::RGBA) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        (rgba.red() * 255.0).round() as u8,
        (rgba.green() * 255.0).round() as u8,
        (rgba.blue() * 255.0).round() as u8,
    )
}

pub(in crate::ui::app) fn editor_picker(configured: &str, tx: relm4::Sender<Msg>) -> (gtk::Box, gtk::DropDown, Rc<RefCell<Vec<String>>>, Rc<Cell<bool>>) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("settings-row");
    let name = gtk::Label::new(Some("Editor    F4"));
    name.set_xalign(0.0);
    let choices = rust_commander::external::editor_choices(configured);
    let labels: Vec<&str> = choices.iter().map(|choice| choice.label.as_str()).collect();
    let drop = gtk::DropDown::from_strings(&labels);
    drop.add_css_class("editor-pick");
    drop.set_hexpand(true);
    let ids = Rc::new(RefCell::new(choices.iter().map(|choice| choice.id.clone()).collect::<Vec<_>>()));
    let guard = Rc::new(Cell::new(false));
    let selected = choices.iter().position(|choice| choice.id == configured).unwrap_or(0) as u32;
    drop.set_selected(selected);
    let ids_notify = ids.clone();
    let guard_notify = guard.clone();
    drop.connect_selected_notify(move |drop| {
        if guard_notify.get() {
            return;
        }
        let id = ids_notify.borrow().get(drop.selected() as usize).cloned().unwrap_or_default();
        if id == rust_commander::external::EDITOR_BROWSE {
            tx.emit(Msg::PickEditor);
            return;
        }
        tx.emit(Msg::SetEditor(id));
    });
    row.append(&name);
    row.append(&drop);
    (row, drop, ids, guard)
}

pub(in crate::ui::app) fn fill_editor(drop: &gtk::DropDown, ids: &Rc<RefCell<Vec<String>>>, guard: &Rc<Cell<bool>>, configured: &str) {
    let choices = rust_commander::external::editor_choices(configured);
    let labels: Vec<&str> = choices.iter().map(|choice| choice.label.as_str()).collect();
    let selected = choices.iter().position(|choice| choice.id == configured).unwrap_or(0) as u32;
    guard.set(true);
    drop.set_model(Some(&gtk::StringList::new(&labels)));
    drop.set_selected(selected);
    *ids.borrow_mut() = choices.into_iter().map(|choice| choice.id).collect();
    guard.set(false);
}

pub(in crate::ui::app) fn theme_picker(active: &str, tx: relm4::Sender<Msg>) -> (gtk::Box, gtk::DropDown, Rc<RefCell<Vec<String>>>, Rc<Cell<bool>>) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("settings-row");
    let name = gtk::Label::new(Some("Theme"));
    name.set_xalign(0.0);
    let names = theme::available_themes();
    let labels: Vec<&str> = names.iter().map(String::as_str).collect();
    let drop = gtk::DropDown::from_strings(&labels);
    drop.add_css_class("editor-pick");
    drop.set_hexpand(true);
    let ids = Rc::new(RefCell::new(names.clone()));
    let guard = Rc::new(Cell::new(false));
    let selected = names.iter().position(|n| n == active).unwrap_or(0) as u32;
    drop.set_selected(selected);
    let ids_notify = ids.clone();
    let guard_notify = guard.clone();
    drop.connect_selected_notify(move |drop| {
        if guard_notify.get() {
            return;
        }
        let Some(name) = ids_notify.borrow().get(drop.selected() as usize).cloned() else { return };
        tx.emit(Msg::SetTheme(name));
    });
    row.append(&name);
    row.append(&drop);
    (row, drop, ids, guard)
}

pub(in crate::ui::app) fn icon_picker(active: &str, tx: relm4::Sender<Msg>) -> (gtk::Box, gtk::DropDown, Rc<RefCell<Vec<String>>>, Rc<Cell<bool>>) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("settings-row");
    let name = gtk::Label::new(Some("Icons"));
    name.set_xalign(0.0);
    let choices = crate::ui::iconpack::choices();
    let labels: Vec<&str> = choices.iter().map(|(_, label)| label.as_str()).collect();
    let drop = gtk::DropDown::from_strings(&labels);
    drop.add_css_class("editor-pick");
    drop.set_hexpand(true);
    let ids = Rc::new(RefCell::new(choices.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>()));
    let guard = Rc::new(Cell::new(false));
    let selected = choices.iter().position(|(id, _)| id == active).unwrap_or(0) as u32;
    drop.set_selected(selected);
    let ids_notify = ids.clone();
    let guard_notify = guard.clone();
    drop.connect_selected_notify(move |drop| {
        if guard_notify.get() {
            return;
        }
        let Some(id) = ids_notify.borrow().get(drop.selected() as usize).cloned() else { return };
        tx.emit(Msg::SetIconPack(id));
    });
    row.append(&name);
    row.append(&drop);
    (row, drop, ids, guard)
}

pub(in crate::ui::app) fn fill_icon_pack(drop: &gtk::DropDown, ids: &Rc<RefCell<Vec<String>>>, guard: &Rc<Cell<bool>>, active: &str) {
    let choices = crate::ui::iconpack::choices();
    let labels: Vec<&str> = choices.iter().map(|(_, label)| label.as_str()).collect();
    let selected = choices.iter().position(|(id, _)| id == active).unwrap_or(0) as u32;
    guard.set(true);
    drop.set_model(Some(&gtk::StringList::new(&labels)));
    drop.set_selected(selected);
    *ids.borrow_mut() = choices.into_iter().map(|(id, _)| id).collect();
    guard.set(false);
}

pub(in crate::ui::app) fn fill_theme(drop: &gtk::DropDown, ids: &Rc<RefCell<Vec<String>>>, guard: &Rc<Cell<bool>>, active: &str) {
    let names = theme::available_themes();
    let labels: Vec<&str> = names.iter().map(String::as_str).collect();
    let selected = names.iter().position(|n| n == active).unwrap_or(0) as u32;
    guard.set(true);
    drop.set_model(Some(&gtk::StringList::new(&labels)));
    drop.set_selected(selected);
    *ids.borrow_mut() = names;
    guard.set(false);
}

pub(in crate::ui::app) fn focused_control(win: &gtk::Window) -> bool {
    let Some(mut widget) = relm4::gtk::prelude::GtkWindowExt::focus(win) else { return false };
    loop {
        if widget.downcast_ref::<gtk::Scale>().is_some()
            || widget.downcast_ref::<gtk::ColorDialogButton>().is_some()
            || widget.downcast_ref::<gtk::DropDown>().is_some()
            || widget.downcast_ref::<gtk::Entry>().is_some()
            || widget.downcast_ref::<gtk::Button>().is_some()
        {
            return true;
        }
        match widget.parent() {
            Some(parent) => widget = parent,
            None => return false,
        }
    }
}

pub(in crate::ui::app) fn commands_row(parent: &gtk::Window) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("settings-row");
    let name = gtk::Label::new(Some("Keyboard"));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    let open = gtk::Button::with_label("Commands");
    open.add_css_class("fkey");
    let parent = parent.clone();
    crate::ui::shell::on_click(&open, move || show_commands(&parent));
    row.append(&name);
    row.append(&open);
    row
}

pub(in crate::ui::app) fn show_commands(parent: &gtk::Window) {
    let win = gtk::Window::new();
    win.set_title(Some("Keyboard"));
    win.set_modal(true);
    win.set_transient_for(Some(parent));
    win.set_default_width(680);
    win.set_default_height(560);
    let columns = gtk::Box::new(gtk::Orientation::Horizontal, 36);
    columns.set_halign(gtk::Align::Start);
    columns.set_valign(gtk::Align::Start);
    columns.set_margin_start(16);
    columns.set_margin_end(16);
    columns.set_margin_bottom(16);
    columns.append(&command_groups(&[
        ("Sort", &[
            ("Ctrl+F3", "Name"),
            ("Ctrl+F4", "Extension"),
            ("Ctrl+F5", "Date"),
            ("Ctrl+F6", "Size"),
            ("Ctrl+F7", "Attributes"),
        ]),
        ("Navigate", &[
            ("Tab", "Other pane"),
            ("Ctrl+Tab", "Next tab"),
            ("Ctrl+T", "New tab"),
            ("Ctrl+W", "Close tab"),
            ("BackSpace", "Up"),
            ("Alt+Left", "Back"),
            ("Alt+Right", "Forward"),
            ("Ctrl+Left/Right", "Mirror folder"),
            ("Alt+F1", "Left places"),
            ("Alt+F2", "Right places"),
            ("Ctrl+D", "Hotlist"),
            ("Enter", "Open"),
            ("Insert", "Mark"),
            ("Space", "Mark"),
        ]),
        ("Lister", &[
            ("1", "Text"),
            ("2", "Binary"),
            ("3", "Hex"),
            ("A", "Ansi"),
            ("S / D", "Ascii"),
        ]),
    ]));
    columns.append(&command_groups(&[
        ("Files", &[
            ("F2", "Refresh"),
            ("F3", "View"),
            ("F4", "Edit"),
            ("Shift+F4", "New file"),
            ("F5", "Copy"),
            ("F6", "Move"),
            ("F2 in F5/F6", "Add to queue"),
            ("F7", "New folder"),
            ("F8 / Delete", "Trash"),
            ("Shift+F5", "Copy as"),
            ("Shift+F6", "Rename"),
            ("Shift+F8", "Delete"),
            ("F12", "Settings"),
        ]),
        ("Archive", &[
            ("Alt+F5", "Pack"),
            ("Ctrl+Shift+F5", "Pack here"),
            ("Alt+F9", "Unpack to other panel"),
            ("Shift+Alt+F9", "Extract here"),
            ("Shift+F9", "Create SFV"),
            ("Ctrl+Shift+F9", "Verify SFV"),
            ("Enter", "Verify SFV"),
        ]),
        ("View", &[
            ("Ctrl+S", "Filter"),
            ("Ctrl+H", "Hidden"),
            ("Ctrl+Q", "Quick view"),
            ("F9", "Terminal in other panel"),
            ("Ctrl+F9", "New shell"),
            ("Ctrl+W", "Close shell"),
            ("Alt+1…9", "Switch shell"),
            ("Alt+F3", "Viewer"),
            ("Escape", "Close view"),
            ("S", "Fit or 1:1"),
            ("Left/Right", "Previous or next"),
        ]),
    ]));
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_child(Some(&columns));
    win.set_child(Some(&scroll));
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let close = win.clone();
    keys.connect_key_pressed(move |_, key, _, _| {
        if matches!(key, Key::Escape | Key::F12) {
            close.close();
        }
        gtk::glib::Propagation::Stop
    });
    win.add_controller(keys);
    win.present();
}

pub(in crate::ui::app) fn command_groups(groups: &[(&str, &[(&str, &str)])]) -> gtk::Grid {
    let grid = gtk::Grid::new();
    grid.set_column_spacing(18);
    grid.set_row_spacing(3);
    let mut y = 0;
    for (title, rows) in groups {
        let head = gtk::Label::new(Some(title));
        head.set_xalign(0.0);
        head.add_css_class("cmd-head");
        grid.attach(&head, 0, y, 2, 1);
        y += 1;
        for (key, action) in *rows {
            let key_label = gtk::Label::new(Some(key));
            key_label.set_xalign(0.0);
            key_label.add_css_class("cmd-key");
            let action_label = gtk::Label::new(Some(action));
            action_label.set_xalign(0.0);
            action_label.set_hexpand(true);
            grid.attach(&key_label, 0, y, 1, 1);
            grid.attach(&action_label, 1, y, 1, 1);
            y += 1;
        }
    }
    grid
}

pub(in crate::ui::app) fn tool_row(title: &str, hint: &str, value: &str, placeholder: &str, tx: relm4::Sender<Msg>, seven: bool) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
    column.add_css_class("settings-row");
    column.set_margin_start(12);
    column.set_margin_end(12);
    column.set_margin_top(8);
    let name = gtk::Label::new(Some(title));
    name.set_xalign(0.0);
    let entry = gtk::Entry::new();
    entry.set_text(value);
    entry.set_placeholder_text(Some(placeholder));
    entry.connect_changed(move |entry| {
        let path = entry.text().to_string();
        if seven {
            tx.emit(Msg::SetSevenZip(path));
        } else {
            tx.emit(Msg::SetRar(path));
        }
    });
    let help = gtk::Label::new(Some(hint));
    help.set_wrap(true);
    help.set_xalign(0.0);
    help.set_max_width_chars(72);
    column.append(&name);
    column.append(&entry);
    column.append(&help);
    column
}

pub(in crate::ui::app) fn settings_row(title: &str, on: bool) -> (gtk::Box, gtk::Label) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let name = gtk::Label::new(Some(title));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    let value = gtk::Label::new(Some(if on { "on" } else { "off" }));
    if on {
        value.add_css_class("settings-on");
    }
    row.append(&name);
    row.append(&value);
    row.add_css_class("settings-row");
    (row, value)
}

pub(in crate::ui::app) fn paint_switch(label: &gtk::Label, on: bool) {
    label.set_text(if on { "on" } else { "off" });
    if on {
        label.add_css_class("settings-on");
    } else {
        label.remove_css_class("settings-on");
    }
}

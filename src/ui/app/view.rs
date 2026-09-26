use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use relm4::gtk;
use relm4::gtk::prelude::*;
use relm4::prelude::*;
use relm4::RelmWidgetExt;

use comarchy::imageview;
use comarchy::lister::{self, Charset, PreviewBody, ViewMode};
use comarchy::state::PaneId;

use crate::ui::msg::Msg;

use super::{App, ImageUi, ListerUi};

impl App {
    pub(in crate::ui::app) fn open_viewer(&mut self, sender: &ComponentSender<Self>) {
        let image = self
            .state
            .cursor_entry(self.state.active)
            .is_some_and(|entry| !entry.is_dir() && imageview::is_image(&entry.name));
        if image {
            self.open_image(sender);
        } else {
            self.open_lister(sender);
        }
    }

    pub(in crate::ui::app) fn open_image(&mut self, sender: &ComponentSender<Self>) {
        let Some(path) = self.file_under_cursor() else { return };
        let id = self.state.active;
        let files = self.image_files(id);
        let Some(index) = files.iter().position(|item| item == &path) else { return };
        self.close_lister();
        self.close_image();
        self.image_token = self.image_token.wrapping_add(1);
        let token = self.image_token;
        let window = gtk::Window::new();
        window.set_title(Some(&path.display().to_string()));
        window.set_transient_for(Some(&self.shell.window));
        window.set_default_width(800);
        window.set_default_height(600);
        let picture = gtk::Picture::new();
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_child(Some(&picture));
        scroll.set_hexpand(true);
        scroll.set_vexpand(true);
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        bar.add_css_class("fbar");
        bar.set_margin_all(4);
        let scale = gtk::Button::with_label("S Fit");
        scale.add_css_class("fkey");
        scale.set_hexpand(true);
        let tx_scale = sender.input_sender().clone();
        scale.connect_clicked(move |_| tx_scale.emit(Msg::ToggleImageScale));
        bar.append(&scale);
        bar.set_visible(self.cfg.mode_buttons);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.append(&scroll);
        column.append(&bar);
        window.set_child(Some(&column));
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let tx = sender.input_sender().clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            tx.emit(Msg::Key(key, mods));
            gtk::glib::Propagation::Stop
        });
        window.add_controller(keys);
        let tx = sender.input_sender().clone();
        window.connect_close_request(move |_| {
            tx.emit(Msg::ImageClosed(token));
            gtk::glib::Propagation::Proceed
        });
        window.present();
        let ui = ImageUi { window, picture, scroll, bar, scale, files, index, generation: 0, token, fit: true };
        apply_image_scale(&ui);
        self.image = Some(ui);
        self.load_image(sender);
    }

    pub(in crate::ui::app) fn load_image(&mut self, sender: &ComponentSender<Self>) {
        self.image_gen = self.image_gen.wrapping_add(1);
        let generation = self.image_gen;
        let Some(ui) = self.image.as_mut() else { return };
        ui.generation = generation;
        let Some(path) = ui.files.get(ui.index).cloned() else { return };
        let fit = ui.fit;
        ui.window.set_title(Some(&path.display().to_string()));
        let live = Arc::clone(&self.image_live);
        live.store(generation, Ordering::Relaxed);
        let tx = sender.input_sender().clone();
        std::thread::spawn(move || {
            if live.load(Ordering::Relaxed) != generation {
                return;
            }
            let edge = if fit { imageview::MAX_EDGE } else { u32::MAX };
            let body = preview_body(&path, edge);
            if live.load(Ordering::Relaxed) != generation {
                return;
            }
            tx.emit(Msg::ImageReady { generation, body });
        });
    }

    pub(in crate::ui::app) fn show_image(&mut self, generation: u64, body: PreviewBody) {
        let picture = self
            .image
            .as_ref()
            .filter(|ui| ui.generation == generation)
            .map(|ui| ui.picture.clone());
        let Some(picture) = picture else { return };
        let fit = self.image.as_ref().is_some_and(|ui| ui.fit);
        match body {
            PreviewBody::Image { width, height, pixels } => {
                set_texture(&picture, width, height, &pixels);
                picture.set_size_request(if fit { -1 } else { width as i32 }, if fit { -1 } else { height as i32 });
            }
            PreviewBody::Text(err) => self.status = err,
        }
    }

    pub(in crate::ui::app) fn step_image(&mut self, delta: isize, sender: &ComponentSender<Self>) {
        let Some(ui) = self.image.as_mut() else { return };
        let Some(next) = imageview::step_index(ui.files.len(), ui.index, delta) else { return };
        ui.index = next;
        self.load_image(sender);
    }

    pub(in crate::ui::app) fn close_image(&mut self) {
        self.image_gen = self.image_gen.wrapping_add(1);
        self.image_live.store(self.image_gen, Ordering::Relaxed);
        if let Some(ui) = self.image.take() {
            ui.window.close();
        }
    }

    pub(in crate::ui::app) fn step_quick_image(&mut self, delta: isize) {
        if !self.state.quick_view {
            return;
        }
        let id = self.state.active;
        let Some(entry) = self.state.cursor_entry(id) else { return };
        if entry.is_dir() || !imageview::is_image(&entry.name) {
            return;
        }
        let Some(path) = self.file_under_cursor() else { return };
        let files = self.image_files(id);
        let Some(index) = files.iter().position(|item| item == &path) else { return };
        let Some(next) = imageview::step_index(files.len(), index, delta) else { return };
        let Some(name) = files[next].file_name().and_then(|name| name.to_str()) else { return };
        let name = name.to_string();
        if self.point_at(id, &name) {
            self.focus_list();
        }
    }

    pub(in crate::ui::app) fn image_files(&self, id: PaneId) -> Vec<PathBuf> {
        let dir = &self.state.tab(id).path;
        let entries = &self.state.tab(id).entries;
        let order = self.state.visible(id);
        imageview::image_paths(dir, entries, &order)
    }

    pub(in crate::ui::app) fn open_lister(&mut self, sender: &ComponentSender<Self>) {
        self.close_image();
        let Some(path) = self.file_under_cursor() else { return };
        let Ok(mut reader) = lister::Reader::open(&path) else {
            self.status = format!("Cannot open {}", path.display());
            return;
        };
        let text = match lister::render_reader(&mut reader, ViewMode::Text, self.charset) {
            Ok(text) => text,
            Err(_) => {
                self.status = format!("Cannot open {}", path.display());
                return;
            }
        };
        self.close_lister();
        let window = gtk::Window::new();
        window.set_title(Some(&path.display().to_string()));
        window.set_transient_for(Some(&self.shell.window));
        window.set_default_width(800);
        window.set_default_height(600);
        let view = gtk::TextView::new();
        view.set_editable(false);
        view.set_monospace(true);
        view.buffer().set_text(&text);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_child(Some(&view));
        scroll.set_vexpand(true);
        let modes = crate::ui::shell::charset_bar(sender.input_sender());
        modes.bar.set_visible(self.cfg.mode_buttons);
        paint_charset(&modes, self.charset);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.append(&scroll);
        column.append(&modes.bar);
        window.set_child(Some(&column));
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let tx = sender.input_sender().clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            tx.emit(Msg::Key(key, mods));
            gtk::glib::Propagation::Stop
        });
        window.add_controller(keys);
        window.present();
        self.lister = Some(ListerUi { window, view, reader, mode: ViewMode::Text, modes });
    }

    pub(in crate::ui::app) fn toggle_quick_edit(&mut self, sender: &ComponentSender<Self>) {
        if self.shell.editing.get() {
            self.stop_quick_edit(true, sender);
            return;
        }
        if !self.state.quick_view || self.cursor_is_image() {
            self.status = "Edit is for text".into();
            return;
        }
        let Some(path) = self.file_under_cursor() else {
            self.status = "Edit is for text".into();
            return;
        };
        let text = match lister::read_for_edit(&path, self.charset) {
            Ok(text) => text,
            Err(err) => {
                self.status = err;
                return;
            }
        };
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("file").to_string();
        *self.edit_name.borrow_mut() = name.clone();
        self.shell.window.set_title(Some(&name));
        let preview = &self.shell.panes[self.state.active.other().index()].preview;
        self.edit_quiet.set(true);
        preview.buffer().set_text(&text);
        self.edit_quiet.set(false);
        self.shell.editing.set(true);
        preview.set_editable(true);
        preview.set_cursor_visible(true);
        preview.grab_focus();
        paint_edit(&self.shell.edit_button, true);
    }

    pub(in crate::ui::app) fn stop_quick_edit(&mut self, reload: bool, sender: &ComponentSender<Self>) {
        if !self.shell.editing.get() {
            return;
        }
        self.shell.editing.set(false);
        for pane in &self.shell.panes {
            pane.preview.set_editable(false);
        }
        paint_edit(&self.shell.edit_button, false);
        self.shell.window.set_title(Some("Commander"));
        if reload {
            self.note_preview_target();
            self.kick_preview(sender);
        }
        self.focus_list();
    }

    pub(in crate::ui::app) fn save_quick(&mut self) {
        if !self.shell.editing.get() {
            return;
        }
        let text = {
            let preview = &self.shell.panes[self.state.active.other().index()].preview;
            let buffer = preview.buffer();
            buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string()
        };
        let Some(path) = self.file_under_cursor() else { return };
        match lister::write_text(&path, &text) {
            Ok(()) => {
                self.shell.window.set_title(Some(&self.edit_name.borrow()));
                self.status = "Saved".into();
            }
            Err(err) => self.status = err.to_string(),
        }
    }

    pub(in crate::ui::app) fn lister_set_mode(&mut self, mode: ViewMode) {
        if let Some(lister) = self.lister.as_mut() {
            lister.mode = mode;
        }
        self.lister_redraw();
    }

    pub(in crate::ui::app) fn lister_scroll(&mut self, delta: i64) {
        let json = self.charset == Charset::Json;
        if let Some(lister) = self.lister.as_mut() {
            if json && lister.mode == ViewMode::Text {
                scroll_text(&lister.view, delta);
                return;
            }
            if delta != 0 {
                lister.reader.scroll(delta);
            } else {
                lister.reader.offset = lister.reader.offset.min(lister.reader.len);
            }
        }
        self.lister_redraw();
    }

    pub(in crate::ui::app) fn lister_redraw(&mut self) {
        let charset = self.charset;
        let Some(lister) = self.lister.as_mut() else { return };
        let mode = lister.mode;
        if let Ok(text) = lister::render_reader(&mut lister.reader, mode, charset) {
            lister.view.buffer().set_text(&text);
        }
    }

    pub(in crate::ui::app) fn set_charset(&mut self, charset: Charset, sender: &ComponentSender<Self>) {
        if self.shell.editing.get() {
            self.status = "Leave edit first".into();
            return;
        }
        self.charset = charset;
        self.status = match charset {
            Charset::Utf8 => "UTF-8".into(),
            Charset::Ansi => "ANSI".into(),
            Charset::Ascii => "ASCII".into(),
            Charset::Json => "JSON".into(),
        };
        self.lister_redraw();
        if let Some(ui) = &self.lister {
            paint_charset(&ui.modes, charset);
        }
        paint_charset(&self.shell.text_modes, charset);
        if self.state.quick_view {
            self.note_preview_target();
            self.kick_preview(sender);
        }
    }

    pub(in crate::ui::app) fn close_lister(&mut self) {
        if let Some(lister) = self.lister.take() {
            lister.window.close();
        }
    }

    pub(in crate::ui::app) fn note_preview_target(&mut self) {
        if self.state.quick_view {
            self.state.preview_gen = self.state.preview_gen.wrapping_add(1);
        }
    }

    pub(in crate::ui::app) fn kick_preview(&mut self, sender: &ComponentSender<Self>) {
        if !self.state.quick_view || self.preview_sent == self.state.preview_gen {
            return;
        }
        let generation = self.state.preview_gen;
        self.preview.generation = generation;
        self.preview_sent = generation;
        let Some(path) = self.file_under_cursor() else {
            let text = self
                .state
                .cursor_entry(self.state.active)
                .filter(|entry| entry.is_dir())
                .map(|entry| format!("{}/", entry.name))
                .unwrap_or_default();
            self.preview.set_body(PreviewBody::Text(text));
            self.paint_preview();
            return;
        };
        let charset = self.charset;
        if path.file_name().and_then(|name| name.to_str()).is_some_and(imageview::is_image) {
            let tx = sender.input_sender().clone();
            let live = Arc::clone(&self.preview_live);
            live.store(generation, Ordering::Relaxed);
            let edge = if self.quick_fit { imageview::MAX_EDGE } else { u32::MAX };
            std::thread::spawn(move || {
                if live.load(Ordering::Relaxed) != generation {
                    return;
                }
                let body = preview_body(&path, edge);
                if live.load(Ordering::Relaxed) != generation {
                    return;
                }
                tx.emit(Msg::Preview { generation, body });
            });
            return;
        }
        let text = match lister::Reader::open(&path) {
            Ok(mut reader) => lister::render_reader(&mut reader, ViewMode::Text, charset).unwrap_or_else(|err| err.to_string()),
            Err(err) => err.to_string(),
        };
        self.preview.set_body(PreviewBody::Text(text));
    }

    pub(in crate::ui::app) fn paint_preview(&mut self) {
        if self.shell.editing.get() {
            return;
        }
        let pane = self.state.active.other().index();
        let ui = &self.shell.panes[pane];
        if let Some(rev) = show_slot(ui, &self.preview, self.preview_shown[pane], self.quick_fit) {
            self.preview_shown[pane] = rev;
        }
    }

    pub(in crate::ui::app) fn file_under_cursor(&self) -> Option<PathBuf> {
        let entry = self.state.cursor_entry(self.state.active)?;
        if entry.is_dir() {
            return None;
        }
        Some(comarchy::listing::entry_path(&self.state.tab(self.state.active).path, entry))
    }

    pub(in crate::ui::app) fn toggle_image_scale(&mut self, sender: &ComponentSender<Self>) {
        let Some(ui) = self.image.as_mut() else { return };
        ui.fit = !ui.fit;
        apply_image_scale(ui);
        self.load_image(sender);
    }

    pub(in crate::ui::app) fn toggle_shown_scale(&mut self, sender: &ComponentSender<Self>) {
        if self.image.is_some() {
            self.toggle_image_scale(sender);
        } else {
            self.toggle_quick_scale();
        }
    }

    pub(in crate::ui::app) fn toggle_quick_scale(&mut self) {
        if !self.cursor_is_image() {
            return;
        }
        self.quick_fit = !self.quick_fit;
        self.note_preview_target();
    }

    pub(in crate::ui::app) fn step_shown_image(&mut self, delta: i32, sender: &ComponentSender<Self>) {
        if self.image.is_some() {
            self.step_image(delta as isize, sender);
        } else {
            self.step_quick_image(delta as isize);
        }
    }

    pub(in crate::ui::app) fn cursor_is_image(&self) -> bool {
        self.state
            .cursor_entry(self.state.active)
            .is_some_and(|entry| !entry.is_dir() && imageview::is_image(&entry.name))
    }

    pub(in crate::ui::app) fn paint_mode_bar(&self) {
        let image = self.cfg.mode_buttons && self.lister.is_none() && (self.image.is_some() || (self.state.quick_view && self.cursor_is_image()));
        let text = self.cfg.mode_buttons && (self.lister.is_some() || (self.state.quick_view && self.cursor_is_file() && !self.cursor_is_image()));
        let page = if image { "image" } else if text { "text" } else { "files" };
        if self.shell.mode_bar.visible_child_name().as_deref() != Some(page) {
            self.shell.mode_bar.set_visible_child_name(page);
        }
        let fit = self.image.as_ref().map(|ui| ui.fit).unwrap_or(self.quick_fit);
        let label = if fit { "S Fit" } else { "S 1:1" };
        if self.shell.image_scale.label().as_deref() != Some(label) {
            self.shell.image_scale.set_label(label);
        }
        paint_charset(&self.shell.text_modes, self.charset);
        let quick_text = self.cfg.mode_buttons
            && self.lister.is_none()
            && self.state.quick_view
            && self.cursor_is_file()
            && !self.cursor_is_image();
        self.shell.edit_button.set_visible(quick_text);
        self.shell.save_button.set_visible(quick_text);
    }

    pub(in crate::ui::app) fn cursor_is_file(&self) -> bool {
        self.state.cursor_entry(self.state.active).is_some_and(|entry| !entry.is_dir())
    }
}

pub(in crate::ui::app) fn preview_body(path: &Path, max_edge: u32) -> PreviewBody {
    match imageview::decode(path, max_edge) {
        Ok(img) => PreviewBody::Image { width: img.width, height: img.height, pixels: img.pixels },
        Err(err) => PreviewBody::Text(err),
    }
}

fn paint_edit(button: &gtk::Button, on: bool) {
    button.set_label(if on { "E View" } else { "E Edit" });
    mark_button(button, on);
}

pub(in crate::ui::app) fn paint_charset(bar: &crate::ui::shell::CharsetBar, charset: Charset) {
    mark_button(&bar.ansi, charset == Charset::Ansi);
    mark_button(&bar.ascii, charset == Charset::Ascii);
    mark_button(&bar.json, charset == Charset::Json);
}

fn scroll_text(view: &gtk::TextView, delta: i64) {
    let Some(adj) = view.vadjustment() else { return };
    if delta == 0 {
        adj.set_value(adj.lower());
        return;
    }
    let page = adj.page_size();
    let step = if delta.abs() >= 4096 { page } else { (page / 16.0).max(16.0) };
    let sign = if delta < 0 { -1.0 } else { 1.0 };
    let max = (adj.upper() - page).max(adj.lower());
    adj.set_value((adj.value() + sign * step).clamp(adj.lower(), max));
}

pub(in crate::ui::app) fn mark_button(button: &gtk::Button, on: bool) {
    if on {
        button.add_css_class("hot-on");
    } else {
        button.remove_css_class("hot-on");
    }
}

pub(in crate::ui::app) fn apply_image_scale(ui: &ImageUi) {
    let fit = ui.fit;
    ui.picture.set_content_fit(if fit { gtk::ContentFit::Contain } else { gtk::ContentFit::ScaleDown });
    ui.picture.set_can_shrink(fit);
    ui.picture.set_hexpand(fit);
    ui.picture.set_vexpand(fit);
    ui.picture.set_halign(if fit { gtk::Align::Fill } else { gtk::Align::Center });
    ui.picture.set_valign(if fit { gtk::Align::Fill } else { gtk::Align::Center });
    if fit {
        ui.picture.set_size_request(-1, -1);
    }
    let policy = if fit { gtk::PolicyType::Never } else { gtk::PolicyType::Automatic };
    ui.scroll.set_policy(policy, policy);
    ui.scale.set_label(if fit { "S Fit" } else { "S 1:1" });
}

pub(in crate::ui::app) fn show_slot(ui: &crate::ui::shell::PaneUi, slot: &lister::PreviewSlot, shown: u64, fit: bool) -> Option<u64> {
    match &slot.body {
        PreviewBody::Text(text) => {
            if ui.preview_kind.visible_child_name().as_deref() != Some("text") {
                ui.preview_kind.set_visible_child_name("text");
            }
            let buf = ui.preview.buffer();
            let current = buf.text(&buf.start_iter(), &buf.end_iter(), false);
            if current.as_str() != text {
                buf.set_text(text);
                let mut start = buf.start_iter();
                ui.preview.scroll_to_iter(&mut start, 0.0, true, 0.0, 0.0);
            }
            None
        }
        PreviewBody::Image { width, height, pixels } => {
            if ui.preview_kind.visible_child_name().as_deref() != Some("image") {
                ui.preview_kind.set_visible_child_name("image");
            }
            ui.picture.set_content_fit(if fit { gtk::ContentFit::Contain } else { gtk::ContentFit::ScaleDown });
            ui.picture.set_can_shrink(fit);
            ui.picture.set_size_request(if fit { -1 } else { *width as i32 }, if fit { -1 } else { *height as i32 });
            let policy = if fit { gtk::PolicyType::Never } else { gtk::PolicyType::Automatic };
            ui.picture_scroll.set_policy(policy, policy);
            if shown == slot.rev {
                return None;
            }
            set_texture(&ui.picture, *width, *height, pixels);
            Some(slot.rev)
        }
    }
}

pub(in crate::ui::app) fn set_texture(picture: &gtk::Picture, width: u32, height: u32, pixels: &[u8]) {
    let stride = width as usize * 4;
    if height == 0 || stride == 0 || pixels.len() < stride * height as usize {
        return;
    }
    let bytes = gtk::glib::Bytes::from(pixels);
    let texture = gtk::gdk::MemoryTexture::new(
        width as i32,
        height as i32,
        gtk::gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        stride,
    );
    picture.set_paintable(Some(&texture));
}

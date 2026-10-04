use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use relm4::gtk;
use relm4::gtk::prelude::*;
use relm4::prelude::*;
use relm4::RelmWidgetExt;

use rust_commander::imageview;
use rust_commander::lister::{self, Charset, PreviewBody, ViewMode};
use rust_commander::state::PaneId;

use crate::ui::msg::Msg;

use super::{App, ImageUi, ListerUi, QuickEdit};

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
        crate::ui::shell::on_click(&scale, move || tx_scale.emit(Msg::ToggleImageScale));
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

    // The file is read on a thread, so a hung network mount cannot freeze the window.
    pub(in crate::ui::app) fn open_lister(&mut self, sender: &ComponentSender<Self>) {
        self.close_image();
        let Some(path) = self.file_under_cursor() else { return };
        self.lister_gen = self.lister_gen.wrapping_add(1);
        let generation = self.lister_gen;
        let charset = self.charset;
        let tx = sender.input_sender().clone();
        std::thread::spawn(move || {
            let opened = read_lister(&path, charset);
            tx.emit(Msg::ListerOpened { generation, opened });
        });
    }

    pub(in crate::ui::app) fn show_lister(
        &mut self,
        generation: u64,
        opened: Result<(lister::Reader, String), String>,
        sender: &ComponentSender<Self>,
    ) {
        if generation != self.lister_gen {
            return;
        }
        let (reader, text) = match opened {
            Ok(opened) => opened,
            Err(err) => {
                self.status = err;
                return;
            }
        };
        let path = reader.path().to_path_buf();
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
        if self.cfg.syntax_highlight && self.charset == Charset::Utf8 {
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
            paint_syntax(&view, name, true);
        }
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
            self.leave_quick_edit(sender);
            return;
        }
        if !self.state.quick_view || self.cursor_is_image() {
            self.note_quick("Edit is for text");
            return;
        }
        let Some(path) = self.file_under_cursor() else {
            self.note_quick("Edit is for text");
            return;
        };
        let (text, form) = match lister::read_for_edit(&path, self.charset) {
            Ok(read) => read,
            Err(err) => {
                self.note_quick(&err);
                return;
            }
        };
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("file").to_string();
        *self.edit_name.borrow_mut() = name.clone();
        self.shell.window.set_title(Some(&name));
        let pane = self.state.active.other().index();
        let preview = &self.shell.panes[pane].preview;
        self.edit_quiet.set(true);
        preview.buffer().set_text(&text);
        self.edit_quiet.set(false);
        paint_syntax(preview, &name, self.cfg.syntax_highlight);
        self.quick_edit = Some(QuickEdit { path, pane, form, saved: self.edit_changes.get(), armed: None });
        self.shell.editing.set(true);
        preview.set_editable(true);
        preview.set_cursor_visible(true);
        preview.grab_focus();
        paint_edit(&self.shell.edit_button, true);
        self.status = "QUICKVIEW: edit".into();
    }

    pub(in crate::ui::app) fn toggle_quick_view(&mut self, sender: &ComponentSender<Self>) {
        if self.shell.editing.get() && !self.leave_quick_edit(sender) {
            return;
        }
        self.state.panel_terminal = false;
        self.state.quick_view = !self.state.quick_view;
        if self.state.quick_view {
            self.status = "QUICKVIEW: view".into();
            self.note_preview_target();
            self.focus_list();
        }
    }

    pub(in crate::ui::app) fn quick_status_idle(&self) -> String {
        if self.shell.editing.get() {
            "QUICKVIEW: edit".into()
        } else if self.state.quick_view {
            "QUICKVIEW: view".into()
        } else {
            "QUICKVIEW".into()
        }
    }

    fn note_quick(&mut self, text: &str) {
        self.status = if self.state.quick_view {
            format!("QUICKVIEW: {text}")
        } else {
            text.into()
        };
    }

    // Returns false when unsaved text stopped the leave. The next try discards it.
    pub(in crate::ui::app) fn leave_quick_edit(&mut self, sender: &ComponentSender<Self>) -> bool {
        let changes = self.edit_changes.get();
        if let Some(edit) = self.quick_edit.as_mut() {
            if edit.saved != changes && edit.armed != Some(changes) {
                edit.armed = Some(changes);
                self.note_quick(&format!(
                    "Unsaved changes: {} saves, {} discards, or press again",
                    rust_commander::commands::format_keys("Ctrl+S"),
                    rust_commander::commands::format_keys("Ctrl+D"),
                ));
                return false;
            }
        }
        self.stop_quick_edit(true, sender);
        true
    }

    pub(in crate::ui::app) fn stop_quick_edit(&mut self, reload: bool, sender: &ComponentSender<Self>) {
        if !self.shell.editing.get() {
            return;
        }
        self.shell.editing.set(false);
        self.quick_edit = None;
        for pane in &self.shell.panes {
            pane.preview.set_editable(false);
        }
        paint_edit(&self.shell.edit_button, false);
        self.shell.window.set_title(Some(rust_commander::app_title()));
        if reload {
            self.note_preview_target();
            self.kick_preview(sender);
        }
        if self.state.quick_view {
            self.status = "QUICKVIEW: view".into();
        }
        self.focus_list();
    }

    pub(in crate::ui::app) fn discard_quick(&mut self, sender: &ComponentSender<Self>) {
        if !self.shell.editing.get() {
            return;
        }
        let dirty = self.quick_edit.as_ref().is_some_and(|edit| edit.saved != self.edit_changes.get());
        self.stop_quick_edit(true, sender);
        if dirty {
            self.note_quick("Discarded");
        }
    }

    pub(in crate::ui::app) fn save_quick(&mut self) {
        if !self.shell.editing.get() {
            return;
        }
        let Some(edit) = self.quick_edit.as_ref() else { return };
        let text = {
            let buffer = self.shell.panes[edit.pane].preview.buffer();
            buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string()
        };
        match lister::write_text(&edit.path, &text, edit.form) {
            Ok(()) => {
                let changes = self.edit_changes.get();
                if let Some(edit) = self.quick_edit.as_mut() {
                    edit.saved = changes;
                    edit.armed = None;
                }
                self.shell.window.set_title(Some(&self.edit_name.borrow()));
                self.status = "QUICKVIEW: SAVED".into();
            }
            Err(err) => self.note_quick(&err.to_string()),
        }
    }

    pub(in crate::ui::app) fn lister_set_mode(&mut self, mode: ViewMode, sender: &ComponentSender<Self>) {
        if let Some(lister) = self.lister.as_mut() {
            lister.mode = mode;
        }
        self.lister_redraw(sender);
    }

    pub(in crate::ui::app) fn lister_scroll(&mut self, delta: i64, sender: &ComponentSender<Self>) {
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
        self.lister_redraw(sender);
    }

    pub(in crate::ui::app) fn lister_redraw(&mut self, sender: &ComponentSender<Self>) {
        let charset = self.charset;
        let Some(lister) = self.lister.as_ref() else { return };
        let mode = lister.mode;
        let mut reader = lister.reader.clone();
        self.lister_draw = self.lister_draw.wrapping_add(1);
        let generation = self.lister_draw;
        let tx = sender.input_sender().clone();
        std::thread::spawn(move || {
            if let Ok(text) = lister::render_reader(&mut reader, mode, charset) {
                tx.emit(Msg::ListerText { generation, text });
            }
        });
    }

    pub(in crate::ui::app) fn show_lister_text(&mut self, generation: u64, text: String) {
        if generation != self.lister_draw {
            return;
        }
        let Some(lister) = self.lister.as_ref() else { return };
        lister.view.buffer().set_text(&text);
        self.paint_lister_syntax();
    }

    pub(in crate::ui::app) fn paint_lister_syntax(&self) {
        let Some(lister) = self.lister.as_ref() else { return };
        let on = self.cfg.syntax_highlight && lister.mode == ViewMode::Text && self.charset == Charset::Utf8;
        paint_syntax(&lister.view, lister.reader.file_name(), on);
    }

    pub(in crate::ui::app) fn set_charset(&mut self, charset: Charset, sender: &ComponentSender<Self>) {
        if self.shell.editing.get() {
            self.note_quick("Leave edit first");
            return;
        }
        self.charset = charset;
        let name = match charset {
            Charset::Utf8 => "UTF-8",
            Charset::Ansi => "ANSI",
            Charset::Ascii => "ASCII",
            Charset::Json => "JSON",
        };
        self.note_quick(name);
        self.lister_redraw(sender);
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
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
        if let Some(art) = rust_commander::icons::art_for(name) {
            let palette = rust_commander::theme::load_named(&rust_commander::theme::active_name(
                &self.cfg.theme,
                &rust_commander::config::active_theme_path(),
            ));
            let svg = rust_commander::icons::art_svg(art, &palette);
            let stack = &self.shell.panes[self.state.active.other().index()].stack;
            let edge = stack.width().min(stack.height()).clamp(64, 512) as u32;
            let body = imageview::render_svg(&svg, edge)
                .map_or_else(PreviewBody::Text, |img| PreviewBody::Image { width: img.width, height: img.height, pixels: img.pixels });
            self.preview.set_body(body);
            self.paint_preview();
            return;
        }
        if imageview::is_image(name) {
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
        let tx = sender.input_sender().clone();
        let live = Arc::clone(&self.preview_live);
        live.store(generation, Ordering::Relaxed);
        std::thread::spawn(move || {
            if live.load(Ordering::Relaxed) != generation {
                return;
            }
            let text = match lister::Reader::open(&path) {
                Ok(mut reader) => lister::render_reader(&mut reader, ViewMode::Text, charset).unwrap_or_else(|err| err.to_string()),
                Err(err) => err.to_string(),
            };
            if live.load(Ordering::Relaxed) == generation {
                tx.emit(Msg::Preview { generation, body: PreviewBody::Text(text) });
            }
        });
    }

    pub(in crate::ui::app) fn paint_preview(&mut self) {
        if self.shell.editing.get() {
            return;
        }
        let pane = self.state.active.other().index();
        let ui = &self.shell.panes[pane];
        let (syntax, name) = self.preview_syntax();
        if let Some(rev) = show_slot(ui, &self.preview, self.preview_shown[pane], self.quick_fit, syntax, &name) {
            self.preview_shown[pane] = rev;
        }
    }

    pub(in crate::ui::app) fn preview_syntax(&self) -> (bool, String) {
        let name = self
            .file_under_cursor()
            .and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()))
            .unwrap_or_default();
        (self.cfg.syntax_highlight && self.charset == Charset::Utf8, name)
    }

    pub(in crate::ui::app) fn file_under_cursor(&self) -> Option<PathBuf> {
        let entry = self.state.cursor_entry(self.state.active)?;
        if entry.is_dir() {
            return None;
        }
        Some(rust_commander::listing::entry_path(&self.state.tab(self.state.active).path, entry))
    }

    pub(in crate::ui::app) fn preview_image_focused(&self) -> bool {
        if !self.state.quick_view || self.shell.editing.get() {
            return false;
        }
        let ui = &self.shell.panes[self.state.active.other().index()];
        ui.preview_kind.visible_child_name().as_deref() == Some("image") && ui.picture.has_focus()
    }

    pub(in crate::ui::app) fn copy_preview_path(&mut self) {
        let Some(path) = self.file_under_cursor() else { return };
        let Some(display) = gtk::gdk::Display::default() else { return };
        display.clipboard().set_text(&path.display().to_string());
        self.status = "Path copied".into();
    }

    pub(in crate::ui::app) fn copy_preview_image(&mut self, sender: &ComponentSender<Self>) {
        let Some(path) = self.file_under_cursor() else { return };
        if !path.file_name().and_then(|name| name.to_str()).is_some_and(imageview::is_image) {
            return;
        }
        self.copy_gen = self.copy_gen.wrapping_add(1);
        let generation = self.copy_gen;
        let tx = sender.input_sender().clone();
        std::thread::spawn(move || {
            let image = imageview::decode(&path, u32::MAX).map(|rgba| (rgba.width, rgba.height, rgba.pixels));
            tx.emit(Msg::PreviewImageCopied { generation, image });
        });
    }

    pub(in crate::ui::app) fn finish_image_copy(&mut self, generation: u64, image: Result<(u32, u32, Vec<u8>), String>) {
        if generation != self.copy_gen {
            return;
        }
        let (width, height, pixels) = match image {
            Ok(image) => image,
            Err(err) => {
                self.status = err;
                return;
            }
        };
        let stride = width as usize * 4;
        if height == 0 || stride == 0 || pixels.len() < stride * height as usize {
            self.status = "Could not copy image".into();
            return;
        }
        let Some(display) = gtk::gdk::Display::default() else { return };
        let bytes = gtk::glib::Bytes::from(&pixels);
        let texture = gtk::gdk::MemoryTexture::new(width as i32, height as i32, gtk::gdk::MemoryFormat::R8g8b8a8, &bytes, stride);
        display.clipboard().set_texture(&texture);
        self.status = "Image copied".into();
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

    pub(in crate::ui::app) fn cursor_is_media(&self) -> bool {
        self.state
            .cursor_entry(self.state.active)
            .is_some_and(|entry| !entry.is_dir() && rust_commander::listing::is_icon_preview(&entry.name))
    }

    pub(in crate::ui::app) fn paint_mode_bar(&self) {
        let image = self.cfg.mode_buttons && self.lister.is_none() && (self.image.is_some() || (self.state.quick_view && self.cursor_is_image()));
        let text = self.cfg.mode_buttons && (self.lister.is_some() || (self.state.quick_view && self.cursor_is_file() && !self.cursor_is_image() && !self.cursor_is_media()));
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
            && !self.cursor_is_image()
            && !self.cursor_is_media();
        self.shell.edit_button.set_visible(quick_text);
        self.shell.save_button.set_visible(quick_text);
        self.shell.discard_button.set_visible(quick_text && self.shell.editing.get());
    }

    pub(in crate::ui::app) fn cursor_is_file(&self) -> bool {
        self.state.cursor_entry(self.state.active).is_some_and(|entry| !entry.is_dir())
    }
}

pub(in crate::ui::app) fn read_lister(path: &Path, charset: Charset) -> Result<(lister::Reader, String), String> {
    let cannot = || format!("Cannot open {}", path.display());
    let mut reader = lister::Reader::open(path).map_err(|_| cannot())?;
    let text = lister::render_reader(&mut reader, ViewMode::Text, charset).map_err(|_| cannot())?;
    Ok((reader, text))
}

pub(in crate::ui::app) fn preview_body(path: &Path, max_edge: u32) -> PreviewBody {
    match imageview::decode(path, max_edge) {
        Ok(img) => PreviewBody::Image { width: img.width, height: img.height, pixels: img.pixels },
        Err(err) => PreviewBody::Text(err),
    }
}

fn paint_edit(button: &gtk::Button, on: bool) {
    button.set_label(&if on {
        rust_commander::commands::caption("Ctrl+V", "View")
    } else {
        rust_commander::commands::caption("Ctrl+E", "Edit")
    });
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

pub(in crate::ui::app) fn show_slot(ui: &crate::ui::shell::PaneUi, slot: &lister::PreviewSlot, shown: u64, fit: bool, syntax: bool, name: &str) -> Option<u64> {
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
            paint_syntax(&ui.preview, name, syntax);
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

pub(in crate::ui::app) fn paint_syntax(view: &gtk::TextView, name: &str, on: bool) {
    let buf = view.buffer();
    let start = buf.start_iter();
    let end = buf.end_iter();
    buf.remove_all_tags(&start, &end);
    if !on {
        return;
    }
    let text = buf.text(&buf.start_iter(), &buf.end_iter(), false);
    let spans = rust_commander::syntax::highlight(name, text.as_str());
    if spans.is_empty() {
        return;
    }
    for (tag, color) in [("syn-comment", "#565f89"), ("syn-string", "#9ece6a"), ("syn-keyword", "#7aa2f7")] {
        if buf.tag_table().lookup(tag).is_none() {
            let mark = gtk::TextTag::new(Some(tag));
            mark.set_foreground(Some(color));
            buf.tag_table().add(&mark);
        }
    }
    for span in spans {
        let start = buf.iter_at_offset(span.start as i32);
        let end = buf.iter_at_offset(span.end as i32);
        let tag = match span.kind {
            rust_commander::syntax::Kind::Comment => "syn-comment",
            rust_commander::syntax::Kind::String => "syn-string",
            rust_commander::syntax::Kind::Keyword => "syn-keyword",
        };
        buf.apply_tag_by_name(tag, &start, &end);
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

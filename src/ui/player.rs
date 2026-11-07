use std::cell::RefCell;

use relm4::gtk;
use relm4::gtk::prelude::*;

use super::msg::Msg;

/// Everything the player pane shows, so painting is one call and a repeat costs nothing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Look {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub detail: String,
    /// An error, or what is happening while nothing plays.
    pub message: String,
    pub playing: bool,
    /// Play and pause have something to act on.
    pub can_toggle: bool,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    pub volume: f32,
    pub muted: bool,
}

pub struct PlayerView {
    pub root: gtk::Box,
    art: gtk::Stack,
    cover: gtk::Picture,
    title: gtk::Label,
    artist: gtk::Label,
    album: gtk::Label,
    detail: gtk::Label,
    elapsed: gtk::Label,
    total: gtk::Label,
    seek: gtk::Scale,
    toggle: gtk::Button,
    mute: gtk::Button,
    volume: gtk::Scale,
    message: gtk::Label,
    hint: gtk::Label,
    shown: RefCell<Look>,
    cover_rev: RefCell<u64>,
}

fn label(class: &str) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.add_css_class(class);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_justify(gtk::Justification::Center);
    label.set_selectable(false);
    label.set_focusable(false);
    label
}

fn scale(max: f64) -> gtk::Scale {
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, max, 1.0);
    scale.set_draw_value(false);
    scale.set_hexpand(true);
    // The list keeps the keyboard, so Space and the arrows keep working after a click here.
    scale.set_focusable(false);
    scale.set_focus_on_click(false);
    scale
}

fn button(text: &str) -> gtk::Button {
    let button = gtk::Button::with_label(text);
    button.add_css_class("audio-button");
    button.set_focusable(false);
    button.set_focus_on_click(false);
    button
}

impl PlayerView {
    pub fn new(sender: &relm4::Sender<Msg>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
        root.add_css_class("audio-player");
        root.set_hexpand(true);
        root.set_vexpand(true);

        let cover = gtk::Picture::new();
        cover.set_content_fit(gtk::ContentFit::Contain);
        cover.set_can_shrink(true);
        cover.set_hexpand(true);
        cover.set_vexpand(true);
        let note = gtk::Label::new(Some("\u{266a}"));
        note.add_css_class("audio-note");
        note.set_hexpand(true);
        note.set_vexpand(true);
        let art = gtk::Stack::new();
        art.set_vexpand(true);
        art.set_hexpand(true);
        art.add_named(&cover, Some("cover"));
        art.add_named(&note, Some("note"));
        art.set_visible_child_name("note");

        let title = label("audio-title");
        let artist = label("audio-artist");
        let album = label("audio-album");
        let detail = label("audio-detail");
        let meta = gtk::Box::new(gtk::Orientation::Vertical, 2);
        meta.set_halign(gtk::Align::Fill);
        for widget in [&title, &artist, &album, &detail] {
            meta.append(widget);
        }

        let elapsed = label("audio-time");
        let total = label("audio-time");
        for time in [&elapsed, &total] {
            time.set_width_chars(5);
            time.set_wrap(false);
        }
        elapsed.set_xalign(1.0);
        total.set_xalign(0.0);
        let seek = scale(1000.0);
        let tx = sender.clone();
        seek.connect_change_value(move |_, _, value| {
            tx.emit(Msg::AudioSeek((value / 1000.0).clamp(0.0, 1.0)));
            gtk::glib::Propagation::Proceed
        });
        let progress = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        progress.append(&elapsed);
        progress.append(&seek);
        progress.append(&total);

        let toggle = button("Pause");
        toggle.set_width_request(96);
        let tx = sender.clone();
        toggle.connect_clicked(move |_| tx.emit(Msg::AudioToggle));
        let mute = button("Vol");
        let tx = sender.clone();
        mute.connect_clicked(move |_| tx.emit(Msg::AudioMute));
        let volume = scale(100.0);
        volume.set_hexpand(false);
        volume.set_width_request(130);
        let tx = sender.clone();
        volume.connect_change_value(move |_, _, value| {
            tx.emit(Msg::AudioVolume((value / 100.0).clamp(0.0, 1.0)));
            gtk::glib::Propagation::Proceed
        });
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        controls.append(&toggle);
        controls.append(&spacer);
        controls.append(&mute);
        controls.append(&volume);

        let message = label("audio-message");
        let hint = label("audio-hint");
        hint.set_text("Space pause   \u{2190}/\u{2192} seek   +/\u{2212} volume   M mute   Enter open elsewhere");

        root.append(&art);
        root.append(&meta);
        root.append(&progress);
        root.append(&controls);
        root.append(&message);
        root.append(&hint);

        Self {
            root,
            art,
            cover,
            title,
            artist,
            album,
            detail,
            elapsed,
            total,
            seek,
            toggle,
            mute,
            volume,
            message,
            hint,
            shown: RefCell::new(Look { volume: -1.0, ..Look::default() }),
            cover_rev: RefCell::new(0),
        }
    }

    /// What the pane shows now.
    pub fn looks(&self) -> Look {
        self.shown.borrow().clone()
    }

    /// Shows or clears the cover. `rev` tells a repeat of the same picture from a new one.
    pub fn set_cover(&self, rev: u64, image: Option<(u32, u32, &[u8])>) {
        if *self.cover_rev.borrow() == rev {
            return;
        }
        *self.cover_rev.borrow_mut() = rev;
        match image {
            Some((width, height, pixels)) if pixels.len() >= width as usize * height as usize * 4 && width > 0 && height > 0 => {
                let bytes = gtk::glib::Bytes::from(pixels);
                let texture = gtk::gdk::MemoryTexture::new(width as i32, height as i32, gtk::gdk::MemoryFormat::R8g8b8a8, &bytes, width as usize * 4);
                self.cover.set_paintable(Some(&texture));
                self.art.set_visible_child_name("cover");
            }
            _ => {
                self.cover.set_paintable(None::<&gtk::gdk::Paintable>);
                self.art.set_visible_child_name("note");
            }
        }
    }

    pub fn paint(&self, look: &Look) {
        let mut shown = self.shown.borrow_mut();
        if *shown == *look {
            return;
        }
        set_text(&self.title, &shown.title, &look.title);
        set_text(&self.artist, &shown.artist, &look.artist);
        set_text(&self.album, &shown.album, &look.album);
        set_text(&self.detail, &shown.detail, &look.detail);
        set_text(&self.message, &shown.message, &look.message);
        // The key help is for a track that plays. A message takes its place.
        self.hint.set_visible(look.message.is_empty());
        // Labels with no text take no room, so the layout does not leave gaps.
        for (widget, text) in [
            (&self.artist, &look.artist),
            (&self.album, &look.album),
            (&self.detail, &look.detail),
            (&self.message, &look.message),
        ] {
            widget.set_visible(!text.is_empty());
        }
        let name = if look.playing { "Pause" } else { "Play" };
        if self.toggle.label().as_deref() != Some(name) {
            self.toggle.set_label(name);
        }
        if self.toggle.is_sensitive() != look.can_toggle {
            self.toggle.set_sensitive(look.can_toggle);
        }
        let elapsed = clock_text(look.position_ms);
        if self.elapsed.text().as_str() != elapsed {
            self.elapsed.set_text(&elapsed);
        }
        let total = look.duration_ms.map_or_else(|| "--:--".to_string(), clock_text);
        if self.total.text().as_str() != total {
            self.total.set_text(&total);
        }
        match look.duration_ms {
            Some(total) if total > 0 => {
                self.seek.set_sensitive(look.can_toggle);
                let at = (look.position_ms as f64 / total as f64 * 1000.0).clamp(0.0, 1000.0);
                if (self.seek.value() - at).abs() > 0.4 {
                    self.seek.set_value(at);
                }
            }
            _ => {
                self.seek.set_sensitive(false);
                self.seek.set_value(0.0);
            }
        }
        if shown.volume != look.volume || shown.muted != look.muted {
            let shown_volume = if look.muted { 0.0 } else { f64::from(look.volume) * 100.0 };
            if (self.volume.value() - shown_volume).abs() > 0.4 {
                self.volume.set_value(shown_volume);
            }
            self.mute.set_label(if look.muted { "Muted" } else { "Vol" });
        }
        *shown = look.clone();
    }
}

fn set_text(label: &gtk::Label, old: &str, new: &str) {
    if old != new || label.text().as_str() != new {
        label.set_text(new);
    }
}

fn clock_text(ms: u64) -> String {
    rust_commander::audio::clock(std::time::Duration::from_millis(ms))
}

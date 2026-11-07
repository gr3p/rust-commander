use std::cell::{Cell, RefCell};

use relm4::gtk;
use relm4::gtk::prelude::*;

use super::media::{Icon, IconButton};
use super::msg::Msg;
use super::player::{Look, Transport, cover_texture, paint_progress, paint_volume, progress_row, set_text, volume_row};

/// The small player docked under a file list. It plays while both panels work as usual.
pub struct MiniPlayer {
    pub root: gtk::Box,
    art: gtk::Stack,
    cover: gtk::Image,
    title: gtk::Label,
    artist: gtk::Label,
    transport: Transport,
    elapsed: gtk::Label,
    total: gtk::Label,
    seek: gtk::Scale,
    speaker: IconButton,
    volume: gtk::Scale,
    shown: RefCell<Look>,
    cover_rev: Cell<u64>,
}

fn line(class: &str) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.add_css_class(class);
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_width_chars(4);
    label.set_max_width_chars(40);
    label.set_selectable(false);
    label.set_focusable(false);
    label
}

impl MiniPlayer {
    pub fn new(sender: &relm4::Sender<Msg>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        root.add_css_class("audio-mini");
        root.set_visible(false);

        // A picture would ask for the size of the cover, an image keeps to its pixel size.
        let cover = gtk::Image::new();
        cover.set_pixel_size(44);
        let note = gtk::Label::new(Some("\u{266a}"));
        note.add_css_class("audio-mini-note");
        let art = gtk::Stack::new();
        art.set_size_request(44, 44);
        art.set_valign(gtk::Align::Center);
        art.set_overflow(gtk::Overflow::Hidden);
        art.add_css_class("audio-mini-art");
        art.add_named(&cover, Some("cover"));
        art.add_named(&note, Some("note"));
        art.set_visible_child_name("note");

        let title = line("audio-mini-title");
        let artist = line("audio-mini-artist");
        let meta = gtk::Box::new(gtk::Orientation::Vertical, 0);
        meta.set_valign(gtk::Align::Center);
        meta.set_hexpand(true);
        meta.append(&title);
        meta.append(&artist);

        let transport = Transport::new(sender, 16, 22);
        transport.row.set_valign(gtk::Align::Center);
        let (progress, elapsed, seek, total) = progress_row(sender);
        progress.set_hexpand(true);
        let (volume_box, speaker, volume) = volume_row(sender, 70);

        let expand = IconButton::new(Icon::Expand, 16, "media-quiet");
        expand.set_tooltip("Show the full player (Ctrl+M)");
        let tx = sender.clone();
        expand.button.connect_clicked(move |_| tx.emit(Msg::AudioExpand));
        let close = IconButton::new(Icon::Close, 14, "media-quiet");
        close.set_tooltip("Stop and close the player");
        let tx = sender.clone();
        close.button.connect_clicked(move |_| tx.emit(Msg::AudioClose));
        let ends = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        ends.set_valign(gtk::Align::Center);
        ends.append(&expand.button);
        ends.append(&close.button);

        // Two rows keep it narrow enough for a half-width list: who plays and the buttons above, time and volume below.
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        top.append(&art);
        top.append(&meta);
        top.append(&transport.row);
        top.append(&ends);
        let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        bottom.append(&progress);
        bottom.append(&volume_box);
        root.append(&top);
        root.append(&bottom);

        Self {
            root,
            art,
            cover,
            title,
            artist,
            transport,
            elapsed,
            total,
            seek,
            speaker,
            volume,
            shown: RefCell::new(Look { volume: -1.0, ..Look::default() }),
            cover_rev: Cell::new(0),
        }
    }

    pub fn set_visible(&self, visible: bool) {
        if self.root.is_visible() != visible {
            self.root.set_visible(visible);
        }
    }

    /// The keyboard drives the player: its bar is outlined in the accent color.
    pub fn set_focused(&self, focused: bool) {
        if self.root.has_css_class("focused") != focused {
            if focused {
                self.root.add_css_class("focused");
            } else {
                self.root.remove_css_class("focused");
            }
        }
    }

    pub fn looks(&self) -> Look {
        self.shown.borrow().clone()
    }

    pub fn set_cover(&self, rev: u64, image: Option<(u32, u32, &[u8])>) {
        if self.cover_rev.get() == rev {
            return;
        }
        self.cover_rev.set(rev);
        let texture = cover_texture(image);
        self.cover.set_paintable(texture.as_ref());
        self.art.set_visible_child_name(if texture.is_some() { "cover" } else { "note" });
    }

    pub fn paint(&self, look: &Look) {
        let mut shown = self.shown.borrow_mut();
        if *shown == *look {
            return;
        }
        set_text(&self.title, &look.title);
        let line = if look.message.is_empty() { &look.artist } else { &look.message };
        set_text(&self.artist, line);
        self.artist.set_visible(!line.is_empty());
        self.transport.paint(look);
        paint_progress(&self.elapsed, &self.total, &self.seek, look);
        paint_volume(&self.speaker, &self.volume, look);
        *shown = look.clone();
    }
}

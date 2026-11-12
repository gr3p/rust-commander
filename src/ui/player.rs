use std::cell::{Cell, RefCell};
use std::rc::Rc;

use relm4::gtk;
use relm4::gtk::prelude::*;

use rust_commander::audio::{Repeat, Scope, VisualStyle};

use super::media::{Icon, IconButton, IconMenu};
use super::msg::Msg;
use super::poster::Poster;
use super::visual::Visualizer;

/// Everything the player shows, so painting is one call and a repeat costs nothing.
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
    /// There is a track before or after this one.
    pub can_skip: bool,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    pub volume: f32,
    pub muted: bool,
    pub shuffle: bool,
    pub repeat: Repeat,
    pub visual_on: bool,
    pub visual: VisualStyle,
    /// How hard the sound moves the picture, 0 to 100. 50 is the original.
    pub strength: u8,
    /// How slowly the picture falls back, 0 to 100. 50 is the original.
    pub smooth: u8,
    /// How visible the picture is behind the text, 0 to 100. 50 is the original.
    pub density: u8,
}

fn click(button: &IconButton, sender: &relm4::Sender<Msg>, make: fn() -> Msg) {
    let tx = sender.clone();
    button.button.connect_clicked(move |_| tx.emit(make()));
}

/// The controls both players share: shuffle, previous, play, next and repeat.
pub struct Transport {
    pub row: gtk::Box,
    shuffle: IconButton,
    prev: IconButton,
    play: IconButton,
    next: IconButton,
    repeat: IconButton,
}

impl Transport {
    pub fn new(sender: &relm4::Sender<Msg>, side: i32, play_side: i32, gap: i32) -> Self {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, gap);
        row.set_halign(gtk::Align::Center);
        let shuffle = IconButton::new(Icon::Shuffle, side, "media-aux");
        shuffle.set_tooltip("Shuffle (S)");
        let prev = IconButton::new(Icon::Previous, side, "media-step");
        prev.set_tooltip("Previous (P)");
        let play = IconButton::new(Icon::Play, play_side, "media-play");
        play.set_tooltip("Play or pause (Space)");
        let next = IconButton::new(Icon::Next, side, "media-step");
        next.set_tooltip("Next (N)");
        let repeat = IconButton::new(Icon::Repeat, side, "media-aux");
        repeat.set_tooltip("Repeat: off (R)");
        click(&shuffle, sender, || Msg::AudioShuffle);
        click(&prev, sender, || Msg::AudioPrev);
        click(&play, sender, || Msg::AudioToggle);
        click(&next, sender, || Msg::AudioNext);
        click(&repeat, sender, || Msg::AudioRepeat);
        for widget in [&shuffle, &prev, &play, &next, &repeat] {
            row.append(&widget.button);
        }
        Self { row, shuffle, prev, play, next, repeat }
    }

    pub fn paint(&self, look: &Look) {
        self.play.set_icon(if look.playing { Icon::Pause } else { Icon::Play });
        if self.play.button.is_sensitive() != look.can_toggle {
            self.play.button.set_sensitive(look.can_toggle);
        }
        for step in [&self.prev, &self.next] {
            if step.button.is_sensitive() != look.can_skip {
                step.button.set_sensitive(look.can_skip);
            }
        }
        self.shuffle.set_on(look.shuffle);
        self.repeat.set_on(look.repeat != Repeat::Off);
        self.repeat.set_icon(if look.repeat == Repeat::One { Icon::RepeatOne } else { Icon::Repeat });
        self.repeat.set_tooltip(&format!("Repeat: {} (R)", look.repeat.name()));
    }
}

pub fn clock_text(ms: u64) -> String {
    rust_commander::audio::clock(std::time::Duration::from_millis(ms))
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

pub fn scale(max: f64) -> gtk::Scale {
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, max, 1.0);
    scale.set_draw_value(false);
    scale.set_hexpand(true);
    // The list keeps the keyboard, so Space and the arrows keep working after a click here.
    scale.set_focusable(false);
    scale.set_focus_on_click(false);
    scale
}

/// The progress bar with elapsed and total time around it. `chars` is the room for each time and
/// `line` the least width of the bar.
pub fn progress_row(sender: &relm4::Sender<Msg>, chars: i32, line: i32) -> (gtk::Box, gtk::Label, gtk::Scale, gtk::Label) {
    let elapsed = label("audio-time");
    let total = label("audio-time");
    for time in [&elapsed, &total] {
        time.set_width_chars(chars);
        time.set_wrap(false);
    }
    elapsed.set_xalign(1.0);
    total.set_xalign(0.0);
    let seek = scale(1000.0);
    seek.set_size_request(line, -1);
    let tx = sender.clone();
    seek.connect_change_value(move |_, _, value| {
        tx.emit(Msg::AudioSeek((value / 1000.0).clamp(0.0, 1.0)));
        gtk::glib::Propagation::Proceed
    });
    let row = gtk::Box::new(gtk::Orientation::Horizontal, if chars < 5 { 4 } else { 8 });
    row.append(&elapsed);
    row.append(&seek);
    row.append(&total);
    (row, elapsed, seek, total)
}

/// The speaker button and the volume slider.
pub fn volume_row(sender: &relm4::Sender<Msg>, width: i32) -> (gtk::Box, IconButton, gtk::Scale) {
    let speaker = IconButton::new(Icon::Speaker, 18, "media-aux");
    speaker.set_tooltip("Mute (M)");
    click(&speaker, sender, || Msg::AudioMute);
    let volume = scale(100.0);
    volume.set_hexpand(false);
    volume.set_width_request(width);
    let tx = sender.clone();
    volume.connect_change_value(move |_, _, value| {
        tx.emit(Msg::AudioVolume((value / 100.0).clamp(0.0, 1.0)));
        gtk::glib::Propagation::Proceed
    });
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    row.append(&speaker.button);
    row.append(&volume);
    (row, speaker, volume)
}

pub fn paint_progress(elapsed: &gtk::Label, total: &gtk::Label, seek: &gtk::Scale, look: &Look) {
    let text = clock_text(look.position_ms);
    if elapsed.text().as_str() != text {
        elapsed.set_text(&text);
    }
    let text = look.duration_ms.map_or_else(|| "--:--".to_string(), clock_text);
    if total.text().as_str() != text {
        total.set_text(&text);
    }
    match look.duration_ms {
        Some(length) if length > 0 => {
            seek.set_sensitive(look.can_toggle);
            let at = (look.position_ms as f64 / length as f64 * 1000.0).clamp(0.0, 1000.0);
            if (seek.value() - at).abs() > 0.4 {
                seek.set_value(at);
            }
        }
        _ => {
            seek.set_sensitive(false);
            seek.set_value(0.0);
        }
    }
}

pub fn paint_volume(speaker: &IconButton, volume: &gtk::Scale, look: &Look) {
    let shown = if look.muted { 0.0 } else { f64::from(look.volume) * 100.0 };
    if (volume.value() - shown).abs() > 0.4 {
        volume.set_value(shown);
    }
    speaker.set_icon(if look.muted || look.volume <= 0.0 { Icon::SpeakerMuted } else { Icon::Speaker });
}

pub fn set_text(label: &gtk::Label, new: &str) {
    if label.text().as_str() != new {
        label.set_text(new);
    }
}

/// A texture from decoded RGBA pixels, if they are enough for the size.
pub fn cover_texture(image: Option<(u32, u32, &[u8])>) -> Option<gtk::gdk::MemoryTexture> {
    let (width, height, pixels) = image?;
    if width == 0 || height == 0 || pixels.len() < width as usize * height as usize * 4 {
        return None;
    }
    let bytes = gtk::glib::Bytes::from(pixels);
    Some(gtk::gdk::MemoryTexture::new(width as i32, height as i32, gtk::gdk::MemoryFormat::R8g8b8a8, &bytes, width as usize * 4))
}

/// Shows or clears a cover in a picture. True if there is a cover to show.
pub fn show_cover(picture: &gtk::Picture, image: Option<(u32, u32, &[u8])>) -> bool {
    let texture = cover_texture(image);
    picture.set_paintable(texture.as_ref());
    texture.is_some()
}

pub struct PlayerView {
    pub root: gtk::Overlay,
    visual: Visualizer,
    art: gtk::Stack,
    cover: gtk::Picture,
    title: gtk::Label,
    artist: gtk::Label,
    album: gtk::Label,
    detail: gtk::Label,
    elapsed: gtk::Label,
    total: gtk::Label,
    seek: gtk::Scale,
    transport: Transport,
    speaker: IconButton,
    volume: gtk::Scale,
    bars: IconButton,
    message: gtk::Label,
    hint: gtk::Label,
    visual_check: gtk::CheckButton,
    style_checks: Vec<(VisualStyle, gtk::CheckButton)>,
    strength: gtk::Scale,
    smooth: gtk::Scale,
    density: gtk::Scale,
    // Set while the view itself changes a control, so that does not come back as a click.
    painting: Rc<Cell<bool>>,
    visual_on: Rc<Cell<bool>>,
    shown: RefCell<Look>,
    cover_rev: Cell<u64>,
}

impl PlayerView {
    pub fn new(sender: &relm4::Sender<Msg>) -> Self {
        let visual = Visualizer::new(VisualStyle::default(), true);
        let painting = Rc::new(Cell::new(false));
        let visual_on = Rc::new(Cell::new(true));

        // The small buttons in the corner: the visualizer switch, its settings, and fold-away.
        let (popover, visual_check, style_checks, strength, smooth, density) = settings_popover(sender, &painting);
        let bars = IconButton::new(Icon::Bars, 16, "media-quiet");
        bars.set_tooltip("Show or hide the visualizer");
        let (tx, state) = (sender.clone(), Rc::clone(&visual_on));
        bars.button.connect_clicked(move |_| tx.emit(Msg::AudioVisualOn(!state.get())));
        let gear = IconMenu::new(Icon::Gear, 16, "media-quiet", &popover);
        gear.set_tooltip("Player settings");
        let minimize = IconButton::new(Icon::Minimize, 16, "media-quiet");
        minimize.set_tooltip("Minimize: keep playing in a small bar under the file list");
        click(&minimize, sender, || Msg::AudioMinimize);
        let close = IconButton::new(Icon::Close, 16, "media-quiet");
        close.set_tooltip("Stop and close the player (Ctrl+Shift+Q)");
        click(&close, sender, || Msg::AudioClose);
        let corner = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        corner.set_halign(gtk::Align::End);
        corner.append(&bars.button);
        corner.append(&gear.button);
        corner.append(&minimize.button);
        corner.append(&close.button);

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
        art.add_named(&cover, Some("cover"));
        art.add_named(&note, Some("note"));
        art.set_visible_child_name("note");
        // The cover keeps to a size the window sets, however much room the pane has.
        let poster = Poster::new(&art);

        let title = label("audio-title");
        let artist = label("audio-artist");
        let album = label("audio-album");
        let detail = label("audio-detail");
        let meta = gtk::Box::new(gtk::Orientation::Vertical, 2);
        for widget in [&title, &artist, &album, &detail] {
            meta.append(widget);
        }

        let (progress, elapsed, seek, total) = progress_row(sender, 5, 40);
        let transport = Transport::new(sender, 22, 30, 6);
        let (volume_box, speaker, volume) = volume_row(sender, 120);
        let controls = gtk::CenterBox::new();
        // The spacer balances the volume, so the buttons sit in the middle of the row.
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_size_request(170, 1);
        controls.set_start_widget(Some(&spacer));
        controls.set_center_widget(Some(&transport.row));
        controls.set_end_widget(Some(&volume_box));

        let message = label("audio-message");
        let hint = label("audio-hint");
        hint.set_text("Space pause  \u{b7}  Ctrl+\u{2190}/\u{2192} seek  \u{b7}  N/P next, previous  \u{b7}  +/\u{2212} volume  \u{b7}  M mute  \u{b7}  Ctrl+Enter Open In External Player");

        let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
        content.add_css_class("audio-player");
        content.set_hexpand(true);
        content.set_vexpand(true);
        content.append(&corner);
        content.append(&poster);
        content.append(&meta);
        content.append(&progress);
        content.append(&controls);
        content.append(&message);
        content.append(&hint);

        let root = gtk::Overlay::new();
        root.add_css_class("audio-big");
        root.set_child(Some(&visual.area));
        root.add_overlay(&content);
        root.set_hexpand(true);
        root.set_vexpand(true);

        Self {
            root,
            visual,
            art,
            cover,
            title,
            artist,
            album,
            detail,
            elapsed,
            total,
            seek,
            transport,
            speaker,
            volume,
            bars,
            message,
            hint,
            visual_check,
            style_checks,
            strength,
            smooth,
            density,
            painting,
            visual_on,
            shown: RefCell::new(Look { volume: -1.0, ..Look::default() }),
            cover_rev: Cell::new(0),
        }
    }

    /// The keyboard drives the player: it is outlined in the accent color.
    pub fn set_focused(&self, focused: bool) {
        if self.root.has_css_class("focused") != focused {
            if focused {
                self.root.add_css_class("focused");
            } else {
                self.root.remove_css_class("focused");
            }
        }
    }

    pub fn set_scope(&self, scope: Scope) {
        self.visual.set_scope(scope);
    }

    /// What the pane shows now.
    pub fn looks(&self) -> Look {
        self.shown.borrow().clone()
    }

    pub fn set_cover(&self, rev: u64, image: Option<(u32, u32, &[u8])>) {
        if self.cover_rev.get() == rev {
            return;
        }
        self.cover_rev.set(rev);
        let has = show_cover(&self.cover, image);
        self.art.set_visible_child_name(if has { "cover" } else { "note" });
    }

    pub fn paint(&self, look: &Look) {
        let mut shown = self.shown.borrow_mut();
        if *shown == *look {
            return;
        }
        self.painting.set(true);
        set_text(&self.title, &look.title);
        set_text(&self.artist, &look.artist);
        set_text(&self.album, &look.album);
        set_text(&self.detail, &look.detail);
        set_text(&self.message, &look.message);
        // The key help is for a track that plays. A message takes its place.
        self.hint.set_visible(look.message.is_empty());
        // Labels with no text take no room, so the layout does not leave gaps.
        for (widget, text) in [(&self.artist, &look.artist), (&self.album, &look.album), (&self.detail, &look.detail), (&self.message, &look.message)] {
            widget.set_visible(!text.is_empty());
        }
        self.transport.paint(look);
        paint_progress(&self.elapsed, &self.total, &self.seek, look);
        paint_volume(&self.speaker, &self.volume, look);
        self.visual_on.set(look.visual_on);
        self.visual.set_on(look.visual_on);
        self.visual.set_style(look.visual);
        self.visual.set_tune(look.strength, look.smooth, look.density);
        show_knob(&self.strength, look.strength);
        show_knob(&self.smooth, look.smooth);
        show_knob(&self.density, look.density);
        for scale in [&self.strength, &self.smooth, &self.density] {
            scale.set_sensitive(look.visual_on);
        }
        self.bars.set_on(look.visual_on);
        if self.visual_check.is_active() != look.visual_on {
            self.visual_check.set_active(look.visual_on);
        }
        for (style, check) in &self.style_checks {
            if check.is_active() != (*style == look.visual) {
                check.set_active(*style == look.visual);
            }
            check.set_sensitive(look.visual_on);
        }
        self.painting.set(false);
        *shown = look.clone();
    }
}

fn show_knob(scale: &gtk::Scale, value: u8) {
    let value = f64::from(value.min(100));
    if (scale.value() - value).abs() > 0.5 {
        scale.set_value(value);
    }
}

fn tune_scale(title: &str, tip: &str, painting: &Rc<Cell<bool>>, tx: relm4::Sender<Msg>, msg: fn(u8) -> Msg) -> (gtk::Box, gtk::Scale) {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let name = gtk::Label::new(Some(title));
    name.set_xalign(0.0);
    name.set_tooltip_text(Some(tip));
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    scale.set_value(50.0);
    scale.set_draw_value(false);
    scale.set_hexpand(true);
    scale.set_width_request(168);
    scale.set_tooltip_text(Some(tip));
    let flag = Rc::clone(painting);
    scale.connect_value_changed(move |scale| {
        if !flag.get() {
            tx.emit(msg(scale.value().round().clamp(0.0, 100.0) as u8));
        }
    });
    row.append(&name);
    row.append(&scale);
    (row, scale)
}

/// The little menu behind the cogwheel: the visualizer on or off, which one, and how it moves.
fn settings_popover(
    sender: &relm4::Sender<Msg>,
    painting: &Rc<Cell<bool>>,
) -> (gtk::Popover, gtk::CheckButton, Vec<(VisualStyle, gtk::CheckButton)>, gtk::Scale, gtk::Scale, gtk::Scale) {
    let popover = gtk::Popover::new();
    popover.add_css_class("audio-pop");
    let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
    column.set_margin_top(6);
    column.set_margin_bottom(6);
    column.set_margin_start(8);
    column.set_margin_end(8);
    let heading = gtk::Label::new(Some("Visualizer"));
    heading.add_css_class("audio-pop-title");
    heading.set_xalign(0.0);
    column.append(&heading);
    let on = gtk::CheckButton::with_label("Show");
    let (tx, flag) = (sender.clone(), Rc::clone(painting));
    on.connect_toggled(move |check| {
        if !flag.get() {
            tx.emit(Msg::AudioVisualOn(check.is_active()));
        }
    });
    column.append(&on);
    let mut checks: Vec<(VisualStyle, gtk::CheckButton)> = Vec::new();
    for style in VisualStyle::ALL {
        let check = gtk::CheckButton::with_label(style.label());
        if let Some((_, first)) = checks.first() {
            check.set_group(Some(first));
        }
        let (tx, flag) = (sender.clone(), Rc::clone(painting));
        check.connect_toggled(move |check| {
            if check.is_active() && !flag.get() {
                tx.emit(Msg::AudioVisualStyle(style));
            }
        });
        column.append(&check);
        checks.push((style, check));
    }
    column.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let (strength_row, strength) = tune_scale("Strength", "How hard the sound moves the picture", painting, sender.clone(), Msg::AudioVisualStrength);
    let (smooth_row, smooth) = tune_scale("Smooth", "How slowly the picture falls back", painting, sender.clone(), Msg::AudioVisualSmooth);
    let (density_row, density) = tune_scale("Density", "How visible the picture is behind the text", painting, sender.clone(), Msg::AudioVisualDensity);
    column.append(&strength_row);
    column.append(&smooth_row);
    column.append(&density_row);
    popover.set_child(Some(&column));
    (popover, on, checks, strength, smooth, density)
}

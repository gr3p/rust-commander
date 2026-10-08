use relm4::gtk::glib;
use relm4::gtk::glib::subclass::prelude::*;
use relm4::gtk::prelude::*;
use std::path::Path;

use rust_commander::entry::Entry;
use rust_commander::listing;

mod imp {
    use super::*;
    use glib::Properties;
    use std::cell::{Cell, RefCell};

    #[derive(Properties, Default)]
    #[properties(wrapper_type = super::EntryObject)]
    pub struct EntryObject {
        #[property(get, set)]
        name: RefCell<String>,
        #[property(get, set)]
        ext: RefCell<String>,
        #[property(get, set)]
        size_text: RefCell<String>,
        #[property(get, set)]
        date: RefCell<String>,
        #[property(get, set)]
        attr: RefCell<String>,
        #[property(get, set)]
        marked: Cell<bool>,
        #[property(get, set)]
        glyph: RefCell<String>,
        #[property(get, set)]
        icon: RefCell<String>,
        #[property(get, set)]
        is_dir: Cell<bool>,
        // The thumbnail grid always draws an icon, so it does not follow the file icons switch.
        #[property(get, set)]
        big_icon: RefCell<String>,
        // Full path of an image, empty for everything else.
        #[property(get, set)]
        thumb: RefCell<String>,
        // Changes when the file does, so an edited image is decoded again.
        #[property(get, set)]
        stamp: Cell<u64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for EntryObject {
        const NAME: &'static str = "RustCommanderEntry";
        type Type = super::EntryObject;
    }

    #[glib::derived_properties]
    impl ObjectImpl for EntryObject {}
}

glib::wrapper! {
    pub struct EntryObject(ObjectSubclass<imp::EntryObject>);
}

impl EntryObject {
    pub fn from_entry(entry: &Entry, dir: &Path, marked: bool, icons: bool, pack: &str) -> Self {
        let skip = entry.name.as_ref() == "..";
        let name = if skip {
            "↑ [..]".into()
        } else if entry.is_symlink() {
            format!("{}@", entry.name)
        } else if entry.is_dir() && !skip {
            format!("{}/", entry.name)
        } else {
            entry.name.to_string()
        };
        let glyph = if skip || entry.is_dir() { "" } else { rust_commander::archive::unpack_mark(&entry.name) };
        let token = rust_commander::icons::file_token(pack, &entry.name, entry.is_dir(), entry.is_symlink());
        let icon = if icons { token.clone() } else { String::new() };
        let thumb = if entry.is_dir() || !rust_commander::imageview::is_image(&entry.name) {
            String::new()
        } else {
            listing::entry_path(dir, entry).display().to_string()
        };
        glib::Object::builder()
            .property("name", name)
            .property("ext", if skip { String::new() } else { listing::extension(&entry.name).to_string() })
            .property("size-text", size_text(entry, skip))
            .property("date", if skip { String::new() } else { listing::format_mtime(entry.mtime) })
            .property("attr", listing::format_attr(entry.mode))
            .property("marked", marked)
            .property("glyph", glyph)
            .property("icon", icon)
            .property("big-icon", token)
            .property("thumb", thumb)
            .property("stamp", entry.mtime as u64 ^ entry.size.rotate_left(32))
            .property("is-dir", entry.is_dir())
            .build()
    }
}

fn size_text(entry: &Entry, skip: bool) -> String {
    if skip {
        String::new()
    } else if entry.is_dir() {
        "<DIR>".into()
    } else {
        listing::format_size(entry.size)
    }
}

pub fn file_columns() -> [relm4::gtk::ColumnViewColumn; 6] {
    [
        glyph_column(),
        column("name", "Name", false, -1, 0.0, 0, 8, true, true),
        column("ext", "Ext", false, 80, 0.0, 0, 8, false, false),
        column("size", "Size", false, 96, 0.0, 0, 8, false, false),
        column("time", "Date", false, 188, 0.0, 0, 16, false, false),
        column("attr", "Attr", false, 120, 0.0, 0, 8, false, false),
    ]
}

fn column(
    id: &'static str,
    title: &str,
    expand: bool,
    width: i32,
    align: f32,
    margin_start: i32,
    margin_end: i32,
    ellipsize: bool,
    marked: bool,
) -> relm4::gtk::ColumnViewColumn {
    use relm4::gtk;
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let label = gtk::Label::new(None);
        label.set_xalign(align);
        label.set_hexpand(true);
        label.set_margin_start(margin_start);
        label.set_margin_end(margin_end);
        if ellipsize {
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        }
        if id == "name" {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
            row.set_hexpand(true);
            let icon = gtk::Image::new();
            icon.add_css_class("file-icon");
            icon.set_pixel_size(14);
            icon.set_visible(false);
            let glyph = gtk::Label::new(None);
            glyph.add_css_class("icon-glyph");
            glyph.add_css_class("file-icon");
            glyph.set_valign(gtk::Align::Center);
            glyph.set_visible(false);
            row.append(&up_arrow());
            row.append(&folder_icon());
            row.append(&glyph);
            row.append(&icon);
            row.append(&label);
            item.set_child(Some(&row));
        } else {
            item.set_child(Some(&label));
        }
    });
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let obj = item.item().unwrap().downcast::<EntryObject>().unwrap();
        let text = match id {
            "ext" => obj.ext(),
            "size" => obj.size_text(),
            "time" => obj.date(),
            "attr" => obj.attr(),
            _ => obj.name(),
        };
        let label = cell_label(item);
        if id == "name" && text == "↑ [..]" {
            item.child().and_then(|child| child.downcast::<gtk::Box>().ok()).unwrap().first_child().unwrap().set_visible(true);
            label.set_label("[..]");
        } else {
            if let Some(arrow) = item.child().and_then(|child| child.downcast::<gtk::Box>().ok()).and_then(|row| row.first_child()) {
                arrow.set_visible(false);
            }
            label.set_label(&text);
        }
        if id == "name" {
            paint_file_icon(item, &obj);
        }
        if marked {
            apply_marked(&label, obj.marked());
            let label = label.clone();
            let handler = obj.connect_marked_notify(move |obj| apply_marked(&label, obj.marked()));
            unsafe { item.set_data("mark-watch", MarkWatch { obj: obj.downgrade(), handler }) };
        }
    });
    if marked {
        factory.connect_unbind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let Some(watch) = (unsafe { item.steal_data::<MarkWatch>("mark-watch") }) else { return };
            if let Some(obj) = watch.obj.upgrade() {
                obj.disconnect(watch.handler);
            }
        });
    }
    let col = gtk::ColumnViewColumn::new(Some(title), Some(factory));
    col.set_expand(expand);
    col.set_resizable(true);
    if width > 0 {
        col.set_fixed_width(width);
    }
    col
}

fn glyph_column() -> relm4::gtk::ColumnViewColumn {
    use relm4::gtk;
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let row = gtk::CenterBox::new();
        let cursor = cursor_mark();
        let kind = gtk::Label::new(None);
        kind.add_css_class("glyph");
        kind.add_css_class("marked");
        kind.set_visible(false);
        row.set_center_widget(Some(&cursor));
        row.set_end_widget(Some(&kind));
        item.set_child(Some(&row));
        item.connect_selected_notify(set_cursor_glyph);
    });
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let obj = item.item().unwrap().downcast::<EntryObject>().unwrap();
        let row = item.child().unwrap().downcast::<gtk::CenterBox>().unwrap();
        let kind = row.end_widget().unwrap().downcast::<gtk::Label>().unwrap();
        set_cursor_glyph(item);
        let glyph = obj.glyph();
        kind.set_visible(!glyph.is_empty());
        kind.set_label(&glyph);
    });
    let col = gtk::ColumnViewColumn::new(Some(""), Some(factory));
    col.set_expand(false);
    col.set_resizable(false);
    col.set_fixed_width(12);
    col
}

fn set_cursor_glyph(item: &relm4::gtk::ListItem) {
    let Some(row) = item.child().and_then(|child| child.downcast::<relm4::gtk::CenterBox>().ok()) else { return };
    let Some(cursor) = row.center_widget().and_then(|child| child.downcast::<relm4::gtk::DrawingArea>().ok()) else { return };
    cursor.set_visible(item.is_selected());
}

fn cursor_mark() -> relm4::gtk::DrawingArea {
    let area = relm4::gtk::DrawingArea::new();
    area.add_css_class("cursor-mark");
    area.set_content_width(11);
    area.set_content_height(11);
    area.set_halign(relm4::gtk::Align::Center);
    area.set_valign(relm4::gtk::Align::Center);
    area.set_visible(false);
    area.set_draw_func(|area, cr, w, h| {
        let color = area.color();
        let (r, g, b, a) = (color.red() as f64, color.green() as f64, color.blue() as f64, color.alpha() as f64);
        cr.set_source_rgba(r, g, b, a);
        let w = w as f64;
        let h = h as f64;
        let loading = area.ancestor(relm4::gtk::ColumnView::static_type()).is_some_and(|list| list.has_css_class("loading"));
        if loading {
            // Eight dots on a ring. The bright one walks round, the ones behind it fade out.
            let head = (relm4::gtk::glib::monotonic_time() / SPIN_STEP_US) as usize % 8;
            let ring = w.min(h) * 0.5 - 1.3;
            for i in 0..8 {
                let angle = i as f64 * std::f64::consts::TAU / 8.0 - std::f64::consts::FRAC_PI_2;
                let age = (head + 8 - i) % 8;
                cr.set_source_rgba(r, g, b, a * (1.0 - age as f64 * 0.11));
                cr.arc(w * 0.5 + ring * angle.cos(), h * 0.5 + ring * angle.sin(), 1.1, 0.0, std::f64::consts::TAU);
                let _ = cr.fill();
            }
            return;
        }
        // Chevron like ">", with round ends so it reads as an icon. Drawn in the middle 8 px.
        let x = (w - 8.0) * 0.5;
        cr.set_line_width(2.0);
        cr.set_line_cap(relm4::gtk::cairo::LineCap::Round);
        cr.set_line_join(relm4::gtk::cairo::LineJoin::Round);
        cr.move_to(x + 1.4, 1.6);
        cr.line_to(x + 6.7, h * 0.5);
        cr.line_to(x + 1.4, h - 1.6);
        let _ = cr.stroke();
    });
    area
}

const SPIN_STEP_US: i64 = 90_000;

// The cursor mark turns into a dot spinner while the pane waits for a listing.
pub fn set_loading(list: &relm4::gtk::ColumnView, on: bool) {
    if list.has_css_class("loading") == on {
        return;
    }
    if on {
        list.add_css_class("loading");
        spin(list);
    } else {
        list.remove_css_class("loading");
    }
    redraw_marks(list.upcast_ref());
}

// Ticks at half a step so no dot is skipped. Stops by itself when the list stops loading.
fn spin(list: &relm4::gtk::ColumnView) {
    use relm4::gtk::glib;
    let list = list.downgrade();
    glib::timeout_add_local(std::time::Duration::from_micros(SPIN_STEP_US as u64 / 2), move || {
        let Some(list) = list.upgrade().filter(|list| list.has_css_class("loading")) else {
            return glib::ControlFlow::Break;
        };
        redraw_marks(list.upcast_ref());
        glib::ControlFlow::Continue
    });
}

// A class change on the list does not redraw the drawing areas under it.
fn redraw_marks(widget: &relm4::gtk::Widget) {
    let mut child = widget.first_child();
    while let Some(widget) = child {
        if widget.has_css_class("cursor-mark") {
            widget.queue_draw();
        } else {
            redraw_marks(&widget);
        }
        child = widget.next_sibling();
    }
}

struct MarkWatch {
    obj: relm4::gtk::glib::WeakRef<EntryObject>,
    handler: relm4::gtk::glib::SignalHandlerId,
}

fn apply_marked(label: &relm4::gtk::Label, on: bool) {
    if on {
        label.add_css_class("marked");
    } else {
        label.remove_css_class("marked");
    }
}

fn paint_file_icon(item: &relm4::gtk::ListItem, obj: &EntryObject) {
    let Some(row) = item.child().and_then(|child| child.downcast::<relm4::gtk::Box>().ok()) else { return };
    let Some(folder) = row.first_child().and_then(|arrow| arrow.next_sibling()) else { return };
    let Some(glyph) = folder.next_sibling().and_then(|widget| widget.downcast::<relm4::gtk::Label>().ok()) else { return };
    let Some(icon) = glyph.next_sibling().and_then(|widget| widget.downcast::<relm4::gtk::Image>().ok()) else { return };
    paint_icon(&folder, &glyph, &icon, &obj.icon(), obj.is_dir());
}

fn paint_icon(folder: &relm4::gtk::Widget, glyph: &relm4::gtk::Label, icon: &relm4::gtk::Image, name: &str, is_dir: bool) {
    let glyph_icon = crate::ui::iconpack::is_glyph(name);
    // Themes draw folder-symbolic as an outline, so directories get the filled folder instead.
    let filled = name == "folder-symbolic";
    folder.set_visible(filled);
    glyph.set_visible(glyph_icon);
    icon.set_visible(!filled && !glyph_icon && !name.is_empty());
    if glyph_icon {
        glyph.set_label(name);
    } else if !name.is_empty() && !filled {
        crate::ui::iconpack::apply_image(icon, name);
    }
    if is_dir {
        glyph.add_css_class("dir");
        icon.add_css_class("dir");
    } else {
        glyph.remove_css_class("dir");
        icon.remove_css_class("dir");
    }
}

const THUMB: i32 = 128;
const THUMB_ICON: i32 = 48;
const DECODERS: usize = 4;
const CACHE_MAX: usize = 512;

thread_local! {
    static THUMBS: std::cell::RefCell<std::collections::HashMap<(String, u64), relm4::gtk::gdk::Texture>> = Default::default();
    static DECODING: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[derive(Clone)]
struct ThumbCell {
    root: relm4::gtk::Box,
    stack: relm4::gtk::Stack,
    arrow: relm4::gtk::Widget,
    folder: relm4::gtk::Widget,
    glyph: relm4::gtk::Label,
    icon: relm4::gtk::Image,
    picture: relm4::gtk::Picture,
    label: relm4::gtk::Label,
}

impl ThumbCell {
    fn new() -> Self {
        use relm4::gtk;
        let (arrow, folder) = (up_arrow(), folder_icon());
        for area in [&arrow, &folder] {
            area.set_content_width(THUMB_ICON);
            area.set_content_height(THUMB_ICON);
        }
        let glyph = gtk::Label::new(None);
        glyph.add_css_class("icon-glyph");
        glyph.add_css_class("file-icon");
        let icon = gtk::Image::new();
        icon.add_css_class("file-icon");
        icon.set_pixel_size(THUMB_ICON);
        let symbols = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        symbols.set_halign(gtk::Align::Center);
        symbols.set_valign(gtk::Align::Center);
        for widget in [arrow.upcast_ref::<gtk::Widget>(), folder.upcast_ref(), glyph.upcast_ref(), icon.upcast_ref()] {
            symbols.append(widget);
        }
        let picture = gtk::Picture::new();
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_can_shrink(true);
        let stack = gtk::Stack::new();
        stack.set_size_request(THUMB, THUMB);
        stack.add_named(&symbols, Some("icon"));
        stack.add_named(&picture, Some("thumb"));
        let label = gtk::Label::new(None);
        label.set_justify(gtk::Justification::Center);
        label.set_wrap(true);
        label.set_wrap_mode(gtk::pango::WrapMode::Char);
        label.set_lines(2);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_max_width_chars(12);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        root.add_css_class("thumb-cell");
        root.append(&stack);
        root.append(&label);
        Self { root, stack, arrow: arrow.upcast(), folder: folder.upcast(), glyph, icon, picture, label }
    }

    fn show(&self, texture: &relm4::gtk::gdk::Texture) {
        self.picture.set_paintable(Some(texture));
        self.stack.set_visible_child_name("thumb");
    }
}

pub fn thumb_factory() -> relm4::gtk::SignalListItemFactory {
    use relm4::gtk;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let cell = ThumbCell::new();
        item.set_child(Some(&cell.root));
        unsafe { item.set_data("thumb-cell", cell) };
    });
    factory.connect_bind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let obj = item.item().unwrap().downcast::<EntryObject>().unwrap();
        let cell = unsafe { item.data::<ThumbCell>("thumb-cell").unwrap().as_ref().clone() };
        let name = obj.name();
        let up = name == "↑ [..]";
        cell.arrow.set_visible(up);
        cell.label.set_label(if up { "[..]" } else { &name });
        paint_icon(&cell.folder, &cell.glyph, &cell.icon, &obj.big_icon(), obj.is_dir());
        apply_marked(&cell.label, obj.marked());
        let label = cell.label.clone();
        let handler = obj.connect_marked_notify(move |obj| apply_marked(&label, obj.marked()));
        unsafe { item.set_data("mark-watch", MarkWatch { obj: obj.downgrade(), handler }) };
        load_thumb(item, &cell, &obj);
    });
    factory.connect_unbind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        if let Some(cancel) = unsafe { item.steal_data::<Arc<AtomicBool>>("thumb-cancel") } {
            cancel.store(true, Ordering::Relaxed);
        }
        let Some(watch) = (unsafe { item.steal_data::<MarkWatch>("mark-watch") }) else { return };
        if let Some(obj) = watch.obj.upgrade() {
            obj.disconnect(watch.handler);
        }
    });
    factory
}

// The icon stays until the image is decoded. A cell that is rebound or scrolled away cancels its decode.
fn load_thumb(item: &relm4::gtk::ListItem, cell: &ThumbCell, obj: &EntryObject) {
    use relm4::gtk::{gdk, gio, glib};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    cell.stack.set_visible_child_name("icon");
    let path = obj.thumb();
    if path.is_empty() {
        return;
    }
    let key = (path, obj.stamp());
    if let Some(texture) = THUMBS.with(|cache| cache.borrow().get(&key).cloned()) {
        cell.show(&texture);
        return;
    }
    let cancel = Arc::new(AtomicBool::new(false));
    unsafe { item.set_data("thumb-cancel", cancel.clone()) };
    let cell = cell.clone();
    glib::spawn_future_local(async move {
        // Large photos decode into a lot of memory, so only a few run at once.
        while DECODING.get() >= DECODERS {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            glib::timeout_future(std::time::Duration::from_millis(40)).await;
        }
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        DECODING.set(DECODING.get() + 1);
        let file = std::path::PathBuf::from(&key.0);
        let decoded = gio::spawn_blocking(move || rust_commander::imageview::decode(&file, THUMB as u32)).await;
        DECODING.set(DECODING.get() - 1);
        let Ok(Ok(img)) = decoded else { return };
        let stride = img.width as usize * 4;
        let texture: gdk::Texture = gdk::MemoryTexture::new(
            img.width as i32,
            img.height as i32,
            gdk::MemoryFormat::R8g8b8a8,
            &glib::Bytes::from_owned(img.pixels),
            stride,
        )
        .upcast();
        THUMBS.with(|cache| {
            let mut cache = cache.borrow_mut();
            if cache.len() >= CACHE_MAX {
                cache.clear();
            }
            cache.insert(key, texture.clone());
        });
        if !cancel.load(Ordering::Relaxed) {
            cell.show(&texture);
        }
    });
}

// GridView does not report its column count. A full row is the most cells sharing one top edge.
// Cells it keeps for reuse are not mapped and sit in a corner, so they are skipped.
pub fn grid_shape(grid: &relm4::gtk::GridView) -> (usize, usize) {
    let mut rows = std::collections::HashMap::<i32, usize>::new();
    let mut child = grid.first_child();
    while let Some(cell) = child {
        if let Some(bounds) = cell.compute_bounds(grid).filter(|_| cell.is_mapped()) {
            *rows.entry(bounds.y().round() as i32).or_default() += 1;
        }
        child = cell.next_sibling();
    }
    (rows.values().copied().max().unwrap_or(1), rows.len())
}

pub fn reveal_cell(grid: &relm4::gtk::GridView, pos: u32) {
    let _ = grid.activate_action("list.scroll-to-item", Some(&relm4::gtk::glib::Variant::from(pos)));
}

fn cell_label(item: &relm4::gtk::ListItem) -> relm4::gtk::Label {
    let child = item.child().unwrap();
    if let Some(row) = child.downcast_ref::<relm4::gtk::Box>() {
        row.last_child().unwrap().downcast().unwrap()
    } else {
        child.downcast().unwrap()
    }
}

fn folder_icon() -> relm4::gtk::DrawingArea {
    let area = relm4::gtk::DrawingArea::new();
    area.add_css_class("file-icon");
    area.add_css_class("dir");
    area.set_content_width(14);
    area.set_content_height(14);
    area.set_valign(relm4::gtk::Align::Center);
    area.set_visible(false);
    area.set_draw_func(|area, cr, w, h| {
        let color = area.color();
        cr.set_source_rgba(color.red() as f64, color.green() as f64, color.blue() as f64, color.alpha() as f64);
        cr.scale(w as f64 / 14.0, h as f64 / 14.0);
        let (r, pi) = (1.5, std::f64::consts::PI);
        // Tab on the upper left, then the body. Straight edges sit on whole pixels at 14 px.
        cr.arc(r, 1.0 + r, r, pi, 1.5 * pi);
        cr.line_to(5.0, 1.0);
        cr.line_to(7.0, 3.0);
        cr.arc(14.0 - r, 3.0 + r, r, 1.5 * pi, 2.0 * pi);
        cr.arc(14.0 - r, 13.0 - r, r, 0.0, 0.5 * pi);
        cr.arc(r, 13.0 - r, r, 0.5 * pi, pi);
        cr.close_path();
        let _ = cr.fill();
    });
    area
}

fn up_arrow() -> relm4::gtk::DrawingArea {
    let area = relm4::gtk::DrawingArea::new();
    area.set_content_width(12);
    area.set_content_height(12);
    area.set_valign(relm4::gtk::Align::Center);
    area.set_visible(false);
    area.set_draw_func(|area, cr, w, h| {
        let color = area.color();
        cr.set_source_rgba(color.red() as f64, color.green() as f64, color.blue() as f64, color.alpha() as f64);
        let w = w as f64;
        let h = h as f64;
        // Wide head and a short stem, so the row stays one line high.
        cr.move_to(w * 0.5, 0.5);
        cr.line_to(w - 0.5, h * 0.46);
        cr.line_to(w * 0.68, h * 0.46);
        cr.line_to(w * 0.68, h - 0.5);
        cr.line_to(w * 0.32, h - 0.5);
        cr.line_to(w * 0.32, h * 0.46);
        cr.line_to(0.5, h * 0.46);
        cr.close_path();
        let _ = cr.fill();
    });
    area
}

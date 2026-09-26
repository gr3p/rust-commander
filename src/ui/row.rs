use relm4::gtk::glib;
use relm4::gtk::glib::subclass::prelude::*;
use relm4::gtk::prelude::*;

use comarchy::entry::Entry;
use comarchy::listing;

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
    }

    #[glib::object_subclass]
    impl ObjectSubclass for EntryObject {
        const NAME: &'static str = "ComarchyEntry";
        type Type = super::EntryObject;
    }

    #[glib::derived_properties]
    impl ObjectImpl for EntryObject {}
}

glib::wrapper! {
    pub struct EntryObject(ObjectSubclass<imp::EntryObject>);
}

impl EntryObject {
    pub fn from_entry(entry: &Entry, marked: bool, icons: bool) -> Self {
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
        let glyph = if skip || entry.is_dir() { "" } else { comarchy::archive::unpack_mark(&entry.name) };
        let icon = if icons { listing::file_icon(&entry.name, entry.is_dir(), entry.is_symlink()) } else { "" };
        glib::Object::builder()
            .property("name", name)
            .property("ext", if skip { String::new() } else { listing::extension(&entry.name).to_string() })
            .property("size-text", size_text(entry, skip))
            .property("date", if skip { String::new() } else { listing::format_mtime(entry.mtime) })
            .property("attr", listing::format_attr(entry.mode))
            .property("marked", marked)
            .property("glyph", glyph)
            .property("icon", icon)
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
            let icon = gtk::Label::new(None);
            icon.add_css_class("file-icon");
            icon.set_visible(false);
            row.append(&up_arrow());
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
        let cursor = gtk::Label::new(None);
        cursor.add_css_class("glyph");
        cursor.add_css_class("cursor");
        cursor.set_visible(false);
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
    let Some(cursor) = row.center_widget().and_then(|child| child.downcast::<relm4::gtk::Label>().ok()) else { return };
    let on = item.is_selected();
    cursor.set_visible(on);
    if on {
        cursor.set_label(">");
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
    let Some(icon) = row.first_child().and_then(|arrow| arrow.next_sibling()).and_then(|widget| widget.downcast::<relm4::gtk::Label>().ok()) else { return };
    let text = obj.icon();
    icon.set_visible(!text.is_empty());
    icon.set_label(&text);
    if obj.is_dir() {
        icon.add_css_class("dir");
    } else {
        icon.remove_css_class("dir");
    }
}

fn cell_label(item: &relm4::gtk::ListItem) -> relm4::gtk::Label {
    let child = item.child().unwrap();
    if let Some(row) = child.downcast_ref::<relm4::gtk::Box>() {
        row.last_child().unwrap().downcast().unwrap()
    } else {
        child.downcast().unwrap()
    }
}

fn up_arrow() -> relm4::gtk::DrawingArea {
    let area = relm4::gtk::DrawingArea::new();
    area.set_content_width(12);
    area.set_content_height(9);
    area.set_valign(relm4::gtk::Align::Center);
    area.set_visible(false);
    area.set_draw_func(|area, cr, w, h| {
        let color = area.style_context().color();
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

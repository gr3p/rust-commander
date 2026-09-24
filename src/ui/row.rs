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
    pub fn from_entry(entry: &Entry, marked: bool) -> Self {
        let skip = entry.name.as_ref() == "..";
        let name = if entry.is_symlink() {
            format!("{}@", entry.name)
        } else if entry.is_dir() && !skip {
            format!("{}/", entry.name)
        } else {
            entry.name.to_string()
        };
        glib::Object::builder()
            .property("name", name)
            .property("ext", if skip { String::new() } else { listing::extension(&entry.name).to_string() })
            .property("size-text", size_text(entry, skip))
            .property("date", if skip { String::new() } else { listing::format_mtime(entry.mtime) })
            .property("attr", listing::format_attr(entry.mode))
            .property("marked", marked)
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

pub fn file_columns() -> [relm4::gtk::ColumnViewColumn; 5] {
    [
        column("name", "Name", true, -1, 0.0, 8, 8, true, true),
        column("ext", "Ext", false, 80, 0.0, 8, 8, false, false),
        column("size", "Size", false, 96, 1.0, 8, 8, false, false),
        column("time", "Date", false, 188, 0.0, 8, 16, false, false),
        column("attr", "Attr", false, 120, 0.0, 8, 8, false, false),
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
        label.set_hexpand(expand);
        label.set_margin_start(margin_start);
        label.set_margin_end(margin_end);
        if ellipsize {
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        }
        item.set_child(Some(&label));
    });
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let obj = item.item().unwrap().downcast::<EntryObject>().unwrap();
        let label = item.child().unwrap().downcast::<gtk::Label>().unwrap();
        let text = match id {
            "ext" => obj.ext(),
            "size" => obj.size_text(),
            "time" => obj.date(),
            "attr" => obj.attr(),
            _ => {
                let prefix = if obj.marked() { "● " } else { "  " };
                format!("{prefix}{}", obj.name())
            }
        };
        label.set_label(&text);
        if marked {
            if obj.marked() {
                label.add_css_class("marked");
            } else {
                label.remove_css_class("marked");
            }
        }
    });
    let col = gtk::ColumnViewColumn::new(Some(title), Some(factory));
    col.set_expand(expand);
    col.set_resizable(true);
    if width > 0 {
        col.set_fixed_width(width);
    }
    col
}

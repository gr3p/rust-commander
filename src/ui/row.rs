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
        meta: RefCell<String>,
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
        let meta = if entry.name.as_ref() == ".." {
            String::new()
        } else if entry.is_dir() {
            format!("<DIR>  {}", listing::format_mtime(entry.mtime))
        } else {
            format!(
                "{:>10}  {}",
                listing::format_size(entry.size),
                listing::format_mtime(entry.mtime)
            )
        };
        let name = if entry.is_symlink() {
            format!("{}@", entry.name)
        } else if entry.is_dir() && entry.name.as_ref() != ".." {
            format!("{}/", entry.name)
        } else {
            entry.name.to_string()
        };
        glib::Object::builder()
            .property("name", name)
            .property("meta", meta)
            .property("marked", marked)
            .property("is-dir", entry.is_dir())
            .build()
    }
}

pub fn factory() -> relm4::gtk::SignalListItemFactory {
    use relm4::gtk;
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let name = gtk::Label::new(None);
        name.set_xalign(0.0);
        name.set_hexpand(true);
        name.set_widget_name("name");
        let meta = gtk::Label::new(None);
        meta.set_xalign(1.0);
        meta.set_widget_name("meta");
        row.append(&name);
        row.append(&meta);
        item.set_child(Some(&row));
    });
    factory.connect_bind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let obj = item.item().unwrap().downcast::<EntryObject>().unwrap();
        let row = item.child().unwrap().downcast::<gtk::Box>().unwrap();
        let name = row.first_child().unwrap().downcast::<gtk::Label>().unwrap();
        let meta = name.next_sibling().unwrap().downcast::<gtk::Label>().unwrap();
        let prefix = if obj.marked() { "● " } else { "  " };
        name.set_label(&format!("{prefix}{}", obj.name()));
        meta.set_label(&obj.meta());
        if obj.marked() {
            name.add_css_class("marked");
        } else {
            name.remove_css_class("marked");
        }
    });
    factory
}

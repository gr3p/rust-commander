use relm4::gtk;
use relm4::gtk::gio;
use relm4::gtk::prelude::*;
use relm4::RelmWidgetExt;

use super::row::{self, EntryObject};

pub struct PaneUi {
    pub frame: gtk::Frame,
    pub path: gtk::Entry,
    pub tabs: gtk::Box,
    pub filter: gtk::Entry,
    pub stack: gtk::Stack,
    pub list: gtk::ListView,
    pub store: gio::ListStore,
    pub selection: gtk::SingleSelection,
    pub preview: gtk::TextView,
    pub spinner: gtk::Spinner,
}

pub struct Shell {
    pub window: gtk::Window,
    pub panes: [PaneUi; 2],
    pub status: gtk::Label,
    pub css: gtk::CssProvider,
    pub f_labels: Vec<String>,
}

pub fn build(window: &gtk::Window, sender: &relm4::Sender<super::msg::Msg>) -> Shell {
    let css = gtk::CssProvider::new();
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.set_hexpand(true);
    paned.set_vexpand(true);
    paned.set_position(560);
    let left = pane(0, sender);
    let right = pane(1, sender);
    paned.set_start_child(Some(&left.frame));
    paned.set_end_child(Some(&right.frame));
    paned.set_resize_start_child(true);
    paned.set_resize_end_child(true);
    root.append(&paned);

    let status = gtk::Label::new(Some(""));
    status.set_xalign(0.0);
    status.add_css_class("status");
    status.set_margin_start(8);
    root.append(&status);

    let fbar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    fbar.add_css_class("fbar");
    fbar.set_margin_all(4);
    let mut f_labels = Vec::new();
    for (n, label) in [
        (3, "F3 View"),
        (4, "F4 Edit"),
        (5, "F5 Copy"),
        (6, "F6 Move"),
        (7, "F7 Mkdir"),
        (8, "F8 Delete"),
    ] {
        let button = gtk::Button::with_label(label);
        button.add_css_class("fkey");
        button.set_hexpand(true);
        f_labels.push(label.to_string());
        let tx = sender.clone();
        button.connect_clicked(move |_| tx.emit(super::msg::Msg::FKey(n, false)));
        fbar.append(&button);
    }
    root.append(&fbar);
    window.set_child(Some(&root));

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let tx = sender.clone();
    let win = window.clone();
    keys.connect_key_pressed(move |_, key, _, mods| {
        let focused = relm4::gtk::prelude::GtkWindowExt::focus(&win);
        let typing = focused.is_some_and(|w| w.downcast_ref::<gtk::Entry>().is_some());
        if typing && !is_command(key, mods) {
            return gtk::glib::Propagation::Proceed;
        }
        tx.emit(super::msg::Msg::Key(key, mods));
        gtk::glib::Propagation::Stop
    });
    window.add_controller(keys);

    Shell {
        window: window.clone(),
        panes: [left, right],
        status,
        css,
        f_labels,
    }
}

fn is_command(key: gtk::gdk::Key, mods: gtk::gdk::ModifierType) -> bool {
    use gtk::gdk::Key;
    let ctrl = mods.contains(gtk::gdk::ModifierType::CONTROL_MASK);
    let alt = mods.contains(gtk::gdk::ModifierType::ALT_MASK);
    matches!(
        key,
        Key::F1
            | Key::F2
            | Key::F3
            | Key::F4
            | Key::F5
            | Key::F6
            | Key::F7
            | Key::F8
            | Key::F9
            | Key::F10
            | Key::Escape
            |         Key::Insert
            | Key::Tab
            | Key::ISO_Left_Tab
            | Key::Up
            | Key::Down
            | Key::KP_Up
            | Key::KP_Down
            | Key::Page_Up
            | Key::Page_Down
            | Key::KP_Page_Up
            | Key::KP_Page_Down
            | Key::Home
            | Key::End
            | Key::KP_Home
            | Key::KP_End
    ) || ctrl
        || alt
}

fn pane(index: usize, sender: &relm4::Sender<super::msg::Msg>) -> PaneUi {
    let frame = gtk::Frame::new(None);
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    tabs.set_margin_all(4);
    let path = gtk::Entry::new();
    path.set_hexpand(true);
    let tx = sender.clone();
    path.connect_activate(move |entry| {
        tx.emit(super::msg::Msg::PathGo(index, entry.text().to_string()));
    });
    let filter = gtk::Entry::new();
    filter.set_placeholder_text(Some("Filter"));
    filter.set_visible(false);
    let tx = sender.clone();
    filter.connect_changed(move |entry| {
        tx.emit(super::msg::Msg::FilterChanged(index, entry.text().to_string()));
    });
    let store = gio::ListStore::new::<EntryObject>();
    let selection = gtk::SingleSelection::new(Some(store.clone()));
    selection.set_autoselect(false);
    let tx = sender.clone();
    selection.connect_selected_notify(move |sel| {
        if super::msg::syncing() {
            return;
        }
        tx.emit(super::msg::Msg::Cursor(index, sel.selected()));
    });
    let list = gtk::ListView::new(Some(selection.clone()), Some(row::factory()));
    list.set_hexpand(true);
    list.set_vexpand(true);
    let tx = sender.clone();
    list.connect_activate(move |_, _| tx.emit(super::msg::Msg::Activate(index)));
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_child(Some(&list));
    scroll.set_vexpand(true);
    let preview = gtk::TextView::new();
    preview.set_editable(false);
    preview.set_monospace(true);
    preview.set_wrap_mode(gtk::WrapMode::WordChar);
    let preview_scroll = gtk::ScrolledWindow::new();
    preview_scroll.set_child(Some(&preview));
    preview_scroll.set_vexpand(true);
    let stack = gtk::Stack::new();
    stack.add_named(&scroll, Some("list"));
    stack.add_named(&preview_scroll, Some("preview"));
    stack.set_vexpand(true);
    let spinner = gtk::Spinner::new();
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&stack));
    spinner.set_halign(gtk::Align::Center);
    spinner.set_valign(gtk::Align::Center);
    spinner.set_can_target(false);
    overlay.add_overlay(&spinner);
    column.append(&tabs);
    column.append(&path);
    column.append(&filter);
    column.append(&overlay);
    frame.set_child(Some(&column));
    frame.add_css_class("pane");
    PaneUi {
        frame,
        path,
        tabs,
        filter,
        stack,
        list,
        store,
        selection,
        preview,
        spinner,
    }
}

use relm4::gtk;
use relm4::gtk::gio;
use relm4::gtk::prelude::*;
use relm4::RelmWidgetExt;

use comarchy::listing::SortKey;

use super::row::{self, EntryObject};

pub struct PaneUi {
    pub frame: gtk::Frame,
    pub path: gtk::Label,
    pub tabs: gtk::Box,
    pub filter: gtk::Entry,
    pub stack: gtk::Stack,
    pub list: gtk::ColumnView,
    pub store: gio::ListStore,
    pub selection: gtk::SingleSelection,
    pub preview: gtk::TextView,
    pub spinner: gtk::Spinner,
}

pub struct Shell {
    pub window: gtk::Window,
    pub panes: [PaneUi; 2],
    pub status: gtk::Label,
    pub command: gtk::Entry,
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
    if let Some(drives) = drive_bar(sender) {
        root.append(&drives);
    }
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

    let command = gtk::Entry::new();
    command.set_hexpand(true);
    command.set_margin_start(4);
    command.set_margin_end(4);
    command.set_margin_top(4);
    let tx = sender.clone();
    command.connect_activate(move |entry| {
        tx.emit(super::msg::Msg::Command(entry.text().to_string()));
    });
    root.append(&command);

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
        (12, "F12 Settings"),
    ] {
        let text = gtk::Label::new(Some(label));
        text.set_ellipsize(gtk::pango::EllipsizeMode::End);
        text.set_hexpand(true);
        let button = gtk::Button::new();
        button.set_child(Some(&text));
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
    let command_keys = command.clone();
    keys.connect_key_pressed(move |_, key, _, mods| {
        let focused = relm4::gtk::prelude::GtkWindowExt::focus(&win);
        let typing = focused.is_some_and(|w| w.downcast_ref::<gtk::Entry>().is_some());
        let ctrl = mods.contains(gtk::gdk::ModifierType::CONTROL_MASK);
        let ctrl_arrow = ctrl && matches!(key, gtk::gdk::Key::Left | gtk::gdk::Key::Right | gtk::gdk::Key::KP_Left | gtk::gdk::Key::KP_Right);
        if gtk::prelude::WidgetExt::has_focus(&command_keys) && ctrl_arrow {
            return gtk::glib::Propagation::Proceed;
        }
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
        command,
        css,
        f_labels,
    }
}

fn drive_bar(sender: &relm4::Sender<super::msg::Msg>) -> Option<gtk::Box> {
    let mounts = comarchy::places::mounts();
    if mounts.is_empty() {
        return None;
    }
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    bar.add_css_class("drives");
    bar.set_margin_start(6);
    bar.set_margin_end(6);
    bar.set_margin_top(4);
    for mount in mounts {
        let button = gtk::Button::new();
        button.add_css_class("drive");
        let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
        column.append(&drive_icon(mount.removable, mount.network));
        let caption = gtk::Label::new(Some(&mount.label));
        caption.set_ellipsize(gtk::pango::EllipsizeMode::End);
        caption.set_max_width_chars(12);
        column.append(&caption);
        button.set_child(Some(&column));
        button.set_tooltip_text(Some(&format!("{}  {}", mount.device, mount.path.display())));
        let tx = sender.clone();
        let path = mount.path;
        button.connect_clicked(move |_| tx.emit(super::msg::Msg::Mount(path.clone())));
        bar.append(&button);
    }
    Some(bar)
}

fn drive_icon(removable: bool, network: bool) -> gtk::Widget {
    let (name, fallback) = if network {
        ("network-server-symbolic", "⇄")
    } else if removable {
        ("drive-removable-media-symbolic", "⏏")
    } else {
        ("drive-harddisk-symbolic", "▮")
    };
    let theme = gtk::gdk::Display::default().map(|display| gtk::IconTheme::for_display(&display));
    if theme.is_some_and(|theme| theme.has_icon(name)) {
        let icon = gtk::Image::from_icon_name(name);
        icon.set_pixel_size(22);
        icon.upcast()
    } else {
        gtk::Label::new(Some(fallback)).upcast()
    }
}

fn bind_header_clicks(list: &gtk::ColumnView, index: usize, sender: &relm4::Sender<super::msg::Msg>) {
    let keys = [SortKey::Name, SortKey::Ext, SortKey::Size, SortKey::Time, SortKey::Attr];
    let sender = sender.clone();
    let bound = std::rc::Rc::new(std::cell::Cell::new(false));
    let list_for_map = list.clone();
    let try_bind = {
        let bound = std::rc::Rc::clone(&bound);
        move |list: &gtk::ColumnView| {
            if bound.get() {
                return;
            }
            let titles = header_titles(list);
            if titles.len() != keys.len() {
                return;
            }
            for (title, key) in titles.into_iter().zip(keys) {
                let Some(label) = title_label(&title) else { continue };
                label.set_xalign(0.0);
                label.set_halign(gtk::Align::Start);
                label.set_hexpand(true);
                if let Some(parent) = label.parent() {
                    parent.set_halign(gtk::Align::Start);
                    parent.set_hexpand(true);
                }
                label.set_can_target(true);
                let click = gtk::GestureClick::new();
                let tx = sender.clone();
                click.connect_released(move |_, n, _, _| {
                    if n == 1 {
                        tx.emit(super::msg::Msg::Sort(index, key));
                    }
                });
                label.add_controller(click);
            }
            bound.set(true);
        }
    };
    try_bind(list);
    if !bound.get() {
        list_for_map.connect_map(move |list| try_bind(list));
    }
}

pub fn header_titles(list: &gtk::ColumnView) -> Vec<gtk::Widget> {
    let mut titles = Vec::new();
    collect_titles(list.upcast_ref(), &mut titles);
    titles
}

fn title_label(title: &gtk::Widget) -> Option<gtk::Label> {
    title.first_child()?.first_child()?.downcast().ok()
}

fn collect_titles(widget: &gtk::Widget, out: &mut Vec<gtk::Widget>) {
    if widget.type_().name() == "GtkColumnViewTitle" {
        out.push(widget.clone());
        return;
    }
    let mut child = widget.first_child();
    while let Some(next) = child {
        collect_titles(&next, out);
        child = next.next_sibling();
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
            | Key::F12
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
    tabs.set_visible(false);
    let path = gtk::Label::new(None);
    path.add_css_class("path");
    path.set_xalign(0.0);
    path.set_hexpand(true);
    path.set_can_focus(false);
    path.set_selectable(false);
    path.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
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
    let list = gtk::ColumnView::new(Some(selection.clone()));
    list.set_hexpand(true);
    list.set_vexpand(true);
    list.set_show_column_separators(true);
    for col in row::file_columns() {
        list.append_column(&col);
    }
    bind_header_clicks(&list, index, sender);
    let tx = sender.clone();
    list.connect_activate(move |_, _| tx.emit(super::msg::Msg::Activate(index)));
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_child(Some(&list));
    scroll.set_vexpand(true);
    let preview = gtk::TextView::new();
    preview.set_editable(false);
    preview.set_monospace(true);
    preview.set_wrap_mode(gtk::WrapMode::None);
    preview.set_left_margin(8);
    preview.set_right_margin(8);
    preview.set_top_margin(8);
    preview.set_bottom_margin(8);
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

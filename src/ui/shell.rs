use std::cell::Cell;
use std::rc::Rc;

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
    pub picture: gtk::Picture,
    pub picture_scroll: gtk::ScrolledWindow,
    pub preview_kind: gtk::Stack,
    pub spinner: gtk::Spinner,
}

pub struct Shell {
    pub window: gtk::Window,
    pub panes: [PaneUi; 2],
    pub status: gtk::Label,
    pub css: gtk::CssProvider,
    pub f_labels: Vec<String>,
    pub mode_bar: gtk::Stack,
    pub image_scale: gtk::Button,
    pub unpack_list: gtk::Button,
    pub unpack_here: gtk::Button,
    pub verify_sfv: gtk::Button,
    pub active: Rc<Cell<usize>>,
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
    let active = Rc::new(Cell::new(0));
    if let Some(drives) = drive_bar(sender, &active) {
        root.append(&drives);
    }
    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.set_hexpand(true);
    paned.set_vexpand(true);
    paned.set_position(560);
    let left = pane(0, sender, &active);
    let right = pane(1, sender, &active);
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
    let unpack_list = hint_button("Alt+F9 List", || super::msg::Msg::UnpackList, sender);
    let unpack_here = hint_button("Ctrl+Alt+F10 Here", || super::msg::Msg::UnpackHere, sender);
    let verify_sfv = hint_button("Enter Verify", || super::msg::Msg::VerifySfv, sender);
    fbar.append(&unpack_list);
    fbar.append(&unpack_here);
    fbar.append(&verify_sfv);
    let image_bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    image_bar.add_css_class("fbar");
    image_bar.set_margin_all(4);
    let image_scale = hint_button("S Fit", || super::msg::Msg::ToggleImageScale, sender);
    let image_prev = hint_button("Left Previous", || super::msg::Msg::ImageStep(-1), sender);
    let image_next = hint_button("Right Next", || super::msg::Msg::ImageStep(1), sender);
    image_scale.set_visible(true);
    image_prev.set_visible(true);
    image_next.set_visible(true);
    image_bar.append(&image_scale);
    image_bar.append(&image_prev);
    image_bar.append(&image_next);
    let mode_bar = gtk::Stack::new();
    mode_bar.add_named(&fbar, Some("files"));
    mode_bar.add_named(&image_bar, Some("image"));
    root.append(&mode_bar);
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
        mode_bar,
        image_scale,
        unpack_list,
        unpack_here,
        verify_sfv,
        active,
    }
}

fn hint_button(
    label: &str,
    make: impl Fn() -> super::msg::Msg + 'static,
    sender: &relm4::Sender<super::msg::Msg>,
) -> gtk::Button {
    let button = gtk::Button::with_label(label);
    button.add_css_class("fkey");
    button.set_hexpand(true);
    button.set_visible(false);
    let tx = sender.clone();
    button.connect_clicked(move |_| tx.emit(make()));
    button
}

fn drive_bar(sender: &relm4::Sender<super::msg::Msg>, active: &Rc<Cell<usize>>) -> Option<gtk::Box> {
    let mounts = comarchy::places::mounts();
    if mounts.is_empty() {
        return None;
    }
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    bar.add_css_class("drives");
    bar.set_margin_start(4);
    bar.set_margin_end(4);
    bar.set_margin_top(2);
    for mount in mounts {
        let button = gtk::Button::new();
        button.add_css_class("drive");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        row.append(&drive_icon(mount.removable, mount.network));
        let caption = gtk::Label::new(Some(&mount.label));
        caption.set_ellipsize(gtk::pango::EllipsizeMode::End);
        caption.set_max_width_chars(10);
        row.append(&caption);
        button.set_child(Some(&row));
        button.set_tooltip_text(Some(&format!("{}  {}", mount.device, mount.path.display())));
        button.set_focus_on_click(false);
        let tx = sender.clone();
        let path = mount.path;
        let active = active.clone();
        button.connect_clicked(move |_| tx.emit(super::msg::Msg::Mount(active.get(), path.clone())));
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
        icon.set_pixel_size(12);
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
                if let Some(label) = title_label(&title) {
                    label.set_xalign(0.0);
                    label.set_halign(gtk::Align::Start);
                }
                // Header claims the click in bubble phase, so listen on the way down.
                let click = gtk::GestureClick::new();
                click.set_propagation_phase(gtk::PropagationPhase::Capture);
                click.set_button(1);
                let tx = sender.clone();
                click.connect_pressed(move |gesture, n, _, _| {
                    if n == 1 {
                        gesture.set_state(gtk::EventSequenceState::Claimed);
                        tx.emit(super::msg::Msg::Sort(index, key));
                    }
                });
                title.add_controller(click);
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
    titles.retain(|title| title_label(title).is_some_and(|label| !label.text().is_empty()));
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

fn pane(index: usize, sender: &relm4::Sender<super::msg::Msg>, active_pane: &Rc<Cell<usize>>) -> PaneUi {
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
    let path_click = gtk::GestureClick::new();
    let tx = sender.clone();
    path_click.connect_released(move |_, n, _, _| {
        if n == 2 {
            tx.emit(super::msg::Msg::Hotlist);
        }
    });
    path.add_controller(path_click);
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
    let focus = gtk::EventControllerFocus::new();
    let tx = sender.clone();
    focus.connect_enter(move |_| tx.emit(super::msg::Msg::FocusPane(index)));
    list.add_controller(focus);
    let watch = gtk::EventControllerLegacy::new();
    watch.set_propagation_phase(gtk::PropagationPhase::Capture);
    let tx = sender.clone();
    let active_pane = active_pane.clone();
    watch.connect_event(move |_, event| {
        if event.event_type() == gtk::gdk::EventType::ButtonPress {
            active_pane.set(index);
            tx.emit(super::msg::Msg::FocusPane(index));
        }
        gtk::glib::Propagation::Proceed
    });
    frame.add_controller(watch);
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
    let picture = gtk::Picture::new();
    picture.set_can_shrink(true);
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    let picture_scroll = gtk::ScrolledWindow::new();
    picture_scroll.set_child(Some(&picture));
    picture_scroll.set_hexpand(true);
    picture_scroll.set_vexpand(true);
    picture_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
    let preview_kind = gtk::Stack::new();
    preview_kind.add_named(&preview_scroll, Some("text"));
    preview_kind.add_named(&picture_scroll, Some("image"));
    preview_kind.set_vexpand(true);
    let stack = gtk::Stack::new();
    stack.add_named(&scroll, Some("list"));
    stack.add_named(&preview_kind, Some("preview"));
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
        picture,
        picture_scroll,
        preview_kind,
        spinner,
    }
}

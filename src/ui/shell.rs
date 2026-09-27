use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use relm4::gtk;
use relm4::gtk::gio;
use relm4::gtk::prelude::*;
use relm4::RelmWidgetExt;
use vte4::prelude::{TerminalExt, TerminalExtManual};
use vte4::Terminal;

use rust_commander::listing::SortKey;

use super::row::{self, EntryObject};

static FLASH_ENABLED: AtomicBool = AtomicBool::new(true);

pub struct PaneUi {
    pub frame: gtk::Frame,
    pub path: gtk::Entry,
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
    pub terminal_slot: gtk::Box,
}

pub struct Shell {
    pub window: gtk::Window,
    pub panes: [PaneUi; 2],
    pub status: gtk::Label,
    pub css: gtk::CssProvider,
    pub f_labels: Vec<String>,
    pub mode_bar: gtk::Stack,
    pub image_scale: gtk::Button,
    pub text_modes: CharsetBar,
    pub unpack_list: gtk::Button,
    pub unpack_here: gtk::Button,
    pub verify_sfv: gtk::Button,
    pub pack: gtk::Button,
    pub pack_here: gtk::Button,
    pub active: Rc<Cell<usize>>,
    pub drives: Vec<(PathBuf, gtk::Button)>,
    pub edit_button: gtk::Button,
    pub save_button: gtk::Button,
    pub editing: Rc<Cell<bool>>,
    pub shell_key: gtk::Label,
    pub shell_bar: gtk::Box,
    pub shell_buttons: gtk::Box,
    pub shells: Vec<PanelShell>,
    pub shell_current: Option<u64>,
    pub shell_seq: u64,
}

pub struct PanelShell {
    pub id: u64,
    pub term: Terminal,
    pub button: gtk::Button,
    pub number: gtk::Label,
    pub pid: std::rc::Rc<std::cell::Cell<Option<i32>>>,
}

pub fn build(window: &gtk::Window, sender: &relm4::Sender<super::msg::Msg>) -> Shell {
    window.add_css_class("rust-commander");
    set_logo(window);
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
    let (shell_bar, shell_buttons, drives) = drive_bar(sender, &active);
    root.append(&shell_bar);
    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.add_css_class("panes");
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
    status.set_margin_top(4);
    status.set_margin_start(8);
    root.append(&status);

    let fbar = gtk::Box::new(gtk::Orientation::Vertical, 4);
    fbar.add_css_class("fbar");
    fbar.set_margin_all(4);
    let context_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let fkey_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    fbar.append(&context_row);
    fbar.append(&fkey_row);
    let mut f_labels = Vec::new();
    let mut shell_key = gtk::Label::new(Some("F9 Shell"));
    for (n, label) in [
        (3, "F3 View"),
        (4, "F4 Edit"),
        (5, "F5 Copy"),
        (6, "F6 Move"),
        (7, "F7 Mkdir"),
        (8, "F8 Delete"),
        (9, "F9 Shell"),
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
        if n == 9 {
            shell_key = text;
            on_click(&button, move || tx.emit(super::msg::Msg::Key(gtk::gdk::Key::F9, gtk::gdk::ModifierType::empty())));
        } else {
            on_click(&button, move || tx.emit(super::msg::Msg::FKey(n, false)));
        }
        fkey_row.append(&button);
    }
    let unpack_list = hint_button("Alt+F9 Unpack to right panel", || super::msg::Msg::UnpackList, sender);
    let unpack_here = hint_button("Shift+Alt+F9 Extract here", || super::msg::Msg::UnpackHere, sender);
    let verify_sfv = hint_button("Enter Verify", || super::msg::Msg::VerifySfv, sender);
    let pack = hint_button("Alt+F5 Pack", || super::msg::Msg::Key(gtk::gdk::Key::F5, gtk::gdk::ModifierType::ALT_MASK), sender);
    let pack_here = hint_button(
        "Ctrl+Shift+F5 Pack here",
        || super::msg::Msg::Key(gtk::gdk::Key::F5, gtk::gdk::ModifierType::CONTROL_MASK | gtk::gdk::ModifierType::SHIFT_MASK),
        sender,
    );
    context_row.append(&pack);
    context_row.append(&pack_here);
    context_row.append(&unpack_list);
    context_row.append(&unpack_here);
    context_row.append(&verify_sfv);
    let filter_button = bar_button("Ctrl+S Filter", || super::msg::Msg::Key(gtk::gdk::Key::s, gtk::gdk::ModifierType::CONTROL_MASK), sender);
    let search_button = bar_button(
        "Ctrl+Shift+S Search",
        || super::msg::Msg::Key(gtk::gdk::Key::S, gtk::gdk::ModifierType::CONTROL_MASK | gtk::gdk::ModifierType::SHIFT_MASK),
        sender,
    );
    filter_button.set_visible(true);
    search_button.set_visible(true);
    context_row.append(&filter_button);
    context_row.append(&search_button);
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
    let text_modes = charset_bar(sender);
    let edit_button = bar_button("Ctrl+E Edit", || super::msg::Msg::ToggleQuickEdit, sender);
    let save_button = bar_button("Ctrl+S Save", || super::msg::Msg::SaveQuick, sender);
    text_modes.bar.append(&edit_button);
    text_modes.bar.append(&save_button);
    let editing = Rc::new(Cell::new(false));
    let mode_bar = gtk::Stack::new();
    mode_bar.set_vhomogeneous(false);
    mode_bar.add_named(&fbar, Some("files"));
    mode_bar.add_named(&image_bar, Some("image"));
    mode_bar.add_named(&text_modes.bar, Some("text"));
    root.append(&mode_bar);
    window.set_child(Some(&root));

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let tx = sender.clone();
    let win = window.clone();
    let editing_keys = Rc::clone(&editing);
    keys.connect_key_pressed(move |controller, key, _, mods| {
        let focused = relm4::gtk::prelude::GtkWindowExt::focus(&win);
        let typing = focused.as_ref().is_some_and(|widget| {
            let mut current = Some(widget.clone());
            while let Some(child) = current {
                if child.is::<gtk::Entry>() {
                    return true;
                }
                current = child.parent();
            }
            false
        });
        if typing && entry_input(key, mods) {
            if let Some(widget) = focused.as_ref() {
                controller.forward(widget);
            }
            return gtk::glib::Propagation::Stop;
        }
        if editing_keys.get() {
            if let Some(widget) = focused.as_ref() {
                if widget.downcast_ref::<gtk::TextView>().is_some() && !edit_command(key, mods) {
                    // Capture on the window runs before the text view, so Proceed never inserts.
                    controller.forward(widget);
                    return gtk::glib::Propagation::Stop;
                }
            }
        }
        let on_term = focused.as_ref().is_some_and(|widget| widget.has_css_class("panel-shell"));
        if on_term && !terminal_command(key, mods) {
            if let Some(widget) = focused.as_ref() {
                controller.forward(widget);
            }
            return gtk::glib::Propagation::Stop;
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
        text_modes,
        unpack_list,
        unpack_here,
        verify_sfv,
        pack,
        pack_here,
        active,
        drives,
        edit_button,
        save_button,
        editing,
        shell_key,
        shell_bar,
        shell_buttons,
        shells: Vec::new(),
        shell_current: None,
        shell_seq: 1,
    }
}

pub struct CharsetBar {
    pub bar: gtk::Box,
    pub ansi: gtk::Button,
    pub ascii: gtk::Button,
    pub json: gtk::Button,
}

pub fn charset_bar(sender: &relm4::Sender<super::msg::Msg>) -> CharsetBar {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    bar.add_css_class("fbar");
    bar.set_margin_all(4);
    let ansi = hint_button("A ANSI", || super::msg::Msg::SetCharset(rust_commander::lister::Charset::Ansi), sender);
    let ascii = hint_button("S ASCII", || super::msg::Msg::SetCharset(rust_commander::lister::Charset::Ascii), sender);
    let json = hint_button("J JSON", || super::msg::Msg::SetCharset(rust_commander::lister::Charset::Json), sender);
    ansi.set_visible(true);
    ascii.set_visible(true);
    json.set_visible(true);
    bar.append(&ansi);
    bar.append(&ascii);
    bar.append(&json);
    CharsetBar { bar, ansi, ascii, json }
}

fn bar_button(
    label: &str,
    make: impl Fn() -> super::msg::Msg + 'static,
    sender: &relm4::Sender<super::msg::Msg>,
) -> gtk::Button {
    let button = hint_button(label, make, sender);
    button.set_focus_on_click(false);
    button
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
    on_click(&button, move || tx.emit(make()));
    button
}

fn drive_bar(
    sender: &relm4::Sender<super::msg::Msg>,
    active: &Rc<Cell<usize>>,
) -> (gtk::Box, gtk::Box, Vec<(PathBuf, gtk::Button)>) {
    let mounts = rust_commander::places::mounts();
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    bar.add_css_class("drives");
    bar.set_margin_top(2);
    let mut drives = Vec::new();
    for (index, mount) in mounts.into_iter().enumerate() {
        let button = gtk::Button::new();
        button.add_css_class("drive");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        let number = gtk::Label::new(Some(&(index + 1).to_string()));
        number.add_css_class("drive-num");
        row.append(&number);
        row.append(&drive_icon(mount.removable, mount.network));
        let caption = gtk::Label::new(Some(&mount.label));
        caption.set_ellipsize(gtk::pango::EllipsizeMode::End);
        caption.set_max_width_chars(10);
        row.append(&caption);
        button.set_child(Some(&row));
        let shortcut = if index < 9 {
            format!("Ctrl+{}", index + 1)
        } else {
            String::new()
        };
        let tip = if shortcut.is_empty() {
            format!("{}  {}", mount.device, mount.path.display())
        } else {
            format!("{shortcut}  {}  {}", mount.device, mount.path.display())
        };
        button.set_tooltip_text(Some(&tip));
        button.set_focus_on_click(false);
        let tx = sender.clone();
        let path = mount.path.clone();
        drives.push((mount.path, button.clone()));
        let active = active.clone();
        button.connect_clicked(move |_| tx.emit(super::msg::Msg::Mount(active.get(), path.clone())));
        bar.append(&button);
    }
    let shells = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    shells.set_margin_start(8);
    shells.set_visible(false);
    bar.append(&shells);
    bar.set_visible(!drives.is_empty());
    (bar, shells, drives)
}

pub fn blink_drive(button: &gtk::Button) {
    blink_button(button);
}

/// Blinks the button and runs `action` once the blink is over, so it stays visible even when the action closes the window.
pub fn on_click(button: &gtk::Button, action: impl Fn() + 'static) {
    let action = Rc::new(action);
    let pending = Rc::new(Cell::new(false));
    button.connect_clicked(move |button| {
        if !FLASH_ENABLED.load(Ordering::Relaxed) {
            action();
            return;
        }
        if pending.replace(true) {
            return;
        }
        blink_button(button);
        let (action, pending, button) = (Rc::clone(&action), Rc::clone(&pending), button.downgrade());
        gtk::glib::timeout_add_local_once(Duration::from_millis(240), move || {
            pending.set(false);
            if button.upgrade().is_some_and(|button| button.is_mapped()) {
                action();
            }
        });
    });
}

pub fn blink_button(button: &gtk::Button) {
    blink_widget(button.upcast_ref());
}

pub fn blink_widget(widget: &gtk::Widget) {
    if !FLASH_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let widget = widget.clone();
    widget.add_css_class("flash");
    let step = Cell::new(0u8);
    gtk::glib::timeout_add_local(Duration::from_millis(80), move || {
        let n = step.get() + 1;
        step.set(n);
        if n % 2 == 0 {
            widget.add_css_class("flash");
        } else {
            widget.remove_css_class("flash");
        }
        if n >= 3 {
            widget.remove_css_class("flash");
            gtk::glib::ControlFlow::Break
        } else {
            gtk::glib::ControlFlow::Continue
        }
    });
}

pub fn set_flash_enabled(enabled: bool) {
    FLASH_ENABLED.store(enabled, Ordering::Relaxed);
}

fn set_logo(window: &gtk::Window) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("img/rustcommander_no white.png");
    let Ok(texture) = gtk::gdk::Texture::from_filename(path) else { return };
    window.connect_realize(move |window| {
        let Some(surface) = window.surface() else { return };
        let Ok(toplevel) = surface.downcast::<gtk::gdk::Toplevel>() else { return };
        toplevel.set_icon_list(&[texture.clone()]);
    });
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

fn pointer_buttons_down() -> bool {
    let Some(pointer) = gtk::gdk::Display::default().and_then(|display| display.default_seat()).and_then(|seat| seat.pointer()) else {
        return false;
    };
    let mods = pointer.modifier_state();
    mods.contains(gtk::gdk::ModifierType::BUTTON1_MASK)
        || mods.contains(gtk::gdk::ModifierType::BUTTON2_MASK)
        || mods.contains(gtk::gdk::ModifierType::BUTTON3_MASK)
}

fn bind_header_clicks(
    list: &gtk::ColumnView,
    index: usize,
    sender: &relm4::Sender<super::msg::Msg>,
    active: &Rc<Cell<usize>>,
) {
    let keys = [SortKey::Name, SortKey::Ext, SortKey::Size, SortKey::Time, SortKey::Attr];
    let sender = sender.clone();
    let active = active.clone();
    let bound = std::rc::Rc::new(std::cell::Cell::new(false));
    let list_for_map = list.clone();
    let try_bind = {
        let bound = std::rc::Rc::clone(&bound);
        let active = active.clone();
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
                let flash = title.clone();
                let active = active.clone();
                click.connect_pressed(move |gesture, n, _, _| {
                    if n == 1 {
                        active.set(index);
                        blink_widget(&flash);
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

fn edit_command(key: gtk::gdk::Key, mods: gtk::gdk::ModifierType) -> bool {
    use gtk::gdk::Key;
    let ctrl = mods.contains(gtk::gdk::ModifierType::CONTROL_MASK);
    let alt = mods.contains(gtk::gdk::ModifierType::ALT_MASK);
    // Ctrl+V leaves for view; Ctrl+E is reserved so typing stays free.
    key == Key::Escape
        || (ctrl && !alt && matches!(key, Key::s | Key::S | Key::v | Key::V | Key::e | Key::E))
}

fn terminal_command(key: gtk::gdk::Key, mods: gtk::gdk::ModifierType) -> bool {
    use gtk::gdk::Key;
    let ctrl = mods.contains(gtk::gdk::ModifierType::CONTROL_MASK);
    let alt = mods.contains(gtk::gdk::ModifierType::ALT_MASK);
    // Escape stays in the shell (vim, fzf). F9 toggles back to the file list.
    (!ctrl && !alt && matches!(key, Key::F9))
        || (ctrl && !alt && matches!(key, Key::q | Key::Q | Key::w | Key::W | Key::F9))
        || (alt && !ctrl && is_digit_key(key))
}

fn is_digit_key(key: gtk::gdk::Key) -> bool {
    use gtk::gdk::Key;
    matches!(
        key,
        Key::_1 | Key::KP_1 | Key::_2 | Key::KP_2 | Key::_3 | Key::KP_3 | Key::_4 | Key::KP_4 | Key::_5 | Key::KP_5
            | Key::_6 | Key::KP_6 | Key::_7 | Key::KP_7 | Key::_8 | Key::KP_8 | Key::_9 | Key::KP_9
    )
}

fn entry_input(key: gtk::gdk::Key, mods: gtk::gdk::ModifierType) -> bool {
    use gtk::gdk::Key;
    let ctrl = mods.contains(gtk::gdk::ModifierType::CONTROL_MASK);
    let alt = mods.contains(gtk::gdk::ModifierType::ALT_MASK);
    !is_command(key, mods)
        || (!alt && matches!(key, Key::Left | Key::Right | Key::KP_Left | Key::KP_Right | Key::Home | Key::End | Key::KP_Home | Key::KP_End | Key::Delete))
        || (ctrl && !alt && matches!(key, Key::a | Key::A | Key::c | Key::C | Key::v | Key::V | Key::x | Key::X | Key::z | Key::Z))
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
    let path = gtk::Entry::new();
    path.add_css_class("path");
    path.set_has_frame(false);
    path.set_hexpand(true);
    let tx = sender.clone();
    path.connect_activate(move |entry| {
        tx.emit(super::msg::Msg::PathGo(index, entry.text().to_string()));
    });
    let path_focus = path.clone();
    let tx = sender.clone();
    let path_active = active_pane.clone();
    let focus = gtk::EventControllerFocus::new();
    focus.connect_enter(move |_| {
        path_active.set(index);
        tx.emit(super::msg::Msg::FocusPane(index));
        // The click moves the cursor after focus; select the whole path once that is done.
        let entry = path_focus.clone();
        gtk::glib::idle_add_local_once(move || {
            if entry.has_focus() {
                entry.select_region(0, -1);
            }
        });
    });
    path.add_controller(focus);
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
    bind_header_clicks(&list, index, sender, active_pane);
    let tx = sender.clone();
    list.connect_activate(move |_, _| tx.emit(super::msg::Msg::Activate(index)));
    // Already-selected rows do not notify, so a click in Ext/Size/Date/Attr must still activate the pane.
    let column_click = gtk::GestureClick::new();
    column_click.set_propagation_phase(gtk::PropagationPhase::Capture);
    column_click.set_exclusive(false);
    column_click.set_button(0);
    let tx = sender.clone();
    let column_active = active_pane.clone();
    column_click.connect_pressed(move |_, _, _, _| {
        column_active.set(index);
        tx.emit(super::msg::Msg::FocusPane(index));
    });
    list.add_controller(column_click);
    let focus = gtk::EventControllerFocus::new();
    let tx = sender.clone();
    let focus_active = active_pane.clone();
    focus.connect_enter(move |_| {
        // A click in the other pane can move focus here while the button is still down.
        if pointer_buttons_down() && focus_active.get() != index {
            return;
        }
        tx.emit(super::msg::Msg::FocusPane(index));
    });
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
    let terminal_slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
    terminal_slot.set_hexpand(true);
    terminal_slot.set_vexpand(true);
    let stack = gtk::Stack::new();
    stack.add_named(&scroll, Some("list"));
    stack.add_named(&preview_kind, Some("preview"));
    stack.add_named(&terminal_slot, Some("terminal"));
    stack.set_vexpand(true);
    column.append(&tabs);
    column.append(&path);
    column.append(&filter);
    column.append(&stack);
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
        terminal_slot,
    }
}

pub fn paint_terminal(term: &Terminal, theme: &rust_commander::theme::TerminalTheme) {
    let Ok(fg) = gtk::gdk::RGBA::parse(&theme.foreground) else { return };
    let Ok(bg) = gtk::gdk::RGBA::parse(&theme.background) else { return };
    let ansi: Vec<gtk::gdk::RGBA> = theme.ansi.iter().filter_map(|hex| gtk::gdk::RGBA::parse(hex).ok()).collect();
    if ansi.len() != 16 {
        return;
    }
    let refs: Vec<&gtk::gdk::RGBA> = ansi.iter().collect();
    term.set_colors(Some(&fg), Some(&bg), &refs);
    term.set_color_bold(None);
    if let Ok(cursor) = gtk::gdk::RGBA::parse(&theme.cursor) {
        term.set_color_cursor(Some(&cursor));
        term.set_color_cursor_foreground(Some(&bg));
    }
    if let Ok(sel) = gtk::gdk::RGBA::parse(&theme.selection) {
        term.set_color_highlight(Some(&sel));
        term.set_color_highlight_foreground(Some(&fg));
    }
}

#[cfg(test)]
mod tests {
    use super::entry_input;
    use relm4::gtk::gdk::{Key, ModifierType};

    #[test]
    fn filter_entry_receives_text_but_not_search_shortcuts() {
        for key in [Key::e, Key::d, Key::i, Key::t, Key::period, Key::x] {
            assert!(entry_input(key, ModifierType::empty()));
        }
        assert!(entry_input(Key::Left, ModifierType::empty()));
        assert!(entry_input(Key::v, ModifierType::CONTROL_MASK));
        assert!(!entry_input(Key::s, ModifierType::CONTROL_MASK));
        assert!(!entry_input(Key::S, ModifierType::CONTROL_MASK | ModifierType::SHIFT_MASK));
        assert!(!entry_input(Key::Escape, ModifierType::empty()));
    }
}

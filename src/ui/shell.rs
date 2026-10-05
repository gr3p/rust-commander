use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use relm4::gtk;
use relm4::gtk::gio;
use relm4::gtk::prelude::*;
use relm4::RelmWidgetExt;
use vte4::prelude::{TerminalExt, TerminalExtManual};
use vte4::Terminal;

use rust_commander::commands::Act;
use rust_commander::icons;
use rust_commander::listing::SortKey;

use super::iconpack;

use super::row::{self, EntryObject};

static FLASH_ENABLED: AtomicBool = AtomicBool::new(true);
static PANE_BLINK: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

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
    pub paned: gtk::Paned,
    pub status: gtk::Label,
    pub css: gtk::CssProvider,
    pub f_labels: Vec<String>,
    pub mode_bar: gtk::Stack,
    pub image_scale: gtk::Button,
    pub text_modes: CharsetBar,
    pub hints: [Hint; 6],
    pub active: Rc<Cell<usize>>,
    pub drives: Vec<(PathBuf, gtk::Button)>,
    pub idle_drives: Vec<gtk::Button>,
    pub drive_snapshot: DriveSnapshot,
    pub edit_button: gtk::Button,
    pub save_button: gtk::Button,
    pub discard_button: gtk::Button,
    pub editing: Rc<Cell<bool>>,
    pub shell_key: gtk::Label,
    pub shell_bar: gtk::Box,
    pub shell_buttons: gtk::Box,
    pub shells: Vec<PanelShell>,
    pub shell_current: Option<u64>,
    pub shell_seq: u64,
}

pub struct Hint {
    pub button: gtk::Button,
    pub act: Rc<Cell<Option<Act>>>,
}

pub struct PanelShell {
    pub id: u64,
    pub term: Terminal,
    pub button: gtk::Button,
    pub number: gtk::Label,
    pub pid: std::rc::Rc<std::cell::Cell<Option<i32>>>,
}

pub fn build(window: &gtk::Window, sender: &relm4::Sender<super::msg::Msg>, icon_pack: &str) -> Shell {
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
    let (shell_bar, shell_buttons, drives, idle_drives, drive_snapshot) = drive_bar(sender, &active, &iconpack::active(icon_pack));
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
    // Keep the divider centred on every window resize.
    let last_width = Cell::new(0);
    let tx_tick = sender.clone();
    paned.add_tick_callback(move |paned, _| {
        let width = paned.width();
        if width != last_width.get() {
            last_width.set(width);
            if width > 0 {
                paned.set_position(width / 2);
                tx_tick.emit(super::msg::Msg::PanesResized);
            }
        }
        gtk::glib::ControlFlow::Continue
    });
    let tx = sender.clone();
    paned.connect_position_notify(move |_| tx.emit(super::msg::Msg::PanesResized));
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
    let hints = std::array::from_fn(|_| hint_slot(sender));
    for hint in &hints {
        context_row.append(&hint.button);
    }
    let all = gtk::Button::with_label("F1 All");
    all.add_css_class("fkey");
    all.set_hexpand(true);
    all.set_focus_on_click(false);
    if let Some(label) = all.child().and_then(|child| child.downcast::<gtk::Label>().ok()) {
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    }
    let tx = sender.clone();
    on_click(&all, move || tx.emit(super::msg::Msg::Key(gtk::gdk::Key::F1, gtk::gdk::ModifierType::empty())));
    context_row.append(&all);
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
    let edit_button = bar_button(&rust_commander::commands::caption("Ctrl+E", "Edit"), || super::msg::Msg::ToggleQuickEdit, sender);
    let save_button = bar_button(&rust_commander::commands::caption("Ctrl+S", "Save"), || super::msg::Msg::SaveQuick, sender);
    let discard_button = bar_button(&rust_commander::commands::caption("Ctrl+D", "Discard"), || super::msg::Msg::DiscardQuick, sender);
    text_modes.bar.append(&edit_button);
    text_modes.bar.append(&save_button);
    text_modes.bar.append(&discard_button);
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
        paned,
        status,
        css,
        f_labels,
        mode_bar,
        image_scale,
        text_modes,
        hints,
        active,
        drives,
        idle_drives,
        drive_snapshot,
        edit_button,
        save_button,
        discard_button,
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

fn hint_slot(sender: &relm4::Sender<super::msg::Msg>) -> Hint {
    let act = Rc::new(Cell::new(None));
    let button = gtk::Button::with_label("");
    button.add_css_class("fkey");
    button.set_hexpand(true);
    button.set_visible(false);
    button.set_focus_on_click(false);
    if let Some(label) = button.child().and_then(|child| child.downcast::<gtk::Label>().ok()) {
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    }
    let tx = sender.clone();
    let slot = Rc::clone(&act);
    on_click(&button, move || {
        if let Some(act) = slot.get() {
            tx.emit(super::msg::Msg::Run(act));
        }
    });
    Hint { button, act }
}

pub fn shows(hints: &[Hint], act: Act) -> bool {
    hints.iter().any(|hint| hint.button.is_visible() && hint.act.get() == Some(act))
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
    icon_pack: &str,
) -> (gtk::Box, gtk::Box, Vec<(PathBuf, gtk::Button)>, Vec<gtk::Button>, DriveSnapshot) {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    bar.add_css_class("drives");
    bar.set_margin_top(2);
    let snapshot = (rust_commander::places::mounts(), super::drives::idle_volumes());
    let (drives, idle) = drive_buttons(sender, active, icon_pack, &snapshot);
    for button in drives.iter().map(|(_, button)| button).chain(idle.iter()) {
        bar.append(button);
    }
    let shells = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    shells.set_margin_start(8);
    shells.set_visible(false);
    bar.append(&shells);
    bar.set_visible(!drives.is_empty() || !idle.is_empty());
    (bar, shells, drives, idle, snapshot)
}

pub type DriveSnapshot = (Vec<rust_commander::places::Mount>, Vec<super::drives::Idle>);

/// Rebuilds the drive buttons when the set of drives changed. Returns the mount points that went away.
pub fn rebuild_drives(
    shell: &mut Shell,
    sender: &relm4::Sender<super::msg::Msg>,
    icon_pack: &str,
) -> Option<Vec<PathBuf>> {
    let snapshot = (rust_commander::places::mounts(), super::drives::idle_volumes());
    if snapshot == shell.drive_snapshot {
        return None;
    }
    let gone = shell
        .drive_snapshot
        .0
        .iter()
        .filter(|old| !snapshot.0.iter().any(|new| new.path == old.path))
        .map(|old| old.path.clone())
        .collect();
    for button in shell.drives.drain(..).map(|(_, button)| button).chain(shell.idle_drives.drain(..)) {
        shell.shell_bar.remove(&button);
    }
    let (drives, idle) = drive_buttons(sender, &shell.active, &iconpack::active(icon_pack), &snapshot);
    let mut prev: Option<gtk::Widget> = None;
    for button in drives.iter().map(|(_, button)| button).chain(idle.iter()) {
        shell.shell_bar.insert_child_after(button, prev.as_ref());
        prev = Some(button.clone().upcast());
    }
    shell.drives = drives;
    shell.idle_drives = idle;
    shell.drive_snapshot = snapshot;
    Some(gone)
}

fn drive_buttons(
    sender: &relm4::Sender<super::msg::Msg>,
    active: &Rc<Cell<usize>>,
    icon_pack: &str,
    (mounts, idle): &DriveSnapshot,
) -> (Vec<(PathBuf, gtk::Button)>, Vec<gtk::Button>) {
    use super::msg::Msg;
    let mut drives = Vec::new();
    for (index, mount) in mounts.iter().enumerate() {
        let button = gtk::Button::new();
        button.add_css_class("drive");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let number = gtk::Label::new(Some(&(index + 1).to_string()));
        number.add_css_class("drive-num");
        row.append(&number);
        row.append(&iconpack::icon_widget(
            &icons::drive_token(icon_pack, mount.removable, mount.network, mount.optical),
            16,
        ));
        let caption = gtk::Label::new(Some(&mount.label));
        caption.set_ellipsize(gtk::pango::EllipsizeMode::End);
        caption.set_max_width_chars(10);
        row.append(&caption);
        button.set_child(Some(&row));
        let shortcut = if index < 9 {
            rust_commander::commands::format_keys(&format!("Ctrl+{}", index + 1))
        } else {
            String::new()
        };
        let tip = if shortcut.is_empty() {
            format!("{}  {}", mount.device, mount.path.display())
        } else {
            format!("{shortcut}  {}  {}", mount.device, mount.path.display())
        };
        // Free space changes while the program runs, so it is read each time the tip opens.
        button.set_has_tooltip(true);
        let mount_path = mount.path.clone();
        button.connect_query_tooltip(move |_, _, _, _, tooltip| {
            let space = rust_commander::places::disk_info(&mount_path)
                .map(|info| rust_commander::places::summary(&info))
                .unwrap_or_default();
            tooltip.set_text(Some(&format!("{tip}\n{space}")));
            true
        });
        let menu_click = gtk::GestureClick::new();
        menu_click.set_button(gtk::gdk::BUTTON_SECONDARY);
        let tx_menu = sender.clone();
        let menu_path = mount.path.clone();
        menu_click.connect_pressed(move |gesture, _, _, _| {
            let Some(button) = gesture.widget() else { return };
            let can = super::drives::actions(&menu_path);
            let mut items: Vec<(&str, Box<dyn Fn()>)> = Vec::new();
            let (tx, path) = (tx_menu.clone(), menu_path.clone());
            items.push(("Properties", Box::new(move || tx.emit(Msg::DiskInfo(Some(path.clone()))))));
            if can.unmount {
                let (tx, path) = (tx_menu.clone(), menu_path.clone());
                items.push(("Unmount", Box::new(move || tx.emit(Msg::Unmount(path.clone(), false)))));
            }
            if can.eject {
                let (tx, path) = (tx_menu.clone(), menu_path.clone());
                items.push(("Eject", Box::new(move || tx.emit(Msg::Unmount(path.clone(), true)))));
            }
            popup_menu(&button, items);
        });
        button.add_controller(menu_click);
        button.set_focus_on_click(false);
        let tx = sender.clone();
        let path = mount.path.clone();
        drives.push((mount.path.clone(), button.clone()));
        let active = active.clone();
        button.connect_clicked(move |_| tx.emit(Msg::Mount(active.get(), path.clone())));
    }
    let mut idles = Vec::new();
    for volume in idle {
        let button = gtk::Button::new();
        button.add_css_class("drive");
        button.add_css_class("idle");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.append(&iconpack::icon_widget(&icons::drive_token(icon_pack, true, false, volume.optical), 16));
        let caption = gtk::Label::new(Some(&volume.label));
        caption.set_ellipsize(gtk::pango::EllipsizeMode::End);
        caption.set_max_width_chars(10);
        row.append(&caption);
        button.set_child(Some(&row));
        button.set_tooltip_text(Some(&format!("{}  Click to mount", volume.id)));
        button.set_focus_on_click(false);
        let tx = sender.clone();
        let (id, pane) = (volume.id.clone(), active.clone());
        button.connect_clicked(move |_| tx.emit(Msg::MountVolume(pane.get(), id.clone())));
        let menu_click = gtk::GestureClick::new();
        menu_click.set_button(gtk::gdk::BUTTON_SECONDARY);
        let tx_menu = sender.clone();
        let (id, pane) = (volume.id.clone(), active.clone());
        menu_click.connect_pressed(move |gesture, _, _, _| {
            let Some(button) = gesture.widget() else { return };
            let (tx, mount_id, pane) = (tx_menu.clone(), id.clone(), pane.get());
            let mut items: Vec<(&str, Box<dyn Fn()>)> =
                vec![("Mount", Box::new(move || tx.emit(Msg::MountVolume(pane, mount_id.clone()))))];
            if super::drives::idle_can_eject(&id) {
                let (tx, eject_id) = (tx_menu.clone(), id.clone());
                items.push(("Eject", Box::new(move || tx.emit(Msg::EjectVolume(eject_id.clone())))));
            }
            popup_menu(&button, items);
        });
        button.add_controller(menu_click);
        idles.push(button);
    }
    (drives, idles)
}

fn popup_menu(anchor: &gtk::Widget, items: Vec<(&str, Box<dyn Fn()>)>) {
    let menu = gtk::Popover::new();
    let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
    column.set_margin_all(4);
    for (label, action) in items {
        let button = gtk::Button::with_label(label);
        button.add_css_class("fkey");
        button.set_focus_on_click(false);
        let menu = menu.clone();
        button.connect_clicked(move |_| {
            menu.popdown();
            action();
        });
        column.append(&button);
    }
    menu.set_child(Some(&column));
    menu.set_parent(anchor);
    menu.connect_closed(|menu| {
        let menu = menu.clone();
        gtk::glib::idle_add_local_once(move || menu.unparent());
    });
    menu.popup();
}

pub fn blink_drive(button: &gtk::Button) {
    blink_button(button);
}

/// Blinks the button and runs `action` once the blink is over, so it stays visible even when the action closes the window.
pub fn on_escape(win: &gtk::Window, action: impl Fn() + 'static) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed(move |_, key, _, mods| {
        if mods.contains(gtk::gdk::ModifierType::CONTROL_MASK) || mods.contains(gtk::gdk::ModifierType::ALT_MASK) {
            return gtk::glib::Propagation::Proceed;
        }
        if key == gtk::gdk::Key::Escape {
            action();
            return gtk::glib::Propagation::Stop;
        }
        gtk::glib::Propagation::Proceed
    });
    win.add_controller(keys);
}

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

/// Blinks the widget, then runs `action` once the blink has been seen.
pub fn blink_then(widget: &gtk::Widget, action: impl FnOnce() + 'static) {
    if !FLASH_ENABLED.load(Ordering::Relaxed) {
        action();
        return;
    }
    blink_widget(widget);
    gtk::glib::timeout_add_local_once(Duration::from_millis(240), action);
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

pub fn blink_frame(frame: &gtk::Frame, pane: usize, only_while_active: bool) {
    if !FLASH_ENABLED.load(Ordering::Relaxed) || pane > 1 {
        return;
    }
    let token = PANE_BLINK[pane].fetch_add(1, Ordering::Relaxed) + 1;
    let frame = frame.clone();
    frame.add_css_class("pane-blink");
    let step = Cell::new(0u8);
    gtk::glib::timeout_add_local(Duration::from_millis(80), move || {
        if PANE_BLINK[pane].load(Ordering::Relaxed) != token {
            return gtk::glib::ControlFlow::Break;
        }
        let n = step.get() + 1;
        step.set(n);
        // A pane that lost the accent must not flash again. The shell pane never has it.
        let show = n % 2 == 0 && (!only_while_active || frame.has_css_class("pane-active"));
        if show {
            frame.add_css_class("pane-blink");
        } else {
            frame.remove_css_class("pane-blink");
        }
        if n >= 3 {
            frame.remove_css_class("pane-blink");
            gtk::glib::ControlFlow::Break
        } else {
            gtk::glib::ControlFlow::Continue
        }
    });
}

pub fn set_flash_enabled(enabled: bool) {
    FLASH_ENABLED.store(enabled, Ordering::Relaxed);
}

const LOGO_PNG: &[u8] = include_bytes!("../../img/rust-commander.png");
const APP_ID: &str = "se.rustcommander.RustCommander";

fn set_logo(window: &gtk::Window) {
    // GNOME Shell ignores the toplevel icon and reads the desktop file instead.
    publish_logo();
    let bytes = gtk::glib::Bytes::from_static(LOGO_PNG);
    let Ok(texture) = gtk::gdk::Texture::from_bytes(&bytes) else { return };
    window.connect_realize(move |window| {
        let Some(surface) = window.surface() else { return };
        let Ok(toplevel) = surface.downcast::<gtk::gdk::Toplevel>() else { return };
        toplevel.set_icon_list(&[texture.clone()]);
    });
}

// GLib 2.88 drops a desktop file whose Exec is not an existing binary, and the shell then shows a gear.
// The shell scans the icon theme only when it starts, so Icon= is a file path. The hash in the name
// makes a new logo a new path, which the shell's texture cache has never seen.
fn publish_logo() {
    let Some(data) = data_home() else { return };
    let dir = data.join("rust-commander");
    let name = format!("{APP_ID}-{:016x}.png", fnv1a(LOGO_PNG));
    let icon = dir.join(&name);
    if let Err(err) = write_bytes(&icon, LOGO_PNG) {
        rust_commander::log::error(&format!("logo icon {}: {err}", icon.display()));
        return;
    }
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let old = entry.file_name();
        let old = old.to_string_lossy();
        if old.starts_with(APP_ID) && old.ends_with(".png") && old != name {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    for stale in ["rust-commander.png", "se.rustcommander.RustCommander.png"] {
        let _ = std::fs::remove_file(data.join("icons/hicolor/512x512/apps").join(stale));
    }
    let Some(exe) = std::env::current_exe().ok().filter(|path| path.is_file()) else { return };
    let desktop = data.join("applications").join(format!("{APP_ID}.desktop"));
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=Rust Commander\nComment=Dual-pane file manager\nExec={}\nIcon={}\nTerminal=false\nCategories=System;FileManager;\nStartupWMClass={APP_ID}\n",
        desktop_exec(&exe),
        icon.display()
    );
    if let Err(err) = write_bytes(&desktop, entry.as_bytes()) {
        rust_commander::log::error(&format!("logo desktop {}: {err}", desktop.display()));
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| (hash ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

fn data_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    Some(PathBuf::from(std::env::var_os("HOME")?).join(".local/share"))
}

fn desktop_exec(path: &std::path::Path) -> String {
    let text = path.to_string_lossy();
    if text.contains([' ', '\t', '"', '\\']) {
        format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        text.into_owned()
    }
}

fn write_bytes(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    if std::fs::read(path).is_ok_and(|old| old == bytes) {
        return Ok(());
    }
    let parent = path.parent().ok_or_else(|| std::io::Error::other("no parent"))?;
    std::fs::create_dir_all(parent)?;
    let mut tmp_name = path.as_os_str().to_os_string();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);
    let written = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        use std::io::Write;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(err) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    if let Err(err) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    Ok(())
}

pub fn repaint_drives(drives: &[(PathBuf, gtk::Button)], icon_pack: &str) {
    let icon_pack = iconpack::active(icon_pack);
    let mounts = rust_commander::places::mounts();
    for (path, button) in drives {
        let Some(row) = button.child().and_then(|child| child.downcast::<gtk::Box>().ok()) else { continue };
        let Some(number) = row.first_child() else { continue };
        let Some(old) = number.next_sibling() else { continue };
        let mount = mounts.iter().find(|mount| &mount.path == path);
        let (removable, network, optical) =
            mount.map(|mount| (mount.removable, mount.network, mount.optical)).unwrap_or_default();
        let next = iconpack::icon_widget(&icons::drive_token(&icon_pack, removable, network, optical), 16);
        row.insert_child_after(&next, Some(&number));
        row.remove(&old);
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
    // Escape and Ctrl+V leave edit. Ctrl+Q leaves quick view once the text is saved.
    // Ctrl+D discards. Ctrl+S saves. Ctrl+Y toggles syntax. Ctrl+L edits the path. Ctrl+E stays out of the text. F1 opens commands.
    (!ctrl && !alt && key == Key::F1)
        || key == Key::Escape
        || (ctrl && !alt && matches!(key, Key::s | Key::S | Key::v | Key::V | Key::e | Key::E | Key::d | Key::D | Key::q | Key::Q | Key::y | Key::Y | Key::l | Key::L))
}

fn terminal_command(key: gtk::gdk::Key, mods: gtk::gdk::ModifierType) -> bool {
    use gtk::gdk::Key;
    let ctrl = mods.contains(gtk::gdk::ModifierType::CONTROL_MASK);
    let alt = mods.contains(gtk::gdk::ModifierType::ALT_MASK);
    // Escape stays in the shell (vim, fzf). F9 toggles the file list. F1 opens commands.
    // Tab switches to the file pane. Completion inside the shell is given up.
    (!ctrl && !alt && matches!(key, Key::F1 | Key::F9 | Key::Tab | Key::ISO_Left_Tab))
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

fn image_menu(sender: &relm4::Sender<super::msg::Msg>) -> gtk::Popover {
    let menu = gtk::Popover::new();
    let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
    column.set_margin_all(4);
    let copy_path = gtk::Button::with_label("Copy path");
    let copy_image = gtk::Button::with_label("Copy image");
    copy_path.add_css_class("fkey");
    copy_image.add_css_class("fkey");
    copy_path.set_focus_on_click(false);
    copy_image.set_focus_on_click(false);
    let path_menu = menu.clone();
    let tx = sender.clone();
    copy_path.connect_clicked(move |_| {
        path_menu.popdown();
        tx.emit(super::msg::Msg::CopyPreviewPath);
    });
    let image_menu = menu.clone();
    let tx = sender.clone();
    copy_image.connect_clicked(move |_| {
        image_menu.popdown();
        tx.emit(super::msg::Msg::CopyPreviewImage);
    });
    column.append(&copy_path);
    column.append(&copy_image);
    menu.set_child(Some(&column));
    menu
}

fn hide_path_copy(path: &gtk::Entry, copy: &gtk::Button) {
    let path = path.clone();
    let copy = copy.clone();
    // Focus moves to the button before this runs, so a click or Tab does not hide it first.
    gtk::glib::idle_add_local_once(move || {
        if !path.has_focus() && !copy.has_focus() {
            copy.set_visible(false);
        }
    });
}

fn under(widget: &gtk::Widget, ancestor: &gtk::Widget) -> bool {
    let mut current = Some(widget.clone());
    while let Some(widget) = current {
        if &widget == ancestor {
            return true;
        }
        current = widget.parent();
    }
    false
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
    let copy = gtk::Button::with_label("Copy path");
    copy.add_css_class("fkey");
    copy.set_can_focus(false);
    copy.set_focus_on_click(false);
    copy.set_visible(false);
    let copy_show = copy.clone();
    let path_focus = path.clone();
    let tx = sender.clone();
    let path_active = active_pane.clone();
    let focus = gtk::EventControllerFocus::new();
    focus.connect_enter(move |_| {
        copy_show.set_visible(true);
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
    let path_for_hide = path.clone();
    let copy_for_hide = copy.clone();
    focus.connect_leave(move |_| hide_path_copy(&path_for_hide, &copy_for_hide));
    path.add_controller(focus);
    let tx = sender.clone();
    on_click(&copy, move || tx.emit(super::msg::Msg::CopyShownPath(index)));
    let path_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    path_row.append(&path);
    path_row.append(&copy);
    let filter = gtk::Entry::new();
    filter.add_css_class("filter");
    filter.set_has_frame(false);
    filter.set_placeholder_text(Some("Filter"));
    filter.set_visible(false);
    let tx = sender.clone();
    filter.connect_changed(move |entry| {
        tx.emit(super::msg::Msg::FilterChanged(index, entry.text().to_string()));
    });
    let focus = gtk::EventControllerFocus::new();
    let filter_leave = filter.clone();
    focus.connect_leave(move |_| {
        if filter_leave.text().is_empty() {
            filter_leave.set_visible(false);
        }
    });
    filter.add_controller(focus);
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
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_child(Some(&list));
    scroll.set_vexpand(true);
    // Scrollbars get their own space instead of covering the rows.
    scroll.set_overlay_scrolling(false);
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
    picture.set_focusable(true);
    let picture_scroll = gtk::ScrolledWindow::new();
    picture_scroll.set_child(Some(&picture));
    picture_scroll.set_hexpand(true);
    picture_scroll.set_vexpand(true);
    picture_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
    let preview_kind = gtk::Stack::new();
    preview_kind.add_named(&preview_scroll, Some("text"));
    preview_kind.add_named(&picture_scroll, Some("image"));
    preview_kind.set_vexpand(true);
    let menu = image_menu(sender);
    menu.set_parent(&frame);
    let terminal_slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
    terminal_slot.set_hexpand(true);
    terminal_slot.set_vexpand(true);
    let watch = gtk::EventControllerLegacy::new();
    watch.set_propagation_phase(gtk::PropagationPhase::Capture);
    let tx = sender.clone();
    let active_pane = active_pane.clone();
    let frame_hit = frame.clone();
    let preview_root: gtk::Widget = preview_kind.clone().upcast();
    let terminal_root: gtk::Widget = terminal_slot.clone().upcast();
    let picture_focus = picture.clone();
    let preview_page = preview_kind.clone();
    watch.connect_event(move |_, event| {
        if event.event_type() != gtk::gdk::EventType::ButtonPress {
            return gtk::glib::Propagation::Proceed;
        }
        // Event positions are in surface space. pick and the popover want frame space.
        let local = event.position().and_then(|(x, y)| {
            let native = frame_hit.native()?;
            let (sx, sy) = native.surface_transform();
            let point = gtk::graphene::Point::new((x - sx) as f32, (y - sy) as f32);
            native.compute_point(&frame_hit, &point).map(|p| (p.x() as f64, p.y() as f64))
        });
        let hit = |root: &gtk::Widget| {
            local.is_some_and(|(x, y)| frame_hit.pick(x, y, gtk::PickFlags::DEFAULT).is_some_and(|hit| under(&hit, root)))
        };
        // A click in the shell may select text. It must not activate that pane, or the shell moves.
        if hit(&terminal_root) {
            return gtk::glib::Propagation::Proceed;
        }
        // The preview is not a file pane. A click there must not hide it.
        let in_preview = hit(&preview_root);
        if !in_preview {
            active_pane.set(index);
            tx.emit(super::msg::Msg::FocusPane(index));
            return gtk::glib::Propagation::Proceed;
        }
        let image = preview_page.visible_child_name().as_deref() == Some("image");
        let button = event.downcast_ref::<gtk::gdk::ButtonEvent>().map(|event| event.button()).unwrap_or(1);
        if image && (button == 1 || button == 3) {
            picture_focus.grab_focus();
        }
        if image && button == 3 {
            if let Some((x, y)) = local {
                menu.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
                menu.popup();
            }
            return gtk::glib::Propagation::Stop;
        }
        gtk::glib::Propagation::Proceed
    });
    frame.add_controller(watch);
    let stack = gtk::Stack::new();
    stack.add_named(&scroll, Some("list"));
    stack.add_named(&preview_kind, Some("preview"));
    stack.add_named(&terminal_slot, Some("terminal"));
    stack.set_vexpand(true);
    column.append(&tabs);
    column.append(&path_row);
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
    use super::{edit_command, entry_input};
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

    #[test]
    fn editor_keeps_escape_and_discard() {
        assert!(edit_command(Key::Escape, ModifierType::empty()));
        assert!(edit_command(Key::d, ModifierType::CONTROL_MASK));
        assert!(edit_command(Key::s, ModifierType::CONTROL_MASK));
        assert!(edit_command(Key::q, ModifierType::CONTROL_MASK));
        assert!(edit_command(Key::y, ModifierType::CONTROL_MASK));
        assert!(edit_command(Key::l, ModifierType::CONTROL_MASK));
        assert!(!edit_command(Key::d, ModifierType::empty()));
        assert!(!edit_command(Key::q, ModifierType::empty()));
    }
}

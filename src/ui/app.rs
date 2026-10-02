use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use relm4::gtk;
use relm4::gtk::gdk::{Key, ModifierType};
use relm4::gtk::prelude::*;
use relm4::prelude::*;
use relm4::RelmWidgetExt;

use rust_commander::config::{self, Bookmark, Config};
use rust_commander::entry::Entry;
use rust_commander::lister::{self, Charset, PreviewBody, ViewMode};
use rust_commander::ops::{self, Choice, Kind, Progress, Report};
use rust_commander::state::{AppState, PaneId};
use rust_commander::theme;

use super::msg::{Msg, SYNCING};

static SIZING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
use super::row::{self, EntryObject};
use super::shell::{self, Shell};

mod commands;
mod jobs;
mod props;
mod queue;
mod search;
mod settings;
mod term;
mod view;

// Targets are taken when the dialog opens, so a cursor move or a click behind it changes nothing.
enum Ask {
    Copy(Vec<PathBuf>),
    Move(Vec<PathBuf>),
    CopyAs(PathBuf),
    Rename(PathBuf),
    Mkdir,
    CreateFile,
    Trash(Vec<PathBuf>),
    Permanent(Vec<PathBuf>),
    Password,
    Overwrite,
}

struct QuickEdit {
    path: PathBuf,
    pane: usize,
    form: lister::TextForm,
    saved: u64,
    armed: Option<u64>,
}

#[derive(Clone, Copy)]
enum Work {
    Transfer,
    Delete,
    Unpack,
    Pack,
    Sfv,
    Verify,
}

enum Phase {
    List,
    Extract,
}

struct PendingUnpack {
    archive: PathBuf,
    dest: PathBuf,
    names: Option<Vec<String>>,
    password: Option<String>,
    asked: u8,
    pick: bool,
    phase: Phase,
}

enum Queued {
    Pack(PathBuf, rust_commander::pack::Options),
    Sfv(PathBuf),
}

struct JobOut {
    cancel: Arc<std::sync::atomic::AtomicBool>,
    progress_tx: mpsc::Sender<Progress>,
    conflict_tx: mpsc::Sender<(PathBuf, PathBuf)>,
    answer_rx: mpsc::Receiver<Choice>,
    done_tx: mpsc::Sender<Result<Report, String>>,
}

struct Job {
    work: Work,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    progress_rx: mpsc::Receiver<Progress>,
    conflict_rx: mpsc::Receiver<(PathBuf, PathBuf)>,
    answer_tx: mpsc::Sender<Choice>,
    done_rx: mpsc::Receiver<Result<Report, String>>,
    started: Instant,
    conflict_open: bool,
}

struct ListerUi {
    window: gtk::Window,
    view: gtk::TextView,
    reader: lister::Reader,
    mode: ViewMode,
    modes: super::shell::CharsetBar,
}

struct ImageUi {
    window: gtk::Window,
    picture: gtk::Picture,
    scroll: gtk::ScrolledWindow,
    bar: gtk::Box,
    scale: gtk::Button,
    files: Vec<PathBuf>,
    index: usize,
    generation: u64,
    token: u64,
    fit: bool,
}

struct SettingsUi {
    window: gtk::Window,
    columns: gtk::Label,
    hidden: gtk::Label,
    hints: gtk::Label,
    icons: gtk::Label,
    flash: gtk::Label,
    syntax: gtk::Label,
    image: gtk::Label,
    clear: gtk::Button,
    stretch: gtk::Label,
    strength_name: gtk::Label,
    strength: gtk::Scale,
    editor: gtk::DropDown,
    editor_ids: Rc<RefCell<Vec<String>>>,
    editor_guard: Rc<Cell<bool>>,
    theme: gtk::DropDown,
    theme_ids: Rc<RefCell<Vec<String>>>,
    theme_guard: Rc<Cell<bool>>,
    icon_pack: gtk::DropDown,
    icon_pack_ids: Rc<RefCell<Vec<String>>>,
    icon_pack_guard: Rc<Cell<bool>>,
}

pub struct App {
    state: AppState,
    shell: Shell,
    cfg: Config,
    status: String,
    dirty: [bool; 2],
    job: Option<Job>,
    ask: Option<Ask>,
    dialog: Option<gtk::Window>,
    progress: Option<(gtk::Window, gtk::Label, gtk::ProgressBar, Option<(gtk::Label, gtk::TextView)>)>,
    meter: Option<f64>,
    pending: Option<PendingUnpack>,
    queued: Option<Queued>,
    queue: Option<queue::QueueUi>,
    lister: Option<ListerUi>,
    lister_gen: u64,
    lister_draw: u64,
    image: Option<ImageUi>,
    image_token: u64,
    image_gen: u64,
    image_live: Arc<AtomicU64>,
    charset: Charset,
    hot: Option<gtk::Window>,
    hot_stack: Vec<Option<usize>>,
    hot_pane: PaneId,
    preview: lister::PreviewSlot,
    preview_sent: u64,
    preview_shown: [u64; 2],
    quick_fit: bool,
    edit_quiet: Rc<Cell<bool>>,
    syntax_on: Rc<Cell<bool>>,
    edit_name: Rc<RefCell<String>>,
    edit_changes: Rc<Cell<u64>>,
    quick_edit: Option<QuickEdit>,
    preview_live: Arc<AtomicU64>,
    copy_gen: u64,
    loads: Vec<(u64, Arc<AtomicBool>)>,
    search: Option<search::SearchUi>,
    search_gen: u64,
    search_target: Option<(PaneId, usize, u64, String)>,
    settings: Option<SettingsUi>,
    command_list: Option<gtk::Window>,
    smoke: bool,
}

pub fn run(smoke: bool) {
    gtk::glib::log_set_writer_func(quiet_gdk_vulkan);
    rust_commander::log::info(&format!(
        "start version={} log={}",
        rust_commander::version(),
        rust_commander::log::path().display()
    ));
    let args: Vec<String> = std::env::args().filter(|arg| arg != "--smoke").collect();
    let app = RelmApp::new("se.rustcommander.RustCommander").with_args(args);
    app.allow_multiple_instances(true);
    app.run::<App>(smoke);
}

// GLIB_DOMAIN and MESSAGE are the fields g_warning writes.
fn quiet_gdk_vulkan(
    level: gtk::glib::LogLevel,
    fields: &[gtk::glib::LogField<'_>],
) -> gtk::glib::LogWriterOutput {
    let mut gdk = false;
    let mut hit = false;
    if level == gtk::glib::LogLevel::Warning {
        for field in fields {
            match (field.key(), field.value_str()) {
                ("GLIB_DOMAIN", Some("Gdk")) => gdk = true,
                ("MESSAGE", Some(text))
                    if text.contains("VK_ERROR_OUT_OF_DATE_KHR")
                        || text.contains("VK_SUBOPTIMAL_KHR") =>
                {
                    hit = true;
                }
                _ => {}
            }
        }
    }
    if gdk && hit {
        return gtk::glib::LogWriterOutput::Handled;
    }
    gtk::glib::log_writer_default(level, fields)
}

#[relm4::component(pub)]
impl SimpleComponent for App {
    type Init = bool;
    type Input = Msg;
    type Output = ();

    view! {
        gtk::Window {
            set_title: Some(rust_commander::app_title()),
            set_default_width: 1200,
            set_default_height: 760,
        }
    }

    fn init(smoke: bool, root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
        let widgets = view_output!();
        let cfg = config::load();
        let shell = shell::build(&root, sender.input_sender(), &cfg.icon_pack);
        shell::set_flash_enabled(cfg.flash_feedback);
        let (left, right) = if smoke {
            smoke_dirs()
        } else {
            let home = home_dir();
            (home.clone(), home)
        };
        let mut state = AppState::new(left, right);
        state.show_hidden = cfg.show_hidden;
        let sort = cfg.sort_key();
        for pane in &mut state.panes {
            pane.sort = sort;
        }
        state.bookmarks = cfg
            .bookmarks
            .iter()
            .map(|b| (b.name.clone(), b.path.clone()))
            .collect();
        let edit_quiet = Rc::new(Cell::new(false));
        let syntax_on = Rc::new(Cell::new(cfg.syntax_highlight));
        let edit_name = Rc::new(RefCell::new(String::new()));
        let edit_changes = Rc::new(Cell::new(0u64));
        let editing = Rc::clone(&shell.editing);
        let window = shell.window.clone();
        for pane in &shell.panes {
            let quiet = Rc::clone(&edit_quiet);
            let editing = Rc::clone(&editing);
            let window = window.clone();
            let name = Rc::clone(&edit_name);
            let changes = Rc::clone(&edit_changes);
            let syntax_on = Rc::clone(&syntax_on);
            let preview = pane.preview.clone();
            pane.preview.buffer().connect_changed(move |_| {
                if quiet.get() || !editing.get() {
                    return;
                }
                changes.set(changes.get() + 1);
                window.set_title(Some(&format!("{}*", name.borrow())));
                // Tags do not change the text, so this does not run again.
                view::paint_syntax(&preview, &name.borrow(), syntax_on.get());
            });
        }
        // Closing the window with a quick edit open gets the same unsaved-changes guard as Esc.
        let tx = sender.input_sender().clone();
        root.connect_close_request(move |_| {
            if editing.get() {
                tx.emit(Msg::CloseWindow);
                return gtk::glib::Propagation::Stop;
            }
            gtk::glib::Propagation::Proceed
        });
        let mut model = App {
            state,
            shell,
            cfg,
            status: String::new(),
            dirty: [true, true],
            job: None,
            ask: None,
            dialog: None,
            progress: None,
            meter: None,
            pending: None,
            queued: None,
            queue: None,
            lister: None,
            lister_gen: 0,
            lister_draw: 0,
            image: None,
            image_token: 0,
            image_gen: 0,
            image_live: Arc::new(AtomicU64::new(0)),
            charset: Charset::Utf8,
            hot: None,
            hot_stack: Vec::new(),
            hot_pane: PaneId::Left,
            preview: lister::PreviewSlot::default(),
            preview_sent: 0,
            preview_shown: [0, 0],
            quick_fit: true,
            edit_quiet,
            syntax_on,
            edit_name,
            edit_changes,
            quick_edit: None,
            preview_live: Arc::new(AtomicU64::new(0)),
            copy_gen: 0,
            loads: Vec::new(),
            search: None,
            search_gen: 0,
            search_target: None,
            settings: None,
            command_list: None,
            smoke,
        };
        model.apply_css(&theme::load_named(&theme::active_name(
            &model.cfg.theme,
            &config::active_theme_path(),
        )));
        model.watch_columns(&sender);
        model.sync();
        model.show_sort_indicator();
        for id in [PaneId::Left, PaneId::Right] {
            let path = model.state.tab(id).path.clone();
            let token = model.state.tab(id).token;
            model.spawn_load(&sender, id, 0, path, token);
        }
        let tx = sender.input_sender().clone();
        gtk::glib::timeout_add_local(Duration::from_millis(50), move || {
            tx.emit(Msg::Poll);
            gtk::glib::ControlFlow::Continue
        });
        install_usr1();
        let tx = sender.input_sender().clone();
        gtk::glib::timeout_add_local(Duration::from_millis(200), move || {
            if take_usr1() {
                tx.emit(Msg::ReloadTheme);
            }
            gtk::glib::ControlFlow::Continue
        });
        if smoke {
            let tx = sender.input_sender().clone();
            gtk::glib::idle_add_local(move || {
                tx.emit(Msg::Smoke);
                gtk::glib::ControlFlow::Break
            });
        }
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Msg, sender: ComponentSender<Self>) {
        let idle_poll = matches!(msg, Msg::Poll);
        self.on_msg(msg, &sender);
        self.kick_preview(&sender);
        if !(idle_poll && self.job.is_none() && !self.dirty.iter().any(|pane| *pane)) {
            self.sync();
        }
    }
}

impl App {
    fn on_msg(&mut self, msg: Msg, sender: &ComponentSender<Self>) {
        match msg {
            Msg::Key(key, mods) => self.on_key(key, mods, sender),
            Msg::FKey(n, shift) => self.on_fkey(n, shift, sender),
            Msg::Listed(pane, tab, token, result) => self.on_listed(pane, tab, token, result),
            Msg::Cursor(pane, pos) => {
                let id = PaneId::from_index(pane);
                // The hotlist popup can move the cursor in the left list while it is open.
                if self.hot.is_some() && id != self.hot_pane {
                    return;
                }
                self.shell.active.set(pane);
                self.state.active = id;
                let current = self.state.tab(self.state.active).cursor;
                if current != pos as usize {
                    self.state.set_cursor(pos as usize);
                }
            }
            Msg::Activate(pane) => {
                self.shell.active.set(pane);
                self.state.active = PaneId::from_index(pane);
                self.activate_cursor(sender);
            }
            Msg::Sort(pane, key) => {
                self.shell.active.set(pane);
                self.state.active = PaneId::from_index(pane);
                self.set_sort(key);
            }
            Msg::FilterChanged(pane, text) => {
                self.state.pane_mut(PaneId::from_index(pane)).filter = text;
                self.state.tab_mut(PaneId::from_index(pane)).cursor = 0;
                self.touch(PaneId::from_index(pane));
            }
            Msg::PathGo(pane, text) => self.go_typed_path(pane, &text, sender),
            Msg::CopyShownPath(pane) => self.copy_shown_path(pane),
            Msg::CloseSearch => self.close_search(),
            Msg::SearchEscape => self.escape_search(),
            Msg::SearchStart => self.start_search(sender),
            Msg::SearchCancel => self.cancel_search(),
            Msg::SearchGo(index) => self.go_to_search_result(index, sender),
            Msg::SearchEvent(generation, event) => self.on_search_event(generation, event),
            Msg::Submit(text) => self.on_submit(&text, sender),
            Msg::CancelAsk => {
                self.ask = None;
                self.pending = None;
                self.queued = None;
                self.close_dialog();
            }
            Msg::Conflict(choice) => {
                // A late answer from a closed dialog must not answer the next question.
                let open = self.job.as_mut().is_some_and(|job| std::mem::replace(&mut job.conflict_open, false));
                if open {
                    if let Some(job) = &self.job {
                        let _ = job.answer_tx.send(choice);
                    }
                    self.close_dialog();
                }
            }
            Msg::Poll => {
                self.poll_job(sender);
                self.poll_queue(sender);
            }
            Msg::Enqueue(text) => self.enqueue(&text, sender),
            Msg::QueuePause => self.toggle_queue_pause(),
            Msg::QueueRemove => self.remove_queued(),
            Msg::QueueCancel => self.cancel_queued(),
            Msg::QueueConflict(choice) => self.answer_queue_conflict(choice),
            Msg::QueueClosed => self.close_queue(),
            Msg::Preview { generation, body } => {
                lister::apply_preview(&mut self.preview, generation, body);
            }
            Msg::ImageReady { generation, body } => self.show_image(generation, body),
            Msg::ListerOpened { generation, opened } => self.show_lister(generation, opened, sender),
            Msg::ListerText { generation, text } => self.show_lister_text(generation, text),
            Msg::ImageClosed(token) => {
                if self.image.as_ref().is_some_and(|ui| ui.token == token) {
                    self.image_gen = self.image_gen.wrapping_add(1);
                    self.image_live.store(self.image_gen, Ordering::Relaxed);
                    self.image = None;
                }
            }
            Msg::ReloadTheme => {
                let name = theme::active_name(&self.cfg.theme, &config::active_theme_path());
                self.apply_css(&theme::load_named(&name));
                self.paint_theme();
                self.status = format!("Theme {name}");
            }
            Msg::PickPlace(pane, path) => {
                let id = PaneId::from_index(pane);
                let token = self.state.navigate(id, path.clone(), true);
                let tab = self.state.pane(id).active;
                self.close_dialog();
                self.spawn_load(sender, id, tab, path, token);
            }
            Msg::ColumnSized(pane) => self.on_column_sized(pane),
            Msg::ToggleAutoColumns => self.toggle_auto_columns(),
            Msg::ToggleHidden => self.toggle_hidden(),
            Msg::ToggleModeButtons => self.toggle_mode_buttons(),
            Msg::ToggleFileIcons => self.toggle_file_icons(),
            Msg::ToggleSyntax => self.toggle_syntax(),
            Msg::ToggleFlashFeedback => self.toggle_flash_feedback(),
            Msg::ToggleBackgroundStretch => self.toggle_background_stretch(),
            Msg::ToggleImageScale => self.toggle_shown_scale(sender),
            Msg::SetCharset(charset) => self.set_charset(charset, sender),
            Msg::ToggleQuickEdit => self.toggle_quick_edit(sender),
            Msg::SaveQuick => self.save_quick(),
            Msg::DiscardQuick => self.discard_quick(sender),
            Msg::CopyPreviewPath => self.copy_preview_path(),
            Msg::CopyPreviewImage => self.copy_preview_image(sender),
            Msg::Run(act) => self.run_act(act, sender),
            Msg::ShowCommands => self.toggle_commands(sender),
            Msg::CommandsClosed => self.command_list = None,
            Msg::PropsSaved => {
                self.close_dialog();
                self.refresh_active(sender);
            }
            Msg::RefreshActive => self.refresh_active(sender),
            Msg::PreviewImageCopied { generation, image } => self.finish_image_copy(generation, image),
            Msg::ShellExited(id) => self.on_shell_exited(id),
            Msg::SelectShell(id) => self.select_shell(id),
            Msg::CloseWindow => {
                if self.leave_quick_edit(sender) {
                    self.shell.window.close();
                }
            }
            Msg::ImageStep(delta) => self.step_shown_image(delta, sender),
            Msg::PickBackground => self.pick_background(sender),
            Msg::ClearBackground => self.clear_background(),
            Msg::BackgroundImage(path) => self.set_background_image(path),
            Msg::BackgroundColor(color) => self.set_background_color(color),
            Msg::BackgroundAlpha(alpha) => self.set_background_alpha(alpha),
            Msg::CloseSettings => self.close_settings(),
            Msg::ArchiveListed(result) => self.on_archive_listed(result, sender),
            Msg::UnpackPick(dest, names) => self.on_unpack_pick(dest, names, sender),
            Msg::PackPick(dest, opts) => {
                self.close_dialog();
                self.confirm_pack(PathBuf::from(dest), opts, sender);
            }
            Msg::SetSevenZip(path) => self.set_tool(true, path),
            Msg::SetRar(path) => self.set_tool(false, path),
            Msg::SetEditor(id) => self.set_editor(id),
            Msg::EditorPickCancelled => self.paint_editor(),
            Msg::PickEditor => self.pick_editor(sender),
            Msg::SetTheme(name) => self.set_theme(name),
            Msg::SetIconPack(id) => self.set_icon_pack(id),
            Msg::Hotlist => self.toggle_hotlist(sender),
            Msg::HotlistPick(index) => self.hot_go(index, sender),
            Msg::HotlistMenu(index) => self.hot_menu(index, sender),
            Msg::HotlistBack => self.hot_back(sender),
            Msg::HotlistAdd => self.hot_add(sender),
            Msg::HotlistRemove => self.hot_remove(sender),
            Msg::HotlistConfigure => self.hot_configure(sender),
            Msg::HotlistAdded(title, path, with_target) => self.hot_added(title, path, with_target, sender),
            Msg::HotlistStored(marks) => self.hot_stored(marks),
            Msg::FocusPane(pane) => {
                // The hotlist takes focus, and GTK then focuses the left list.
                if self.hot.is_some() {
                    return;
                }
                // A click in the quick editor must not make its pane active and hide the editor.
                if self.quick_edit.as_ref().is_some_and(|edit| edit.pane == pane) {
                    self.shell.active.set(self.state.active.index());
                } else {
                    self.shell.active.set(pane);
                    self.state.active = PaneId::from_index(pane);
                }
            }
            Msg::Mount(pane, path) => self.goto_mount(pane, path, sender),
            Msg::Smoke => self.run_smoke(sender),
        }
    }

    fn on_key(&mut self, key: Key, mods: ModifierType, sender: &ComponentSender<Self>) {
        if key == Key::F12 && !mods.contains(ModifierType::CONTROL_MASK) && !mods.contains(ModifierType::ALT_MASK) {
            self.toggle_settings(sender);
            return;
        }
        if key == Key::F1
            && !mods.contains(ModifierType::CONTROL_MASK)
            && !mods.contains(ModifierType::ALT_MASK)
            && !mods.contains(ModifierType::SHIFT_MASK)
        {
            self.toggle_commands(sender);
            return;
        }
        let ctrl = mods.contains(ModifierType::CONTROL_MASK);
        let shift = mods.contains(ModifierType::SHIFT_MASK);
        let alt = mods.contains(ModifierType::ALT_MASK);
        // The path field keeps the pane you opened it from. Tab only completes that path.
        if self.path_focused() && matches!(key, Key::Tab | Key::ISO_Left_Tab) {
            if !ctrl && !alt && !shift {
                self.complete_focused_path();
            }
            return;
        }
        if self.image.is_some() && !ctrl && !alt {
            match key {
                Key::Escape => self.close_image(),
                Key::s | Key::S => self.toggle_image_scale(sender),
                Key::Left | Key::KP_Left | Key::Up | Key::KP_Up => self.step_image(-1, sender),
                Key::Right | Key::KP_Right | Key::Down | Key::KP_Down => self.step_image(1, sender),
                _ => {}
            }
            return;
        }
        if self.shell.editing.get() && ctrl && !shift && !alt && matches!(key, Key::s | Key::S) {
            self.save_quick();
            return;
        }
        if self.lister.is_some() && !ctrl && !alt {
            match key {
                Key::Escape => self.close_lister(),
                Key::_1 => self.lister_set_mode(ViewMode::Text, sender),
                Key::_2 => self.lister_set_mode(ViewMode::Binary, sender),
                Key::_3 => self.lister_set_mode(ViewMode::Hex, sender),
                Key::a | Key::A => self.set_charset(Charset::Ansi, sender),
                // S is ASCII/DOS in Total Commander. D does the same.
                Key::s | Key::S | Key::d | Key::D => self.set_charset(Charset::Ascii, sender),
                // j scrolls. Shift+J is pretty JSON.
                Key::J => self.set_charset(Charset::Json, sender),
                Key::F2 => self.lister_scroll(0, sender),
                Key::Down | Key::j => self.lister_scroll(64, sender),
                Key::Up | Key::k => self.lister_scroll(-64, sender),
                Key::Page_Down => self.lister_scroll(4096, sender),
                Key::Page_Up => self.lister_scroll(-4096, sender),
                _ => {}
            }
            return;
        }
        // Ctrl+Alt+Fn is taken by the virtual terminals.
        if ctrl && alt {
            return;
        }
        if ctrl {
            if let Some(slot) = drive_slot(key) {
                self.pick_drive(slot, sender);
                return;
            }
            match key {
                Key::a | Key::A if !shift => {
                    self.state.toggle_visible_marks();
                    self.touch(self.state.active);
                }
                Key::c | Key::C if shift => self.copy_selected_paths(),
                Key::c | Key::C if self.preview_image_focused() => self.copy_preview_image(sender),
                Key::F3 => self.set_sort(rust_commander::listing::SortKey::Name),
                Key::F4 => self.set_sort(rust_commander::listing::SortKey::Ext),
                Key::F5 if shift => self.start_pack(true, sender),
                Key::F5 => self.set_sort(rust_commander::listing::SortKey::Time),
                Key::F6 => self.set_sort(rust_commander::listing::SortKey::Size),
                Key::F7 => self.set_sort(rust_commander::listing::SortKey::Attr),
                Key::F9 if shift => self.start_verify(sender),
                Key::F9 => self.new_shell(sender),
                Key::s | Key::S => {
                    if shift {
                        self.open_search(sender);
                    } else {
                        let ui = &self.shell.panes[self.state.active.index()];
                        ui.filter.set_visible(true);
                        ui.filter.grab_focus();
                    }
                }
                Key::h | Key::H => self.toggle_hidden(),
                Key::l | Key::L => self.focus_path(),
                Key::y | Key::Y => self.toggle_syntax(),
                Key::t | Key::T => self.open_new_tab(sender),
                Key::w | Key::W => {
                    if self.shell_has_focus() {
                        self.close_current_shell();
                    } else {
                        let id = self.state.active;
                        self.state.close_tab(id);
                        self.touch(id);
                    }
                }
                Key::d | Key::D if self.shell.editing.get() => self.discard_quick(sender),
                Key::d | Key::D => self.toggle_hotlist(sender),
                Key::e | Key::E if self.state.quick_view && !self.shell.editing.get() => {
                    self.toggle_quick_edit(sender);
                }
                Key::v | Key::V if self.state.quick_view && self.shell.editing.get() => {
                    self.leave_quick_edit(sender);
                }
                Key::q | Key::Q => {
                    if self.shell.editing.get() && !self.leave_quick_edit(sender) {
                        return;
                    }
                    self.state.panel_terminal = false;
                    self.state.quick_view = !self.state.quick_view;
                    if self.state.quick_view {
                        self.note_preview_target();
                        self.focus_list();
                    }
                }
                Key::Tab | Key::ISO_Left_Tab => {
                    let id = self.state.active;
                    self.state.next_tab(id, shift);
                    self.touch(id);
                    self.note_preview_target();
                }
                Key::Left | Key::KP_Left | Key::Right | Key::KP_Right => self.show_active_on_other(sender),
                _ => {}
            }
            return;
        }
        if alt {
            if let Some(slot) = drive_slot(key) {
                self.show_shell_index(slot);
                return;
            }
            match key {
                Key::c | Key::C if shift => self.copy_panel_dir(),
                Key::Return | Key::KP_Enter => self.show_properties(sender),
                Key::F1 => self.show_places(PaneId::Left, sender),
                Key::F2 => self.show_places(PaneId::Right, sender),
                Key::F3 => self.open_external(true),
                Key::F5 => self.start_pack(false, sender),
                Key::F9 if shift => self.begin_unpack(false, sender),
                Key::F9 => self.begin_unpack(true, sender),
                Key::Left => self.history(true, sender),
                Key::Right => self.history(false, sender),
                _ => {}
            }
            return;
        }
        match key {
            Key::F2 => self.refresh_active(sender),
            Key::F3 => self.open_viewer(sender),
            Key::F4 => self.on_fkey(4, shift, sender),
            Key::F5 => self.on_fkey(5, shift, sender),
            Key::F6 => self.on_fkey(6, shift, sender),
            Key::F7 => self.on_fkey(7, shift, sender),
            Key::F8 | Key::Delete => self.on_fkey(8, shift, sender),
            Key::F9 if shift => self.start_sfv(sender),
            Key::F9 => self.toggle_panel_terminal(sender),
            Key::ISO_Left_Tab | Key::Tab if self.shell.editing.get() => {}
            Key::ISO_Left_Tab | Key::Tab => {
                self.state.active = self.state.active.other();
                self.note_preview_target();
                self.focus_list();
            }
            Key::Insert => {
                let index = self.state.tab(self.state.active).cursor;
                self.state.toggle_mark();
                self.paint_mark(self.state.active, index);
                self.focus_list();
            }
            Key::space | Key::KP_Space => {
                let index = self.state.tab(self.state.active).cursor;
                self.state.toggle_mark_stay();
                self.paint_mark(self.state.active, index);
                self.focus_list();
            }
            Key::BackSpace => self.go_up(sender),
            Key::Return => self.activate_cursor(sender),
            Key::Left | Key::KP_Left if self.state.quick_view => self.step_quick_image(-1),
            Key::Right | Key::KP_Right if self.state.quick_view => self.step_quick_image(1),
            Key::Up | Key::KP_Up => self.move_list(-1),
            Key::Down | Key::KP_Down => self.move_list(1),
            Key::Page_Up | Key::KP_Page_Up => self.move_list(-20),
            Key::Page_Down | Key::KP_Page_Down => self.move_list(20),
            Key::Home | Key::KP_Home => {
                self.state.set_cursor(0);
                self.focus_list();
            }
            Key::End | Key::KP_End => {
                self.state.set_cursor(usize::MAX);
                self.focus_list();
            }
            Key::a | Key::A if self.state.quick_view => self.set_charset(Charset::Ansi, sender),
            Key::j | Key::J if self.state.quick_view => self.set_charset(Charset::Json, sender),
            Key::s | Key::S if self.state.quick_view && self.cursor_is_image() => self.toggle_quick_scale(),
            Key::s | Key::S | Key::d | Key::D if self.state.quick_view => self.set_charset(Charset::Ascii, sender),
            Key::Escape if self.path_focused() => self.cancel_path_edit(),
            Key::Escape if self.shell.editing.get() => {
                self.leave_quick_edit(sender);
            }
            Key::Escape => self.dismiss(),
            _ => {}
        }
    }

    fn on_fkey(&mut self, n: u8, shift: bool, sender: &ComponentSender<Self>) {
        let tx = sender.input_sender().clone();
        match n {
            2 => self.refresh_active(sender),
            3 => self.open_viewer(sender),
            12 => self.toggle_settings(sender),
            4 if shift => {
                self.ask = Some(Ask::CreateFile);
                self.ask_text("New file", "", tx);
            }
            4 => self.open_external(false),
            5 if shift => {
                let Some(src) = self.cursor_path() else {
                    self.status = "Nothing selected".into();
                    return;
                };
                self.ask = Some(Ask::CopyAs(src));
                let name = self.current_name().unwrap_or_default();
                self.ask_text("Copy as", &name, tx);
            }
            5 => {
                let sources = self.state.selected_paths(self.state.active);
                if sources.is_empty() {
                    self.status = "Nothing selected".into();
                    return;
                }
                self.ask = Some(Ask::Copy(sources));
                let dest = self.state.tab(self.state.active.other()).path.display().to_string();
                self.ask_text("Copy to", &dest, tx);
            }
            6 if shift => {
                let Some(src) = self.cursor_path() else {
                    self.status = "Nothing selected".into();
                    return;
                };
                self.ask = Some(Ask::Rename(src));
                let name = self.current_name().unwrap_or_default();
                self.ask_text("Rename", &name, tx);
            }
            6 => {
                let sources = self.state.selected_paths(self.state.active);
                if sources.is_empty() {
                    self.status = "Nothing selected".into();
                    return;
                }
                self.ask = Some(Ask::Move(sources));
                let dest = self.state.tab(self.state.active.other()).path.display().to_string();
                self.ask_text("Move to", &dest, tx);
            }
            7 => {
                self.ask = Some(Ask::Mkdir);
                self.ask_text("New folder", "", tx);
            }
            8 => {
                let paths = self.state.selected_paths(self.state.active);
                let what = match paths.as_slice() {
                    [] => {
                        self.status = "Nothing selected".into();
                        return;
                    }
                    [one] => one.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default(),
                    many => format!("{} items", many.len()),
                };
                let title = if shift { format!("Delete {what} permanently?") } else { format!("Move {what} to trash?") };
                self.ask = Some(if shift { Ask::Permanent(paths) } else { Ask::Trash(paths) });
                self.ask_confirm(&title, tx);
            }
            _ => {}
        }
    }

    fn on_submit(&mut self, text: &str, sender: &ComponentSender<Self>) {
        let text = text.to_string();
        let ask = self.ask.take();
        self.close_dialog();
        match ask {
            Some(Ask::Copy(sources)) => {
                let items = jobs::items_into(sources, &self.target_dir(&text));
                self.spawn_transfer(items, Kind::Copy, sender);
            }
            Some(Ask::Move(sources)) => {
                let items = jobs::items_into(sources, &self.target_dir(&text));
                self.spawn_transfer(items, Kind::Move, sender);
            }
            Some(Ask::CopyAs(src)) => {
                let dest = src.parent().unwrap_or(Path::new(".")).join(text.trim());
                self.spawn_transfer(vec![(src, dest)], Kind::Copy, sender);
            }
            Some(Ask::Rename(path)) => {
                match ops::rename_in_place(&path, text.trim()) {
                    Ok(_) => self.refresh_active(sender),
                    Err(err) => self.status = err.to_string(),
                }
            }
            Some(Ask::Mkdir) => {
                let parent = self.state.tab(self.state.active).path.clone();
                let name = text.trim();
                match ops::make_dir(&parent, name) {
                    Ok(path) => {
                        rust_commander::log::info(&format!("mkdir {}", path.display()));
                        self.refresh_active(sender);
                    }
                    Err(err) => {
                        rust_commander::log::error(&format!("mkdir {name}: {err}"));
                        self.status = err.to_string();
                    }
                }
            }
            Some(Ask::CreateFile) => {
                let parent = self.state.tab(self.state.active).path.clone();
                let name = text.trim();
                match ops::make_file(&parent, name) {
                    Ok(path) => {
                        rust_commander::log::info(&format!("create {}", path.display()));
                        self.refresh_active(sender);
                    }
                    Err(err) => {
                        rust_commander::log::error(&format!("create {name}: {err}"));
                        self.status = err.to_string();
                    }
                }
            }
            Some(Ask::Trash(paths)) => self.spawn_delete(paths, false, sender),
            Some(Ask::Permanent(paths)) => self.spawn_delete(paths, true, sender),
            Some(Ask::Password) => self.retry_password(text, sender),
            Some(Ask::Overwrite) => self.confirm_overwrite(sender),
            None => {}
        }
    }

    fn on_listed(&mut self, pane: usize, tab: usize, token: u64, result: Result<Vec<Entry>, String>) {
        // Timed out or replaced loads have left `loads`, so their late answer is dropped here.
        let Some(at) = self.loads.iter().position(|(live, _)| *live == token) else { return };
        self.loads.remove(at).1.store(true, Ordering::Relaxed);
        let id = PaneId::from_index(pane);
        match result {
            Ok(entries) => {
                if self.state.apply_listing(id, tab, token, entries) {
                    self.touch(id);
                    if let Some((target_id, target_tab, target_token, name)) = self.search_target.take() {
                        if (id, tab, token) == (target_id, target_tab, target_token) {
                            self.point_at(id, &name);
                            self.focus_list();
                        } else if id != target_id || tab != target_tab {
                            self.search_target = Some((target_id, target_tab, target_token, name));
                        }
                    }
                    if self.state.quick_view && id == self.state.active {
                        self.state.preview_gen = self.state.preview_gen.wrapping_add(1);
                    }
                }
            }
            Err(err) => {
                self.state.cancel_load(id, token);
                rust_commander::log::error(&format!("list pane={pane}: {err}"));
                self.status = err;
            }
        }
    }

    fn spawn_load(&mut self, sender: &ComponentSender<Self>, id: PaneId, tab: usize, path: PathBuf, token: u64) {
        rust_commander::log::info(&format!("read {}", path.display()));
        let tx = sender.input_sender().clone();
        let pane = id.index();
        // Stop only loads that no tab waits for. A load for another tab keeps going.
        let state = &self.state;
        self.loads.retain(|(live, stop)| {
            let wanted = [PaneId::Left, PaneId::Right]
                .iter()
                .any(|&id| state.pane(id).tabs.iter().any(|tab| tab.token == *live));
            if !wanted {
                stop.store(true, Ordering::Relaxed);
            }
            wanted
        });
        let stop = Arc::new(AtomicBool::new(false));
        self.loads.push((token, Arc::clone(&stop)));
        let seen = Arc::new(AtomicU64::new(0));
        watch_load(&tx, pane, tab, token, &path, &stop, &seen);
        std::thread::spawn(move || {
            let result = rust_commander::listing::read_dir_while(&path, || {
                seen.fetch_add(1, Ordering::Relaxed);
                stop.load(Ordering::Relaxed)
            });
            if result.as_ref().is_err_and(|err| err.kind() == std::io::ErrorKind::Interrupted) && stop.load(Ordering::Relaxed) {
                return;
            }
            tx.emit(Msg::Listed(pane, tab, token, result.map_err(|e| format!("{}: {e}", path.display()))));
        });
    }

    fn cursor_path(&self) -> Option<PathBuf> {
        let id = self.state.active;
        let entry = self.state.cursor_entry(id).filter(|entry| entry.name.as_ref() != "..")?;
        Some(rust_commander::listing::entry_path(&self.state.tab(id).path, entry))
    }

    // A relative target is taken from the active folder, not from the process working directory.
    fn target_dir(&self, text: &str) -> PathBuf {
        self.state.tab(self.state.active).path.join(text.trim())
    }

    fn refresh_active(&mut self, sender: &ComponentSender<Self>) {
        let id = self.state.active;
        let (path, token) = self.state.refresh(id);
        let tab = self.state.pane(id).active;
        self.spawn_load(sender, id, tab, path, token);
    }

    fn go_up(&mut self, sender: &ComponentSender<Self>) {
        let Some((id, path, token)) = self.state.up() else { return };
        let tab = self.state.pane(id).active;
        self.spawn_load(sender, id, tab, path, token);
    }

    fn activate_cursor(&mut self, sender: &ComponentSender<Self>) {
        if self.file_under_cursor().is_some_and(|path| rust_commander::sfv::is_sfv(&path)) {
            self.start_verify(sender);
            return;
        }
        if let Some((id, path, token)) = self.state.enter() {
            let tab = self.state.pane(id).active;
            self.spawn_load(sender, id, tab, path, token);
        } else {
            self.open_external(true);
        }
    }

    fn history(&mut self, back: bool, sender: &ComponentSender<Self>) {
        let id = self.state.active;
        let step = if back { self.state.go_back(id) } else { self.state.go_forward(id) };
        if let Some((path, token)) = step {
            let tab = self.state.pane(id).active;
            self.spawn_load(sender, id, tab, path, token);
        }
    }

    fn open_new_tab(&mut self, sender: &ComponentSender<Self>) {
        let id = self.state.active;
        let (tab, path, token) = self.state.new_tab(id);
        self.touch(id);
        self.spawn_load(sender, id, tab, path, token);
    }

    fn set_sort(&mut self, key: rust_commander::listing::SortKey) {
        let id = self.state.active;
        let pane = self.state.pane_mut(id);
        if pane.sort == key {
            pane.ascending = !pane.ascending;
        } else {
            pane.sort = key;
            pane.ascending = true;
        }
        self.cfg.sort = self.state.pane(id).sort.as_config().into();
        let _ = config::save(&self.cfg);
        self.touch(id);
        self.show_sort_indicator();
    }

    fn show_sort_indicator(&self) {
        let keys = [
            rust_commander::listing::SortKey::Name,
            rust_commander::listing::SortKey::Ext,
            rust_commander::listing::SortKey::Size,
            rust_commander::listing::SortKey::Time,
            rust_commander::listing::SortKey::Attr,
        ];
        let titles = ["Name", "Ext", "Size", "Date", "Attr"];
        for (i, pane) in self.shell.panes.iter().enumerate() {
            let sort = self.state.panes[i].sort;
            let arrow = if self.state.panes[i].ascending { "↑" } else { "↓" };
            let cols = pane.list.columns();
            let mut data = 0;
            for col_i in 0..cols.n_items() {
                let Some(col) = cols.item(col_i).and_then(|obj| obj.downcast::<gtk::ColumnViewColumn>().ok()) else { continue };
                if col.title().map(|title| title.is_empty()).unwrap_or(true) {
                    continue;
                }
                let Some(title) = titles.get(data) else { continue };
                let text = if keys[data] == sort {
                    format!("{title} {arrow}")
                } else {
                    (*title).to_string()
                };
                data += 1;
                if col.title().as_deref() != Some(text.as_str()) {
                    col.set_title(Some(&text));
                }
            }
        }
    }

    fn current_name(&self) -> Option<String> {
        self.state.cursor_entry(self.state.active).map(|e| e.name.to_string())
    }


    fn open_external(&mut self, viewer: bool) {
        let Some(path) = self.file_under_cursor().or_else(|| {
            self.state.cursor_entry(self.state.active).map(|e| {
                rust_commander::listing::entry_path(&self.state.tab(self.state.active).path, e)
            })
        }) else { return };
        if viewer && self.cfg.viewer.trim().is_empty() {
            if let Err(err) = launch_default(&path) {
                rust_commander::log::error(&err);
                self.status = err;
            }
            return;
        }
        let argv = if viewer {
            rust_commander::external::viewer_argv(&self.cfg.viewer, &path)
        } else {
            rust_commander::external::editor_argv(&self.cfg.editor, std::env::var("EDITOR").ok().as_deref(), &path)
        };
        if argv.is_empty() {
            self.status = if viewer { "No viewer found".into() } else { "No editor found".into() };
            return;
        }
        if let Err(err) = spawn_argv(&argv) {
            rust_commander::log::error(&err);
            self.status = err;
        }
    }

    fn show_places(&mut self, id: PaneId, sender: &ComponentSender<Self>) {
        let tx = sender.input_sender().clone();
        let pane = id.index();
        let places = rust_commander::places::places(&home_dir(), &self.cfg.bookmarks);
        self.close_dialog();
        let win = gtk::Window::new();
        win.set_title(Some(if pane == 0 { "Left places" } else { "Right places" }));
        win.set_modal(true);
        win.set_transient_for(Some(&self.shell.window));
        win.set_default_width(420);
        win.set_default_height(360);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 4);
        list.set_margin_all(8);
        for (label, path) in places {
            let button = gtk::Button::with_label(&format!("{label}  {}", path.display()));
            button.set_halign(gtk::Align::Start);
            let tx = tx.clone();
            shell::on_click(&button, move || {
                tx.emit(Msg::PickPlace(pane, path.clone()));
            });
            list.append(&button);
        }
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_child(Some(&list));
        win.set_child(Some(&scroll));
        win.present();
        self.dialog = Some(win);
    }

    fn toggle_hotlist(&mut self, sender: &ComponentSender<Self>) {
        if self.hot.is_some() {
            self.close_hot();
            return;
        }
        self.hot_pane = self.state.active;
        self.hot_stack = vec![None];
        config::reload_bookmarks(&mut self.cfg);
        self.present_hot(sender);
    }

    fn present_hot(&mut self, sender: &ComponentSender<Self>) {
        if let Some(win) = self.hot.take() {
            win.close();
        }
        let view = self.hot_stack.last().copied().flatten();
        let current = self.state.tab(self.hot_pane).path.clone();
        let win = super::hotlist::popup(
            &self.shell.window,
            &self.cfg.bookmarks,
            view,
            &current,
            sender.input_sender(),
        );
        win.present();
        self.hot = Some(win);
    }

    fn close_hot(&mut self) {
        self.hot_stack.clear();
        if let Some(win) = self.hot.take() {
            win.close();
        }
        self.state.active = self.hot_pane;
        self.focus_list();
    }

    fn hot_back(&mut self, sender: &ComponentSender<Self>) {
        self.hot_stack.pop();
        if self.hot_stack.is_empty() {
            self.close_hot();
        } else {
            self.present_hot(sender);
        }
    }

    fn hot_menu(&mut self, index: usize, sender: &ComponentSender<Self>) {
        self.hot_stack.push(Some(index));
        self.present_hot(sender);
    }

    fn hot_go(&mut self, index: usize, sender: &ComponentSender<Self>) {
        let Some(mark) = self.cfg.bookmarks.get(index).cloned() else { return };
        if !rust_commander::hotlist::is_dir(&mark) || mark.path.as_os_str().is_empty() {
            return;
        }
        let source = self.hot_pane;
        let target = mark.target.clone();
        let path = mark.path.clone();
        self.state.active = source;
        self.close_hot();
        let token = self.state.navigate(source, path.clone(), true);
        let tab = self.state.pane(source).active;
        self.spawn_load(sender, source, tab, path, token);
        if !target.as_os_str().is_empty() {
            let other = source.other();
            let token = self.state.navigate(other, target.clone(), true);
            let tab = self.state.pane(other).active;
            self.spawn_load(sender, other, tab, target, token);
        }
        self.focus_list();
    }

    fn hot_add(&mut self, sender: &ComponentSender<Self>) {
        let path = self.state.tab(self.state.active).path.clone();
        if rust_commander::hotlist::dir_index(&self.cfg.bookmarks, &path).is_some() {
            return;
        }
        let title = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let path_text = path.display().to_string();
        self.close_dialog();
        let win = super::hotlist::add_dialog(&self.shell.window, &title, &path_text, sender.input_sender());
        win.present();
        self.dialog = Some(win);
    }

    fn hot_added(&mut self, title: String, path: String, with_target: bool, sender: &ComponentSender<Self>) {
        self.close_dialog();
        let title = title.trim().to_string();
        let path = PathBuf::from(path.trim());
        if title.is_empty() || path.as_os_str().is_empty() {
            return;
        }
        let target = if with_target {
            self.state.tab(self.state.active.other()).path.clone()
        } else {
            std::path::PathBuf::new()
        };
        self.cfg.bookmarks.push(Bookmark { name: title, path, target, kind: String::new() });
        let _ = config::save(&self.cfg);
        if self.hot_stack.is_empty() {
            self.hot_stack = vec![None];
        }
        self.present_hot(sender);
    }

    fn hot_remove(&mut self, sender: &ComponentSender<Self>) {
        let path = self.state.tab(self.state.active).path.clone();
        let Some(index) = rust_commander::hotlist::dir_index(&self.cfg.bookmarks, &path) else { return };
        self.cfg.bookmarks.remove(index);
        let _ = config::save(&self.cfg);
        for slot in &mut self.hot_stack {
            if let Some(i) = slot {
                if *i == index {
                    *slot = None;
                } else if *i > index {
                    *i -= 1;
                }
            }
        }
        if self.hot.is_some() {
            self.present_hot(sender);
        }
    }

    fn hot_configure(&mut self, sender: &ComponentSender<Self>) {
        self.close_hot();
        self.close_dialog();
        let win = super::hotlist::configure(&self.shell.window, self.cfg.bookmarks.clone(), sender.input_sender());
        win.present();
        self.dialog = Some(win);
    }

    fn hot_stored(&mut self, marks: Vec<Bookmark>) {
        self.cfg.bookmarks = marks;
        let _ = config::save(&self.cfg);
        self.close_dialog();
    }


    fn sync(&mut self) {
        SYNCING.store(true, Ordering::Relaxed);
        if self.state.panel_terminal {
            self.place_panel_terminal(self.state.active.other().index());
        }
        self.restamp_shells();
        let mut pending: [Option<Vec<Entry>>; 2] = [None, None];
        // First sync only paints the accent. A later focus change blinks that border, then it stays.
        let focused_before = self.shell.panes.iter().any(|ui| ui.frame.has_css_class("pane-active"));
        for pane in 0..2 {
            let id = PaneId::from_index(pane);
            let active = self.state.active == id;
            let show_terminal = self.state.panel_terminal && !active;
            let show_preview = self.state.quick_view && !active && !show_terminal;
            let path = self.state.tab(id).path.display().to_string();
            let cursor = self.state.tab(id).cursor;
            let filter_on = !self.state.pane(id).filter.is_empty();
            let tab_titles: Vec<String> = self.state.pane(id).tabs.iter().map(|t| {
                t.path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into())
            }).collect();
            let tab_active = self.state.pane(id).active;
            let pane_dirty = self.dirty[pane];
            let rows: Vec<(Entry, bool)> = if pane_dirty {
                let vis = self.state.visible(id);
                let dir = self.state.tab(id).path.clone();
                let marked = self.state.tab(id).marked.clone();
                vis.iter().map(|&i| {
                    let entry = self.state.tab(id).entries[i].clone();
                    let path = rust_commander::listing::entry_path(&dir, &entry);
                    (entry, marked.contains(&path))
                }).collect()
            } else {
                Vec::new()
            };
            let ui = &self.shell.panes[pane];
            row::set_loading(&ui.list, self.loads.iter().any(|(live, _)| *live == self.state.tab(id).token));
            let class = if active { "pane-active" } else { "pane" };
            if !ui.frame.has_css_class(class) {
                ui.frame.set_css_classes(&[class]);
                if active && focused_before {
                    shell::blink_frame(&ui.frame, pane);
                }
            }
            if !self.focus_inside(&ui.path) && ui.path.text().as_str() != path {
                ui.path.set_text(&path);
            }
            let show_filter = filter_on || ui.filter.has_focus() || gtk::prelude::WidgetExt::is_visible(&ui.filter);
            if gtk::prelude::WidgetExt::is_visible(&ui.filter) != show_filter {
                ui.filter.set_visible(show_filter);
            }
            let page = if show_terminal {
                "terminal"
            } else if show_preview {
                "preview"
            } else {
                "list"
            };
            if ui.stack.visible_child_name().as_deref() != Some(page) {
                ui.stack.set_visible_child_name(page);
            }
            if show_preview && !self.shell.editing.get() {
                let (syntax, name) = self.preview_syntax();
                if let Some(rev) = view::show_slot(ui, &self.preview, self.preview_shown[pane], self.quick_fit, syntax, &name) {
                    self.preview_shown[pane] = rev;
                }
            }
            let labels = tab_labels(&tab_titles, tab_active);
            if tabs_current(&ui.tabs) != labels {
                rust_commander::log::info(&format!("tabs rebuild pane={pane}"));
                while let Some(child) = ui.tabs.first_child() {
                    ui.tabs.remove(&child);
                }
                for label in &labels {
                    ui.tabs.append(&gtk::Button::with_label(label));
                }
            }
            if pane_dirty {
                rust_commander::log::info(&format!("list reload pane={pane} rows={}", rows.len()));
                ui.store.remove_all();
                for (entry, marked) in &rows {
                    ui.store.append(&EntryObject::from_entry(
                        entry,
                        *marked,
                        self.cfg.file_icons,
                        crate::ui::iconpack::active(&self.cfg.icon_pack),
                    ));
                }
                self.dirty[pane] = false;
                pending[pane] = Some(rows.iter().map(|(entry, _)| entry.clone()).collect());
            }
            let n = ui.selection.n_items();
            if n > 0 {
                let want = (cursor as u32).min(n - 1);
                if ui.selection.selected() != want {
                    ui.selection.set_selected(want);
                    reveal_row(&ui.list, want);
                }
            }
        }
        for (pane, entries) in pending.into_iter().enumerate() {
            if let Some(entries) = entries {
                self.layout_columns(pane, &entries);
            }
        }
        if self.shell.status.text().as_str() != self.status {
            self.shell.status.set_text(&self.status);
        }
        self.sync_hints();
        self.paint_mode_bar();
        self.mark_drives();
        SYNCING.store(false, Ordering::Relaxed);
    }

    fn run_smoke(&mut self, sender: &ComponentSender<Self>) {
        if !self.smoke {
            return;
        }
        let mut errors = Vec::new();
        let want = ["F3 View", "F4 Edit", "F5 Copy", "F6 Move", "F7 Mkdir", "F8 Delete", "F12 Settings"];
        for label in want {
            if !self.shell.f_labels.iter().any(|have| have == label) {
                errors.push(format!("missing {label}"));
            }
        }
        for id in [PaneId::Left, PaneId::Right] {
            self.reload_pane(id);
        }
        self.sync();
        let tab_widget = self.shell.panes[0].tabs.first_child();
        self.sync();
        if self.shell.panes[0].tabs.first_child() != tab_widget {
            errors.push("tabs rebuilt".into());
        }
        let left = self.state.tab(PaneId::Left).path.clone();
        let right = self.state.tab(PaneId::Right).path.clone();
        if self.shell.panes[0].path.text().as_str() != left.display().to_string() {
            errors.push("left path".into());
        }
        if self.shell.panes[1].path.text().as_str() != right.display().to_string() {
            errors.push("right path".into());
        }
        if !self.shell.panes[0].frame.has_css_class("pane-active") {
            errors.push("left frame".into());
        }
        for pane in &self.shell.panes {
            let cols = pane.list.columns();
            let mut titles = Vec::new();
            let mut date_resizable = false;
            for i in 0..cols.n_items() {
                let Some(col) = cols.item(i).and_then(|obj| obj.downcast::<gtk::ColumnViewColumn>().ok()) else { continue };
                if let Some(title) = col.title() {
                    if title.contains("Date") {
                        date_resizable = col.is_resizable();
                    }
                    titles.push(title.to_string());
                }
            }
            for want in ["Name", "Ext", "Size", "Date", "Attr"] {
                if !titles.iter().any(|text| text.contains(want)) {
                    errors.push(format!("missing column {want}"));
                }
            }
            if !date_resizable {
                errors.push("date column fixed".into());
            }
            if super::shell::header_titles(&pane.list).len() != 5 {
                errors.push("header titles".into());
            }
        }
        let before = self.shell.panes[0].selection.selected();
        self.state.active = PaneId::Left;
        self.state.move_cursor(1);
        self.sync();
        if self.shell.panes[0].selection.selected() == before {
            errors.push("arrow cursor".into());
        }
        self.state.active = PaneId::Right;
        self.sync();
        if !self.shell.panes[1].frame.has_css_class("pane-active") || self.shell.panes[0].frame.has_css_class("pane-active") {
            errors.push("tab frame".into());
        }
        self.state.active = PaneId::Left;
        self.state.navigate(PaneId::Left, left.join("sub"), true);
        self.sync();
        if self.shell.panes[0].path.text().as_str() != left.display().to_string() {
            errors.push("navigate before listing".into());
        }
        self.reload_pane(PaneId::Left);
        self.sync();
        if self.shell.panes[0].path.text().as_str() != left.join("sub").display().to_string()
            || self.shell.panes[1].path.text().as_str() != right.display().to_string()
        {
            errors.push("navigate".into());
        }
        self.state.navigate(PaneId::Left, left.clone(), true);
        self.reload_pane(PaneId::Left);
        self.state.tab_mut(PaneId::Left).marked.insert(left.join("note.txt"));
        self.state.tab_mut(PaneId::Left).marked.insert(left.join("sub"));
        self.sync();
        if !super::shell::shows(&self.shell.hints, rust_commander::commands::Act::Pack)
            || !super::shell::shows(&self.shell.hints, rust_commander::commands::Act::PackHere)
        {
            errors.push("pack hint".into());
        }
        if Path::new(&self.pack_suggestion(false)).parent() != Some(right.as_path())
            || Path::new(&self.pack_suggestion(true)).parent() != Some(left.as_path())
        {
            errors.push("pack destination suggestion".into());
        }
        self.on_key(Key::F5, ModifierType::ALT_MASK, sender);
        if self.dialog.is_none() {
            errors.push("Alt+F5 pack dialog".into());
        }
        self.close_dialog();
        self.on_key(Key::F5, ModifierType::CONTROL_MASK | ModifierType::SHIFT_MASK, sender);
        if self.dialog.is_none() {
            errors.push("Ctrl+Shift+F5 pack dialog".into());
        }
        self.close_dialog();
        let packed = right.join("smoke.zip");
        self.confirm_pack(packed.clone(), rust_commander::pack::Options { level: 9, password: Some("smoke".into()) }, sender);
        if !self.wait_job(sender) || !rust_commander::archive::list(&packed, None).is_ok_and(|listed| listed.needs_password) {
            errors.push("pack failed".into());
        }
        let _ = std::fs::remove_file(&packed);
        self.on_fkey(5, false, sender);
        self.on_submit(&right.display().to_string(), sender);
        if !self.wait_job(sender) || !right.join("note.txt").is_file() || !right.join("sub/b.txt").is_file() {
            errors.push("copy failed".into());
        }
        self.reload_pane(PaneId::Left);
        self.reload_pane(PaneId::Right);
        self.state.tab_mut(PaneId::Left).marked.clear();
        self.sync();
        if super::shell::shows(&self.shell.hints, rust_commander::commands::Act::Pack)
            || super::shell::shows(&self.shell.hints, rust_commander::commands::Act::PackHere)
        {
            errors.push("pack hint stays".into());
        }
        if !self.point_at(PaneId::Left, "move-me.txt") {
            errors.push("missing move-me".into());
        }
        self.on_fkey(6, false, sender);
        self.on_submit(&right.display().to_string(), sender);
        if !self.wait_job(sender) || left.join("move-me.txt").exists() || !right.join("move-me.txt").is_file() {
            errors.push("move failed".into());
        }
        self.reload_pane(PaneId::Right);
        self.state.tab_mut(PaneId::Right).marked.clear();
        if !self.point_at(PaneId::Right, "move-me.txt") {
            errors.push("missing moved file".into());
        }
        self.on_fkey(6, false, sender);
        self.on_submit(&left.display().to_string(), sender);
        if !self.wait_job(sender) || !left.join("move-me.txt").is_file() {
            errors.push("move back failed".into());
        }
        self.reload_pane(PaneId::Left);
        self.state.tab_mut(PaneId::Left).marked.clear();
        if !self.point_at(PaneId::Left, "move-me.txt") {
            errors.push("missing queued file".into());
        }
        self.on_fkey(5, false, sender);
        self.enqueue(&right.join("queued").display().to_string(), sender);
        if !self.wait_job(sender) || !right.join("queued/move-me.txt").is_file() || !left.join("move-me.txt").is_file() {
            errors.push("queued copy failed".into());
        }

        self.state.active = PaneId::Left;
        self.on_fkey(7, false, sender);
        self.on_submit("created", sender);
        if !left.join("created").is_dir() {
            errors.push("mkdir failed".into());
        }
        self.reload_pane(PaneId::Left);
        self.state.tab_mut(PaneId::Left).marked.clear();
        if !self.point_at(PaneId::Left, "created") {
            errors.push("missing created".into());
        }
        self.on_fkey(6, true, sender);
        self.on_submit("renamed", sender);
        if !left.join("renamed").is_dir() {
            errors.push("rename failed".into());
        }

        let trash_me = left.join("trash-me.txt");
        let _ = std::fs::write(&trash_me, b"trash");
        self.reload_pane(PaneId::Left);
        self.state.tab_mut(PaneId::Left).marked.clear();
        if !self.point_at(PaneId::Left, "trash-me.txt") {
            errors.push("missing trash file".into());
        }
        self.on_fkey(8, false, sender);
        self.on_submit("", sender);
        if !self.wait_job(sender) || trash_me.exists() {
            errors.push("trash failed".into());
        }

        self.reload_pane(PaneId::Left);
        if !self.point_at(PaneId::Left, "note.txt") {
            errors.push("missing note".into());
        }
        if let Some(path) = self.file_under_cursor() {
            self.show_lister(self.lister_gen, view::read_lister(&path, self.charset), sender);
        }
        let lister_text = self.lister.as_ref().map(|ui| {
            let buf = ui.view.buffer();
            buf.text(&buf.start_iter(), &buf.end_iter(), false).to_string()
        });
        if !lister_text.unwrap_or_default().contains("hello rust-commander") {
            errors.push("lister text".into());
        }
        self.close_lister();

        self.state.quick_view = true;
        self.preview.generation = self.state.preview_gen;
        self.preview.set_body(PreviewBody::Text("hello rust-commander\n".into()));
        self.sync();
        if self.shell.panes[1].stack.visible_child_name().as_deref() != Some("preview") {
            errors.push("quick view hidden".into());
        }
        let preview = self.shell.panes[1].preview.buffer();
        if !preview.text(&preview.start_iter(), &preview.end_iter(), false).contains("hello rust-commander") {
            errors.push("quick view text".into());
        }
        let stale_gen = self.state.preview_gen;
        if !self.point_at(PaneId::Left, "move-me.txt") {
            errors.push("missing preview target".into());
        }
        let mut slot = lister::PreviewSlot { generation: self.state.preview_gen, ..lister::PreviewSlot::default() };
        if lister::apply_preview(&mut slot, stale_gen, PreviewBody::Text("stale".into())) {
            errors.push("stale preview applied".into());
        }
        self.preview.generation = self.state.preview_gen;
        self.preview.set_body(PreviewBody::Text("move\n".into()));
        self.sync();
        let preview = self.shell.panes[1].preview.buffer();
        if !preview.text(&preview.start_iter(), &preview.end_iter(), false).contains("move") {
            errors.push("quick view follow".into());
        }
        self.state.quick_view = false;
        self.sync();
        if self.shell.panes[1].stack.visible_child_name().as_deref() != Some("list") {
            errors.push("quick view stayed".into());
        }

        self.toggle_settings(sender);
        let shown = self.settings.as_ref().map(|ui| ui.columns.text().to_string());
        if shown.as_deref() != Some("on") {
            errors.push("settings".into());
        }
        let hidden = self.settings.as_ref().map(|ui| ui.hidden.text().to_string());
        if hidden.as_deref() != Some("off") {
            errors.push("hidden files".into());
        }
        let image = self.settings.as_ref().map(|ui| ui.image.text().to_string());
        if image.as_deref() != Some("none") {
            errors.push("background image".into());
        }
        self.toggle_auto_columns();
        if self.cfg.auto_columns {
            errors.push("auto columns stayed on".into());
        }
        self.toggle_auto_columns();
        if !self.cfg.auto_columns {
            errors.push("auto columns stayed off".into());
        }
        self.close_settings();

        self.on_key(Key::S, ModifierType::CONTROL_MASK | ModifierType::SHIFT_MASK, sender);
        if self.search.is_none() {
            errors.push("search shortcut".into());
        }
        self.close_search();

        self.state.active = PaneId::Left;
        self.on_key(Key::s, ModifierType::CONTROL_MASK, sender);
        self.sync();
        let filter = &self.shell.panes[0].filter;
        if !gtk::prelude::WidgetExt::is_visible(filter) {
            errors.push("quick filter hidden".into());
        }
        filter.set_text("note");
        self.on_msg(Msg::FilterChanged(0, "note".into()), sender);
        self.sync();
        if self.state.visible(PaneId::Left).iter().filter(|&&index| {
            self.state.tab(PaneId::Left).entries[index].name.as_ref() == "note.txt"
        }).count() != 1 || self.state.visible(PaneId::Left).len() != 2 {
            errors.push("quick filter matches".into());
        }

        // Last, since the guard queues a message that would run after this.
        self.shell.editing.set(true);
        self.shell.window.close();
        if !gtk::prelude::WidgetExt::is_visible(&self.shell.window) {
            errors.push("window closed during quick edit".into());
        }
        self.shell.editing.set(false);

        if errors.is_empty() {
            println!("SMOKE_OK");
            std::process::exit(0);
        }
        eprintln!("SMOKE_FAIL {}", errors.join("; "));
        std::process::exit(1);
    }

    fn wait_job(&mut self, sender: &ComponentSender<Self>) -> bool {
        for _ in 0..300 {
            self.poll_job(sender);
            self.poll_queue(sender);
            if self.job.is_none() && self.queue.is_none() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    fn reload_pane(&mut self, id: PaneId) {
        let (path, token) = self.state.refresh(id);
        let Ok(entries) = rust_commander::listing::read_dir(&path) else { return };
        let tab = self.state.pane(id).active;
        self.state.apply_listing(id, tab, token, entries);
        self.touch(id);
    }

    fn paint_mark(&self, id: PaneId, index: usize) {
        let Some(obj) = self.shell.panes[id.index()]
            .store
            .item(index as u32)
            .and_then(|item| item.downcast::<EntryObject>().ok())
        else {
            return;
        };
        let on = self.state.visible(id).get(index).is_some_and(|&slot| {
            let entry = &self.state.tab(id).entries[slot];
            let path = rust_commander::listing::entry_path(&self.state.tab(id).path, entry);
            self.state.tab(id).marked.contains(&path)
        });
        obj.set_marked(on);
    }

    fn pick_drive(&mut self, slot: usize, sender: &ComponentSender<Self>) {
        let Some(path) = self.shell.drives.get(slot).map(|(path, _)| path.clone()) else {
            return;
        };
        self.goto_mount(self.state.active.index(), path, sender);
    }

    fn go_typed_path(&mut self, pane: usize, text: &str, sender: &ComponentSender<Self>) {
        let id = PaneId::from_index(pane);
        let current = self.state.tab(id).path.clone();
        let path = match typed_dir(&current, text) {
            Ok(path) => path,
            Err(err) => {
                self.status = err;
                return;
            }
        };
        if path == current {
            self.focus_list();
            return;
        }
        self.state.active = id;
        self.shell.active.set(pane);
        let token = self.state.navigate(id, path.clone(), true);
        let tab = self.state.pane(id).active;
        self.spawn_load(sender, id, tab, path, token);
        self.focus_list();
    }

    fn dismiss(&mut self) {
        self.close_image();
        self.close_lister();
        self.state.quick_view = false;
        self.state.pane_mut(self.state.active).filter.clear();
        self.shell.panes[self.state.active.index()].filter.set_text("");
        self.shell.panes[self.state.active.index()].filter.set_visible(false);
        self.touch(self.state.active);
    }

    fn focus_path(&mut self) {
        let ui = &self.shell.panes[self.state.active.index()];
        ui.path.grab_focus();
    }

    fn path_focused(&self) -> bool {
        self.shell.panes.iter().any(|ui| self.focus_inside(&ui.path))
    }

    fn focus_inside(&self, ancestor: &impl IsA<gtk::Widget>) -> bool {
        let Some(focus) = gtk::prelude::GtkWindowExt::focus(&self.shell.window) else { return false };
        let ancestor = ancestor.clone().upcast::<gtk::Widget>();
        let mut current: Option<gtk::Widget> = Some(focus);
        while let Some(widget) = current {
            if widget == ancestor {
                return true;
            }
            current = widget.parent();
        }
        false
    }

    fn complete_focused_path(&mut self) -> bool {
        let Some(pane) = self.shell.panes.iter().position(|ui| self.focus_inside(&ui.path)) else {
            return false;
        };
        let text = self.shell.panes[pane].path.text().to_string();
        let current = self.state.tab(PaneId::from_index(pane)).path.clone();
        match complete_path(&current, &text) {
            Ok(next) => {
                let ui = &self.shell.panes[pane];
                ui.path.set_text(&next);
                ui.path.set_position(-1);
                self.status.clear();
            }
            Err(msg) => self.status = msg,
        }
        true
    }

    fn cancel_path_edit(&mut self) {
        for pane in 0..2 {
            let ui = &self.shell.panes[pane];
            if self.focus_inside(&ui.path) {
                let shown = self.state.tab(PaneId::from_index(pane)).path.display().to_string();
                ui.path.set_text(&shown);
            }
        }
        self.focus_list();
    }

    fn goto_mount(&mut self, pane: usize, path: PathBuf, sender: &ComponentSender<Self>) {
        let id = PaneId::from_index(pane);
        self.state.active = id;
        self.shell.active.set(pane);
        let token = self.state.navigate(id, path.clone(), true);
        let tab = self.state.pane(id).active;
        self.spawn_load(sender, id, tab, path.clone(), token);
        if let Some((_, button)) = self.shell.drives.iter().find(|(mount, _)| mount == &path) {
            shell::blink_drive(button);
        }
        self.focus_list();
    }

    fn mark_drives(&self) {
        let dir = &self.state.tab(self.state.active).path;
        let mounts: Vec<&Path> = self.shell.drives.iter().map(|(path, _)| path.as_path()).collect();
        let hit = rust_commander::places::current_mount(&mounts, dir);
        for (index, (_, button)) in self.shell.drives.iter().enumerate() {
            let on = hit == Some(index);
            if button.has_css_class("current") != on {
                if on {
                    button.add_css_class("current");
                } else {
                    button.remove_css_class("current");
                }
            }
        }
    }

    fn touch(&mut self, id: PaneId) {
        self.dirty[id.index()] = true;
    }

    fn touch_both(&mut self) {
        self.dirty = [true, true];
    }

    fn show_active_on_other(&mut self, sender: &ComponentSender<Self>) {
        let (dest, path, token) = self.state.mirror_to_other();
        let tab = self.state.pane(dest).active;
        self.spawn_load(sender, dest, tab, path, token);
    }

    fn move_list(&mut self, delta: isize) {
        self.state.move_cursor(delta);
        self.focus_list();
    }

    fn focus_list(&self) {
        let index = self.state.active.index();
        self.shell.active.set(index);
        self.shell.panes[index].list.grab_focus();
    }

    fn point_at(&mut self, id: PaneId, name: &str) -> bool {
        self.state.active = id;
        let pos = {
            let vis = self.state.visible(id);
            let entries = &self.state.tab(id).entries;
            vis.iter().position(|&i| entries[i].name.as_ref() == name)
        };
        let Some(pos) = pos else { return false };
        self.state.tab_mut(id).cursor = pos;
        self.state.preview_gen = self.state.preview_gen.wrapping_add(1);
        true
    }
}


fn launch_default(path: &Path) -> Result<(), String> {
    let file = gtk::gio::File::for_path(path);
    let mime = file
        .query_info("standard::content-type", gtk::gio::FileQueryInfoFlags::NONE, gtk::gio::Cancellable::NONE)
        .ok()
        .and_then(|info| info.content_type().map(|kind| kind.to_string()));
    let Some(mime) = mime else {
        return spawn_argv(&rust_commander::external::viewer_argv("", path));
    };
    let Some(app) = gtk::gio::AppInfo::default_for_type(&mime, false) else {
        return spawn_argv(&rust_commander::external::viewer_argv("", path));
    };
    let line = app.commandline().map(|cmd| cmd.to_string_lossy().into_owned()).unwrap_or_default();
    if line.contains("--started-from-file") {
        let argv = rust_commander::external::desktop_argv(&line, path);
        if !argv.is_empty() {
            return spawn_argv(&argv);
        }
    }
    let ctx: Option<&gtk::gio::AppLaunchContext> = None;
    app.launch(&[file], ctx).map_err(|err| err.to_string())
}

fn spawn_argv(argv: &[std::ffi::OsString]) -> Result<(), String> {
    if argv.is_empty() {
        return Err("No application".into());
    }
    let mut cmd = std::process::Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    let mut child = cmd.spawn().map_err(|err| format!("Cannot start {}: {err}", argv[0].to_string_lossy()))?;
    // Reap the child so it does not stay as a zombie until we exit.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

// Rows live in the inner list, which owns list.scroll-to-item.
// Gives up when no new name arrived for this long. A slow read that still moves keeps going.
const STALL: Duration = Duration::from_secs(20);

// A read blocked in the kernel cannot be killed. The thread is left behind and its answer is dropped.
fn watch_load(tx: &relm4::Sender<Msg>, pane: usize, tab: usize, token: u64, path: &Path, stop: &Arc<AtomicBool>, seen: &Arc<AtomicU64>) {
    let (tx, stop, seen) = (tx.clone(), Arc::clone(stop), Arc::clone(seen));
    let shown = path.display().to_string();
    let mut last = 0;
    gtk::glib::timeout_add_local(STALL, move || {
        if stop.load(Ordering::Relaxed) {
            return gtk::glib::ControlFlow::Break;
        }
        let now = seen.load(Ordering::Relaxed);
        if now == last {
            let err = format!("No answer from {shown} in {}s, gave up", STALL.as_secs());
            tx.emit(Msg::Listed(pane, tab, token, Err(err)));
            return gtk::glib::ControlFlow::Break;
        }
        last = now;
        gtk::glib::ControlFlow::Continue
    });
}

pub(super) fn shell_bin() -> String {
    let candidates = [
        std::env::var_os("SHELL").map(PathBuf::from),
        Some(PathBuf::from("/bin/bash")),
        Some(PathBuf::from("/bin/sh")),
    ];
    for path in candidates.into_iter().flatten() {
        if path.is_absolute() && std::fs::metadata(&path).is_ok() {
            return path.to_string_lossy().into_owned();
        }
    }
    "/bin/sh".into()
}

fn reveal_row(list: &gtk::ColumnView, pos: u32) {
    let arg = gtk::glib::Variant::from(pos);
    let mut child = list.first_child();
    while let Some(widget) = child {
        if widget.activate_action("list.scroll-to-item", Some(&arg)).is_ok() {
            return;
        }
        child = widget.next_sibling();
    }
}


// Tab fills the next directory name. One match is finished with a slash. Several matches share their prefix, or the names are shown.
fn complete_path(current: &Path, text: &str) -> Result<String, String> {
    let text = text.trim();
    if text == "~" {
        return Ok("~/".into());
    }
    let (base, prefix) = split_typed(text);
    let dir = listing_dir(current, &base)?;
    let mut names = dir_names(&dir, &prefix)?;
    names.sort();
    if names.is_empty() {
        return Err("No match".into());
    }
    if names.len() == 1 {
        return Ok(format!("{base}{}/", names[0]));
    }
    let shared = common_prefix(&names);
    if shared.chars().count() > prefix.chars().count() {
        return Ok(format!("{base}{shared}"));
    }
    let shown = if names.len() > 8 {
        format!("{}  …", names[..8].join("  "))
    } else {
        names.join("  ")
    };
    Err(shown)
}

fn split_typed(text: &str) -> (String, String) {
    if text.ends_with('/') {
        (text.to_string(), String::new())
    } else if let Some((dir, name)) = text.rsplit_once('/') {
        (format!("{dir}/"), name.to_string())
    } else {
        (String::new(), text.to_string())
    }
}

fn listing_dir(current: &Path, base: &str) -> Result<PathBuf, String> {
    if base.is_empty() {
        return Ok(current.to_path_buf());
    }
    let trimmed = base.trim_end_matches('/');
    if trimmed.is_empty() {
        return Ok(PathBuf::from("/"));
    }
    let raw = if trimmed == "~" {
        home_dir()
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        home_dir().join(rest)
    } else {
        let raw = PathBuf::from(trimmed);
        if raw.is_absolute() { raw } else { current.join(raw) }
    };
    if raw.is_dir() { Ok(raw) } else { Err("No match".into()) }
}

fn dir_names(dir: &Path, prefix: &str) -> Result<Vec<String>, String> {
    let read = std::fs::read_dir(dir).map_err(|_| "No match".to_string())?;
    let mut names = Vec::new();
    for item in read.flatten() {
        let name = item.file_name().to_string_lossy().into_owned();
        if !name.starts_with(prefix) || (name.starts_with('.') && !prefix.starts_with('.')) {
            continue;
        }
        let linked = item.file_type().is_ok_and(|kind| kind.is_symlink());
        let is_dir = item.file_type().is_ok_and(|kind| kind.is_dir())
            || (linked && std::fs::metadata(item.path()).is_ok_and(|meta| meta.is_dir()));
        if is_dir {
            names.push(name);
        }
    }
    Ok(names)
}

fn common_prefix(names: &[String]) -> String {
    let mut prefix: Vec<char> = names[0].chars().collect();
    for name in &names[1..] {
        let chars: Vec<char> = name.chars().collect();
        let len = prefix.iter().zip(chars.iter()).take_while(|(a, b)| a == b).count();
        prefix.truncate(len);
    }
    prefix.into_iter().collect()
}

// ~ and ~/ expand to $HOME. A relative path is joined onto the pane's current directory.
fn typed_dir(current: &Path, text: &str) -> Result<PathBuf, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(current.to_path_buf());
    }
    let raw = if text == "~" {
        home_dir()
    } else if let Some(rest) = text.strip_prefix("~/") {
        home_dir().join(rest)
    } else {
        PathBuf::from(text)
    };
    let path = if raw.is_absolute() { raw } else { current.join(&raw) };
    let path = std::fs::canonicalize(&path).map_err(|_| format!("Not a directory: {text}"))?;
    if path.is_dir() {
        Ok(path)
    } else {
        Err(format!("Not a directory: {text}"))
    }
}

fn tab_labels(titles: &[String], active: usize) -> Vec<String> {
    titles
        .iter()
        .enumerate()
        .map(|(i, title)| if i == active { format!("[{title}]") } else { title.clone() })
        .collect()
}

fn tabs_current(tabs: &gtk::Box) -> Vec<String> {
    let mut out = Vec::new();
    let mut child = tabs.first_child();
    while let Some(widget) = child {
        if let Some(button) = widget.downcast_ref::<gtk::Button>() {
            out.push(button.label().map(|s| s.to_string()).unwrap_or_default());
        }
        child = widget.next_sibling();
    }
    out
}


fn install_usr1() {
    unsafe {
        libc::signal(libc::SIGUSR1, on_usr1 as *const () as libc::sighandler_t);
    }
}

fn on_usr1(_: i32) {
    USR1.store(true, Ordering::Relaxed);
}

fn take_usr1() -> bool {
    USR1.swap(false, Ordering::Relaxed)
}

static USR1: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn drive_slot(key: Key) -> Option<usize> {
    Some(match key {
        Key::_1 | Key::KP_1 => 0,
        Key::_2 | Key::KP_2 => 1,
        Key::_3 | Key::KP_3 => 2,
        Key::_4 | Key::KP_4 => 3,
        Key::_5 | Key::KP_5 => 4,
        Key::_6 | Key::KP_6 => 5,
        Key::_7 | Key::KP_7 => 6,
        Key::_8 | Key::KP_8 => 7,
        Key::_9 | Key::KP_9 => 8,
        _ => return None,
    })
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).filter(|p| p.exists()).unwrap_or_else(|| PathBuf::from("/"))
}

fn smoke_dirs() -> (PathBuf, PathBuf) {
    let base = home_dir().join("rust-commander-smoke");
    let left = base.join("left");
    let right = base.join("right");
    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::create_dir_all(left.join("sub"));
    let _ = std::fs::create_dir_all(&right);
    let _ = std::fs::write(left.join("note.txt"), "hello rust-commander\n");
    let _ = std::fs::write(left.join("sub").join("b.txt"), "nested\n");
    let _ = std::fs::write(left.join("move-me.txt"), "move\n");
    (left, right)
}

#[cfg(test)]
mod path_complete {
    use super::complete_path;
    use std::fs;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rust-commander-complete-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Documents")).unwrap();
        fs::create_dir_all(dir.join("Downloads")).unwrap();
        fs::create_dir_all(dir.join("notes")).unwrap();
        fs::write(dir.join("file.txt"), "x").unwrap();
        dir
    }

    #[test]
    fn unique_directory_gains_a_slash() {
        let dir = scratch("one");
        assert_eq!(complete_path(&dir, "Doc").unwrap(), "Documents/");
        assert_eq!(
            complete_path(&dir, &format!("{}/Doc", dir.display())).unwrap(),
            format!("{}/Documents/", dir.display())
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn several_matches_share_a_prefix_or_list() {
        let dir = scratch("many");
        assert_eq!(complete_path(&dir, "D").unwrap(), "Do");
        let listed = complete_path(&dir, "Do").unwrap_err();
        assert!(listed.contains("Documents"));
        assert!(listed.contains("Downloads"));
        assert!(!listed.contains("file.txt"));
        assert!(!listed.contains("notes"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tilde_opens_home() {
        assert_eq!(complete_path(std::path::Path::new("/"), "~").unwrap(), "~/");
    }
}


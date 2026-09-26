use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use relm4::gtk;
use relm4::gtk::gdk::{Key, ModifierType};
use relm4::gtk::prelude::*;
use relm4::prelude::*;
use relm4::RelmWidgetExt;

use comarchy::config::{self, Bookmark, Config};
use comarchy::entry::Entry;
use comarchy::imageview;
use comarchy::lister::{self, Charset, PreviewBody, ViewMode};
use comarchy::ops::{self, Choice, Kind, Progress, Report};
use comarchy::state::{AppState, PaneId};
use comarchy::theme::{self, Palette};

use super::msg::{Msg, SYNCING};

static SIZING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
use super::row::EntryObject;
use super::shell::{self, Shell};

enum Ask {
    Copy,
    Move,
    CopyAs,
    Rename,
    Mkdir,
    Trash,
    Permanent,
    Pack,
    Password,
    Overwrite,
}

#[derive(Clone, Copy)]
enum Work {
    Transfer,
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
    Pack(PathBuf),
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
    image: gtk::Label,
    strength: gtk::Scale,
    editor: gtk::DropDown,
    editor_ids: Rc<RefCell<Vec<String>>>,
    editor_guard: Rc<Cell<bool>>,
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
    progress: Option<(gtk::Window, gtk::Label, gtk::ProgressBar)>,
    meter: Option<f64>,
    pending: Option<PendingUnpack>,
    queued: Option<Queued>,
    lister: Option<ListerUi>,
    image: Option<ImageUi>,
    image_token: u64,
    image_gen: u64,
    image_live: Arc<AtomicU64>,
    charset: Charset,
    hot: Option<gtk::Window>,
    hot_stack: Vec<Option<usize>>,
    preview: lister::PreviewSlot,
    preview_sent: u64,
    preview_shown: [u64; 2],
    quick_fit: bool,
    preview_live: Arc<AtomicU64>,
    list_live: [Arc<AtomicU64>; 2],
    settings: Option<SettingsUi>,
    chooser: Option<gtk::FileChooserNative>,
    smoke: bool,
}

pub fn run(smoke: bool) {
    comarchy::log::info(&format!("start log={}", comarchy::log::path().display()));
    let args: Vec<String> = std::env::args().filter(|arg| arg != "--smoke").collect();
    let app = RelmApp::new("se.comarchy.Comarchy").with_args(args);
    app.allow_multiple_instances(true);
    app.run::<App>(smoke);
}

#[relm4::component(pub)]
impl SimpleComponent for App {
    type Init = bool;
    type Input = Msg;
    type Output = ();

    view! {
        gtk::Window {
            set_title: Some("Commander"),
            set_default_width: 1200,
            set_default_height: 760,
        }
    }

    fn init(smoke: bool, root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
        let widgets = view_output!();
        let shell = shell::build(&root, sender.input_sender());
        let cfg = config::load();
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
            lister: None,
            image: None,
            image_token: 0,
            image_gen: 0,
            image_live: Arc::new(AtomicU64::new(0)),
            charset: Charset::Utf8,
            hot: None,
            hot_stack: Vec::new(),
            preview: lister::PreviewSlot::default(),
            preview_sent: 0,
            preview_shown: [0, 0],
            quick_fit: true,
            preview_live: Arc::new(AtomicU64::new(0)),
            list_live: [Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0))],
            settings: None,
            chooser: None,
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
        if !(idle_poll && self.job.is_none() && !self.dirty.iter().any(|pane| *pane)) {
            self.sync();
        }
        self.kick_preview(&sender);
    }
}

impl App {
    fn on_msg(&mut self, msg: Msg, sender: &ComponentSender<Self>) {
        match msg {
            Msg::Key(key, mods) => self.on_key(key, mods, sender),
            Msg::FKey(n, shift) => self.on_fkey(n, shift, sender),
            Msg::Listed(pane, tab, token, result) => self.on_listed(pane, tab, token, result),
            Msg::Cursor(pane, pos) => {
                self.shell.active.set(pane);
                self.state.active = PaneId::from_index(pane);
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
            Msg::Submit(text) => self.on_submit(&text, sender),
            Msg::CancelAsk => {
                self.ask = None;
                self.pending = None;
                self.queued = None;
                self.close_dialog();
            }
            Msg::Conflict(choice) => {
                if let Some(job) = &self.job {
                    let _ = job.answer_tx.send(choice);
                }
                if let Some(job) = &mut self.job {
                    job.conflict_open = false;
                }
                self.close_dialog();
            }
            Msg::Poll => self.poll_job(sender),
            Msg::Preview { generation, body } => {
                lister::apply_preview(&mut self.preview, generation, body);
            }
            Msg::ImageReady { generation, body } => self.show_image(generation, body),
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
            Msg::ToggleImageScale => self.toggle_shown_scale(sender),
            Msg::ImageStep(delta) => self.step_shown_image(delta, sender),
            Msg::PickBackground => self.pick_background(sender),
            Msg::BackgroundImage(path) => self.set_background_image(path),
            Msg::BackgroundColor(color) => self.set_background_color(color),
            Msg::BackgroundAlpha(alpha) => self.set_background_alpha(alpha),
            Msg::CloseSettings => self.close_settings(),
            Msg::ArchiveListed(result) => self.on_archive_listed(result, sender),
            Msg::UnpackPick(dest, names) => self.on_unpack_pick(dest, names, sender),
            Msg::UnpackList => self.begin_unpack(true, sender),
            Msg::UnpackHere => self.begin_unpack(false, sender),
            Msg::VerifySfv => self.start_verify(sender),
            Msg::SetSevenZip(path) => self.set_tool(true, path),
            Msg::SetRar(path) => self.set_tool(false, path),
            Msg::SetEditor(id) => self.set_editor(id),
            Msg::EditorPickCancelled => self.paint_editor(),
            Msg::PickEditor => self.pick_editor(sender),
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
                self.shell.active.set(pane);
                self.state.active = PaneId::from_index(pane);
            }
            Msg::Mount(pane, path) => {
                let id = PaneId::from_index(pane);
                self.state.active = id;
                self.shell.active.set(pane);
                let token = self.state.navigate(id, path.clone(), true);
                let tab = self.state.pane(id).active;
                self.spawn_load(sender, id, tab, path, token);
                self.focus_list();
            }
            Msg::JobDone(result) => self.finish_job(result, sender),
            Msg::Smoke => self.run_smoke(sender),
        }
    }

    fn on_key(&mut self, key: Key, mods: ModifierType, sender: &ComponentSender<Self>) {
        if key == Key::F12 && !mods.contains(ModifierType::CONTROL_MASK) && !mods.contains(ModifierType::ALT_MASK) {
            self.toggle_settings(sender);
            return;
        }
        let ctrl = mods.contains(ModifierType::CONTROL_MASK);
        let shift = mods.contains(ModifierType::SHIFT_MASK);
        let alt = mods.contains(ModifierType::ALT_MASK);
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
        if self.lister.is_some() && !ctrl && !alt {
            match key {
                Key::Escape => self.close_lister(),
                Key::_1 => self.lister_set_mode(ViewMode::Text),
                Key::_2 => self.lister_set_mode(ViewMode::Binary),
                Key::_3 => self.lister_set_mode(ViewMode::Hex),
                Key::a | Key::A => self.set_charset(Charset::Ansi, sender),
                // S is ASCII/DOS in Total Commander. D does the same.
                Key::s | Key::S | Key::d | Key::D => self.set_charset(Charset::Ascii, sender),
                Key::F2 => self.lister_scroll(0),
                Key::Down | Key::j => self.lister_scroll(64),
                Key::Up | Key::k => self.lister_scroll(-64),
                Key::Page_Down => self.lister_scroll(4096),
                Key::Page_Up => self.lister_scroll(-4096),
                _ => {}
            }
            return;
        }
        if ctrl && alt {
            if key == Key::F10 {
                self.begin_unpack(false, sender);
            }
            return;
        }
        if ctrl {
            match key {
                Key::F3 => self.set_sort(comarchy::listing::SortKey::Name),
                Key::F4 => self.set_sort(comarchy::listing::SortKey::Ext),
                Key::F5 => self.set_sort(comarchy::listing::SortKey::Time),
                Key::F6 => self.set_sort(comarchy::listing::SortKey::Size),
                Key::F7 => self.set_sort(comarchy::listing::SortKey::Attr),
                Key::s | Key::S => {
                    let ui = &self.shell.panes[self.state.active.index()];
                    ui.filter.set_visible(true);
                    ui.filter.grab_focus();
                }
                Key::h | Key::H => self.toggle_hidden(),
                Key::t | Key::T => self.open_new_tab(sender),
                Key::w | Key::W => {
                    let id = self.state.active;
                    self.state.close_tab(id);
                    self.touch(id);
                }
                Key::d | Key::D => self.toggle_hotlist(sender),
                Key::q | Key::Q => {
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
            match key {
                Key::F1 => self.show_places(PaneId::Left, sender),
                Key::F2 => self.show_places(PaneId::Right, sender),
                Key::F3 => self.open_external(true),
                Key::F5 => self.start_pack(sender),
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
            Key::F4 => self.open_external(false),
            Key::F5 => self.on_fkey(5, shift, sender),
            Key::F6 => self.on_fkey(6, shift, sender),
            Key::F7 => self.on_fkey(7, shift, sender),
            Key::F8 | Key::Delete => self.on_fkey(8, shift, sender),
            Key::F9 if shift => self.start_sfv(sender),
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
            Key::s | Key::S if self.state.quick_view && self.cursor_is_image() => self.toggle_quick_scale(),
            Key::s | Key::S | Key::d | Key::D if self.state.quick_view => self.set_charset(Charset::Ascii, sender),
            Key::Escape => {
                self.state.quick_view = false;
                self.state.pane_mut(self.state.active).filter.clear();
                self.shell.panes[self.state.active.index()].filter.set_text("");
                self.shell.panes[self.state.active.index()].filter.set_visible(false);
                self.touch(self.state.active);
            }
            _ => {}
        }
    }

    fn on_fkey(&mut self, n: u8, shift: bool, sender: &ComponentSender<Self>) {
        let tx = sender.input_sender().clone();
        match n {
            2 => self.refresh_active(sender),
            3 => self.open_viewer(sender),
            12 => self.toggle_settings(sender),
            4 => self.open_external(false),
            5 if shift => {
                self.ask = Some(Ask::CopyAs);
                let name = self.current_name().unwrap_or_default();
                self.ask_text("Copy as", &name, tx);
            }
            5 => {
                self.ask = Some(Ask::Copy);
                let dest = self.state.tab(self.state.active.other()).path.display().to_string();
                self.ask_text("Copy to", &dest, tx);
            }
            6 if shift => {
                self.ask = Some(Ask::Rename);
                let name = self.current_name().unwrap_or_default();
                self.ask_text("Rename", &name, tx);
            }
            6 => {
                self.ask = Some(Ask::Move);
                let dest = self.state.tab(self.state.active.other()).path.display().to_string();
                self.ask_text("Move to", &dest, tx);
            }
            7 => {
                self.ask = Some(Ask::Mkdir);
                self.ask_text("New folder", "", tx);
            }
            8 => {
                self.ask = Some(if shift { Ask::Permanent } else { Ask::Trash });
                let title = if shift { "Delete permanently?" } else { "Move to trash?" };
                self.ask_confirm(title, tx);
            }
            _ => {}
        }
    }

    fn on_submit(&mut self, text: &str, sender: &ComponentSender<Self>) {
        let text = text.to_string();
        let ask = self.ask.take();
        self.close_dialog();
        match ask {
            Some(Ask::Copy) => {
                let items = items_into(&self.state, Path::new(&text));
                self.spawn_transfer(items, Kind::Copy, sender);
            }
            Some(Ask::Move) => {
                let items = items_into(&self.state, Path::new(&text));
                self.spawn_transfer(items, Kind::Move, sender);
            }
            Some(Ask::CopyAs) => {
                let items = self.state.transfer_items(Some(text.trim()));
                self.spawn_transfer(items, Kind::Copy, sender);
            }
            Some(Ask::Rename) => {
                if let Some(path) = self.state.selected_paths(self.state.active).into_iter().next() {
                    match ops::rename_in_place(&path, text.trim()) {
                        Ok(_) => self.refresh_active(sender),
                        Err(err) => self.status = err.to_string(),
                    }
                }
            }
            Some(Ask::Mkdir) => {
                let parent = self.state.tab(self.state.active).path.clone();
                let name = text.trim();
                match ops::make_dir(&parent, name) {
                    Ok(path) => {
                        comarchy::log::info(&format!("mkdir {}", path.display()));
                        self.refresh_active(sender);
                    }
                    Err(err) => {
                        comarchy::log::error(&format!("mkdir {name}: {err}"));
                        self.status = err.to_string();
                    }
                }
            }
            Some(Ask::Trash) => self.spawn_delete(false, sender),
            Some(Ask::Permanent) => self.spawn_delete(true, sender),
            Some(Ask::Pack) => self.confirm_pack(PathBuf::from(text.trim()), sender),
            Some(Ask::Password) => self.retry_password(text, sender),
            Some(Ask::Overwrite) => self.confirm_overwrite(sender),
            None => {}
        }
    }

    fn on_listed(&mut self, pane: usize, tab: usize, token: u64, result: Result<Vec<Entry>, String>) {
        let ui = &self.shell.panes[pane];
        ui.spinner.stop();
        ui.spinner.set_visible(false);
        match result {
            Ok(entries) => {
                let id = PaneId::from_index(pane);
                if self.state.apply_listing(id, tab, token, entries) {
                    self.touch(id);
                    if self.state.quick_view && id == self.state.active {
                        self.state.preview_gen = self.state.preview_gen.wrapping_add(1);
                    }
                }
            }
            Err(err) => {
                comarchy::log::error(&format!("list pane={pane}: {err}"));
                self.status = err;
            }
        }
    }

    fn spawn_load(&self, sender: &ComponentSender<Self>, id: PaneId, tab: usize, path: PathBuf, token: u64) {
        let ui = &self.shell.panes[id.index()];
        ui.spinner.set_visible(true);
        ui.spinner.start();
        comarchy::log::info(&format!("read {}", path.display()));
        let tx = sender.input_sender().clone();
        let pane = id.index();
        let live = Arc::clone(&self.list_live[pane]);
        let epoch = live.fetch_add(1, Ordering::Relaxed) + 1;
        std::thread::spawn(move || {
            let result = comarchy::listing::read_dir_while(&path, || live.load(Ordering::Relaxed) != epoch)
                .map_err(|e| e.to_string());
            tx.emit(Msg::Listed(pane, tab, token, result));
        });
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
        if self.file_under_cursor().is_some_and(|path| comarchy::sfv::is_sfv(&path)) {
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

    fn set_sort(&mut self, key: comarchy::listing::SortKey) {
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
            comarchy::listing::SortKey::Name,
            comarchy::listing::SortKey::Ext,
            comarchy::listing::SortKey::Size,
            comarchy::listing::SortKey::Time,
            comarchy::listing::SortKey::Attr,
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

    fn spawn_transfer(&mut self, items: Vec<(PathBuf, PathBuf)>, kind: Kind, sender: &ComponentSender<Self>) {
        if items.is_empty() {
            self.status = "Nothing selected".into();
            return;
        }
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (progress_tx, progress_rx) = mpsc::channel();
        let (conflict_tx, conflict_rx) = mpsc::channel();
        let (answer_tx, answer_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let cancel_flag = Arc::clone(&cancel);
        std::thread::spawn(move || {
            let result = ops::transfer(
                &items,
                kind,
                &cancel_flag,
                &mut |p| {
                    let _ = progress_tx.send(p.clone());
                },
                &mut |src, dst| {
                    if conflict_tx.send((src.to_path_buf(), dst.to_path_buf())).is_err() {
                        return Choice::Cancel;
                    }
                    answer_rx.recv().unwrap_or(Choice::Cancel)
                },
            );
            let _ = done_tx.send(result.map_err(|e| e.to_string()));
        });
        self.meter = None;
        self.job = Some(Job {
            work: Work::Transfer,
            cancel,
            progress_rx,
            conflict_rx,
            answer_tx,
            done_rx,
            started: Instant::now(),
            conflict_open: false,
        });
        let _ = sender;
    }

    fn spawn_delete(&mut self, permanent: bool, sender: &ComponentSender<Self>) {
        let paths = self.state.selected_paths(self.state.active);
        if paths.is_empty() {
            self.status = "Nothing selected".into();
            return;
        }
        let tx = sender.input_sender().clone();
        std::thread::spawn(move || {
            let result = ops::delete_paths(&paths, permanent, &mut |path| {
                if permanent {
                    return Ok(());
                }
                gio_trash(path)
            });
            tx.emit(Msg::JobDone(result.map(|_| Report { cancelled: false, files: 0 }).map_err(|e| e.to_string())));
        });
    }

    fn poll_job(&mut self, sender: &ComponentSender<Self>) {
        let polled = {
            let Some(job) = self.job.as_mut() else { return };
            let mut line = None;
            let mut show = false;
            let mut fraction: Option<Option<f64>> = None;
            while let Ok(progress) = job.progress_rx.try_recv() {
                line = Some(if progress.note.is_empty() {
                    format!("{}/{}  {}", progress.done, progress.total, progress.current.display())
                } else {
                    progress.note.clone()
                });
                fraction = Some(if progress.total == 0 {
                    None
                } else {
                    Some(progress.done as f64 / progress.total as f64)
                });
                show = job.started.elapsed() > Duration::from_millis(200);
            }
            let conflict = if job.conflict_open { None } else { job.conflict_rx.try_recv().ok() };
            if conflict.is_some() {
                job.conflict_open = true;
            }
            let done = job.done_rx.try_recv().ok();
            (line, show, fraction, conflict, done)
        };
        let (line, show, fraction, conflict, done) = polled;
        if let Some(line) = line {
            self.status = line;
        }
        if let Some(next) = fraction {
            self.meter = next;
        }
        if show {
            self.ensure_progress();
        }
        if self.progress.is_some() {
            if let Some((_, label, bar)) = &self.progress {
                label.set_text(&self.status);
                match self.meter {
                    Some(value) => bar.set_fraction(value.clamp(0.0, 1.0)),
                    None => bar.pulse(),
                }
            }
        }
        if let Some((src, dst)) = conflict {
            self.ask_conflict(&src, &dst, sender.input_sender().clone());
        }
        if let Some(result) = done {
            self.finish_job(result, sender);
        }
    }

    fn finish_job(&mut self, result: Result<Report, String>, sender: &ComponentSender<Self>) {
        let work = self.job.as_ref().map(|job| job.work).unwrap_or(Work::Transfer);
        self.job = None;
        if let Some((win, _, _)) = self.progress.take() {
            win.close();
        }
        if let Err(err) = &result {
            if err == "Cancelled" {
                self.pending = None;
                self.status = "Cancelled".into();
                self.refresh_both(sender);
                return;
            }
            if (err == "Password required" || err == "Wrong password") && self.offer_password(sender) {
                return;
            }
            self.pending = None;
            if !matches!(work, Work::Transfer) {
                let title = match work {
                    Work::Unpack => "Unpack failed",
                    Work::Pack => "Pack failed",
                    Work::Sfv | Work::Verify => "SFV failed",
                    Work::Transfer => "Failed",
                };
                self.show_error(title, err);
            }
            comarchy::log::error(&format!("job {err}"));
            self.status = first_line(err);
            self.refresh_both(sender);
            return;
        }
        self.pending = None;
        self.status = match result {
            Ok(report) if report.cancelled => "Cancelled".into(),
            Ok(_) if matches!(work, Work::Verify) => "SFV OK".into(),
            Ok(_) => "Done".into(),
            Err(err) => err,
        };
        self.refresh_both(sender);
    }

    fn refresh_both(&mut self, sender: &ComponentSender<Self>) {
        for id in [PaneId::Left, PaneId::Right] {
            let (path, token) = self.state.refresh(id);
            let tab = self.state.pane(id).active;
            self.spawn_load(sender, id, tab, path, token);
        }
    }

    fn open_job(&mut self, work: Work) -> Option<JobOut> {
        if self.job.is_some() {
            self.status = "Busy".into();
            return None;
        }
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (progress_tx, progress_rx) = mpsc::channel();
        let (conflict_tx, conflict_rx) = mpsc::channel();
        let (answer_tx, answer_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        self.meter = None;
        self.job = Some(Job {
            work,
            cancel: Arc::clone(&cancel),
            progress_rx,
            conflict_rx,
            answer_tx,
            done_rx,
            started: Instant::now(),
            conflict_open: false,
        });
        Some(JobOut { cancel, progress_tx, conflict_tx, answer_rx, done_tx })
    }

    fn begin_unpack(&mut self, pick: bool, sender: &ComponentSender<Self>) {
        let Some(path) = self.file_under_cursor() else {
            self.status = "Not an archive".into();
            return;
        };
        if !comarchy::archive::is_archive(&path) {
            self.status = "Not an archive".into();
            return;
        }
        if self.job.is_some() {
            self.status = "Busy".into();
            return;
        }
        let dest = if pick {
            self.state.tab(self.state.active.other()).path.clone()
        } else {
            self.state.tab(self.state.active).path.clone()
        };
        self.pending = Some(PendingUnpack {
            archive: path,
            dest,
            names: None,
            password: None,
            asked: 0,
            pick,
            phase: Phase::List,
        });
        self.spawn_archive_list(sender);
    }

    fn spawn_archive_list(&mut self, sender: &ComponentSender<Self>) {
        let Some(pending) = &self.pending else { return };
        let path = pending.archive.clone();
        let password = pending.password.clone();
        self.status = "Reading archive".into();
        let tx = sender.input_sender().clone();
        std::thread::spawn(move || {
            let result = comarchy::archive::list(&path, password.as_deref()).map_err(|err| err.to_string());
            tx.emit(Msg::ArchiveListed(result));
        });
    }

    fn on_archive_listed(&mut self, result: Result<comarchy::archive::Listed, String>, sender: &ComponentSender<Self>) {
        let Some(pending) = &self.pending else { return };
        let pick = pending.pick;
        let password = pending.password.clone();
        let dest = pending.dest.display().to_string();
        match result {
            Err(err) if err == "Password required" || err == "Wrong password" => {
                if !self.offer_password(sender) {
                    self.fail_unpack(&err);
                }
            }
            Err(err) => self.fail_unpack(&err),
            Ok(listed) if listed.needs_password && password.is_none() => {
                if !self.offer_password(sender) {
                    self.fail_unpack("Password required");
                }
            }
            Ok(listed) if listed.members.is_empty() => {
                self.pending = None;
                self.status = "Archive is empty".into();
            }
            Ok(listed) if pick => self.ask_unpack(&listed.members, &dest, sender.input_sender().clone()),
            Ok(_) => {
                if let Some(pending) = &mut self.pending {
                    pending.phase = Phase::Extract;
                    pending.names = None;
                }
                self.spawn_extract(sender);
            }
        }
    }

    fn on_unpack_pick(&mut self, dest: String, names: Vec<String>, sender: &ComponentSender<Self>) {
        self.close_dialog();
        if names.is_empty() {
            self.pending = None;
            self.status = "Nothing selected".into();
            return;
        }
        if let Some(pending) = &mut self.pending {
            pending.dest = PathBuf::from(dest);
            pending.names = Some(names);
            pending.phase = Phase::Extract;
        }
        self.spawn_extract(sender);
    }

    fn spawn_extract(&mut self, sender: &ComponentSender<Self>) {
        let Some(pending) = &self.pending else { return };
        let archive = pending.archive.clone();
        let dest = pending.dest.clone();
        let names = pending.names.clone();
        let password = pending.password.clone();
        let Some(pipes) = self.open_job(Work::Unpack) else { return };
        self.status = "Unpacking".into();
        comarchy::log::info(&format!("unpack {}", archive.display()));
        self.ensure_progress();
        let _ = sender;
        std::thread::spawn(move || {
            let set = names.map(|names| names.into_iter().collect::<std::collections::HashSet<_>>());
            let result = comarchy::archive::extract(
                &archive,
                &dest,
                set.as_ref(),
                password.as_deref(),
                &pipes.cancel,
                &mut |progress| {
                    let _ = pipes.progress_tx.send(progress.clone());
                },
                &mut |src, dst| {
                    if pipes.conflict_tx.send((src.to_path_buf(), dst.to_path_buf())).is_err() {
                        return Choice::Cancel;
                    }
                    pipes.answer_rx.recv().unwrap_or(Choice::Cancel)
                },
            );
            let _ = pipes.done_tx.send(result.map_err(|err| err.to_string()));
        });
    }

    fn offer_password(&mut self, sender: &ComponentSender<Self>) -> bool {
        let Some(pending) = &mut self.pending else { return false };
        if pending.asked >= 2 {
            return false;
        }
        pending.asked += 1;
        self.ask = Some(Ask::Password);
        self.ask_text("Password", "", sender.input_sender().clone());
        true
    }

    fn retry_password(&mut self, text: String, sender: &ComponentSender<Self>) {
        let phase = {
            let Some(pending) = &mut self.pending else { return };
            pending.password = Some(text);
            match pending.phase {
                Phase::List => Phase::List,
                Phase::Extract => Phase::Extract,
            }
        };
        match phase {
            Phase::List => self.spawn_archive_list(sender),
            Phase::Extract => self.spawn_extract(sender),
        }
    }

    fn fail_unpack(&mut self, err: &str) {
        self.pending = None;
        self.show_error("Unpack failed", err);
        self.status = first_line(err);
    }

    fn ask_unpack(&mut self, members: &[comarchy::archive::Member], dest: &str, tx: relm4::Sender<Msg>) {
        self.close_dialog();
        let win = gtk::Window::new();
        win.set_title(Some("Unpack"));
        win.set_modal(true);
        win.set_transient_for(Some(&self.shell.window));
        win.set_default_width(560);
        win.set_default_height(480);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        column.set_margin_all(12);
        let entry = gtk::Entry::new();
        entry.set_text(dest);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_vexpand(true);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let checks = Rc::new(RefCell::new(Vec::new()));
        for member in members {
            let shown = if member.dir { format!("{}/", member.name) } else { member.name.clone() };
            let check = gtk::CheckButton::with_label(&shown);
            check.set_active(true);
            check.set_margin_start(4);
            check.set_margin_end(4);
            check.set_margin_top(2);
            check.set_margin_bottom(2);
            list.append(&check);
            checks.borrow_mut().push((check, member.name.clone()));
        }
        scroll.set_child(Some(&list));
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let ok = gtk::Button::with_label("OK");
        let entry_ok = entry.clone();
        let tx_ok = tx.clone();
        let checks_ok = Rc::clone(&checks);
        let fire = Rc::new(move || {
            let names = checks_ok
                .borrow()
                .iter()
                .filter(|(check, _)| check.is_active())
                .map(|(_, name)| name.clone())
                .collect();
            tx_ok.emit(Msg::UnpackPick(entry_ok.text().to_string(), names));
        });
        let fire_ok = Rc::clone(&fire);
        ok.connect_clicked(move |_| fire_ok());
        let fire_entry = Rc::clone(&fire);
        entry.connect_activate(move |_| fire_entry());
        let cancel = gtk::Button::with_label("Cancel");
        cancel.connect_clicked(move |_| tx.emit(Msg::CancelAsk));
        row.append(&ok);
        row.append(&cancel);
        column.append(&entry);
        column.append(&scroll);
        column.append(&row);
        win.set_child(Some(&column));
        win.set_default_widget(Some(&ok));
        win.present();
        ok.grab_focus();
        self.dialog = Some(win);
    }

    fn start_pack(&mut self, sender: &ComponentSender<Self>) {
        if self.state.selected_paths(self.state.active).is_empty() {
            self.status = "Nothing selected".into();
            return;
        }
        self.ask = Some(Ask::Pack);
        self.ask_text("Pack to", &self.pack_suggestion(), sender.input_sender().clone());
    }

    fn pack_suggestion(&self) -> String {
        let sources = self.state.selected_paths(self.state.active);
        let stem = if sources.len() == 1 {
            sources[0].file_stem().and_then(|name| name.to_str()).unwrap_or("archive")
        } else {
            self.state.tab(self.state.active).path.file_name().and_then(|name| name.to_str()).unwrap_or("archive")
        };
        self.state.tab(self.state.active.other()).path.join(format!("{stem}.zip")).display().to_string()
    }

    fn confirm_pack(&mut self, dest: PathBuf, sender: &ComponentSender<Self>) {
        if comarchy::pack::format_of(&dest).is_none() {
            self.status = "Use a .zip, .7z or .rar name".into();
            return;
        }
        if dest.exists() {
            self.queued = Some(Queued::Pack(dest));
            self.ask = Some(Ask::Overwrite);
            self.ask_confirm("Overwrite archive?", sender.input_sender().clone());
            return;
        }
        self.spawn_pack(dest, sender);
    }

    fn spawn_pack(&mut self, dest: PathBuf, sender: &ComponentSender<Self>) {
        let Some(format) = comarchy::pack::format_of(&dest) else {
            self.status = "Use a .zip, .7z or .rar name".into();
            return;
        };
        let bin = match format {
            comarchy::pack::Format::Zip => None,
            other => {
                let configured = match other {
                    comarchy::pack::Format::SevenZ => self.cfg.seven_zip.as_str(),
                    comarchy::pack::Format::Rar => self.cfg.rar.as_str(),
                    comarchy::pack::Format::Zip => "",
                };
                match comarchy::pack::find_program(configured, comarchy::pack::names(other)) {
                    Some(path) => Some(path),
                    None => {
                        self.show_error("Pack failed", comarchy::pack::hint(other));
                        self.status = first_line(comarchy::pack::hint(other));
                        return;
                    }
                }
            }
        };
        let sources = self.state.selected_paths(self.state.active);
        let cwd = self.state.tab(self.state.active).path.clone();
        let Some(pipes) = self.open_job(Work::Pack) else { return };
        let _ = sender;
        std::thread::spawn(move || {
            let result = comarchy::pack::pack(format, bin.as_deref(), &sources, &cwd, &dest, &pipes.cancel, &mut |progress| {
                let _ = pipes.progress_tx.send(progress.clone());
            });
            let _ = pipes.done_tx.send(result.map_err(|err| err.to_string()));
        });
    }

    fn start_sfv(&mut self, sender: &ComponentSender<Self>) {
        let sources = self.state.selected_paths(self.state.active);
        if sources.is_empty() {
            self.status = "Nothing selected".into();
            return;
        }
        let dest = self.sfv_dest(&sources);
        if dest.exists() {
            self.queued = Some(Queued::Sfv(dest));
            self.ask = Some(Ask::Overwrite);
            self.ask_confirm("Overwrite SFV?", sender.input_sender().clone());
            return;
        }
        self.spawn_sfv_create(dest, sender);
    }

    fn sfv_dest(&self, sources: &[PathBuf]) -> PathBuf {
        let stem = if sources.len() == 1 {
            sources[0].file_stem().and_then(|name| name.to_str()).unwrap_or("checksum")
        } else {
            self.state.tab(self.state.active).path.file_name().and_then(|name| name.to_str()).unwrap_or("checksum")
        };
        self.state.tab(self.state.active).path.join(format!("{stem}.sfv"))
    }

    fn spawn_sfv_create(&mut self, dest: PathBuf, sender: &ComponentSender<Self>) {
        let root = self.state.tab(self.state.active).path.clone();
        let sources = self.state.selected_paths(self.state.active);
        let Some(pipes) = self.open_job(Work::Sfv) else { return };
        let _ = sender;
        std::thread::spawn(move || {
            let result = comarchy::sfv::create(&root, &sources, &dest, &pipes.cancel, &mut |progress| {
                let _ = pipes.progress_tx.send(progress.clone());
            });
            let _ = pipes.done_tx.send(result.map_err(|err| err.to_string()));
        });
    }

    fn start_verify(&mut self, sender: &ComponentSender<Self>) {
        let Some(path) = self.file_under_cursor().filter(|path| comarchy::sfv::is_sfv(path)) else {
            self.status = "Not an SFV file".into();
            return;
        };
        let Some(pipes) = self.open_job(Work::Verify) else { return };
        let _ = sender;
        std::thread::spawn(move || {
            let result = comarchy::sfv::verify(&path, &pipes.cancel, &mut |progress| {
                let _ = pipes.progress_tx.send(progress.clone());
            });
            let _ = pipes.done_tx.send(result.map_err(|err| err.to_string()));
        });
    }

    fn confirm_overwrite(&mut self, sender: &ComponentSender<Self>) {
        match self.queued.take() {
            Some(Queued::Pack(dest)) => self.spawn_pack(dest, sender),
            Some(Queued::Sfv(dest)) => self.spawn_sfv_create(dest, sender),
            None => {}
        }
    }

    fn set_tool(&mut self, seven: bool, path: String) {
        if seven {
            self.cfg.seven_zip = path;
        } else {
            self.cfg.rar = path;
        }
        let _ = config::save(&self.cfg);
    }

    fn show_error(&mut self, title: &str, text: &str) {
        self.close_dialog();
        let win = gtk::Window::new();
        win.set_title(Some(title));
        win.set_modal(true);
        win.set_transient_for(Some(&self.shell.window));
        win.set_default_width(520);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        column.set_margin_all(12);
        let label = gtk::Label::new(Some(text));
        label.set_wrap(true);
        label.set_xalign(0.0);
        label.set_selectable(true);
        let ok = gtk::Button::with_label("OK");
        let win_ok = win.clone();
        ok.connect_clicked(move |_| win_ok.close());
        column.append(&label);
        column.append(&ok);
        win.set_child(Some(&column));
        win.present();
        self.dialog = Some(win);
    }

    fn sync_hints(&self) {
        let name = self.state.cursor_entry(self.state.active).map(|entry| entry.name.to_ascii_lowercase());
        let archive = name.as_deref().is_some_and(|name| name.ends_with(".zip") || name.ends_with(".rar"));
        let sfv = name.as_deref().is_some_and(|name| name.ends_with(".sfv"));
        self.shell.unpack_list.set_visible(archive);
        self.shell.unpack_here.set_visible(archive);
        self.shell.verify_sfv.set_visible(sfv);
    }

    fn open_viewer(&mut self, sender: &ComponentSender<Self>) {
        let image = self
            .state
            .cursor_entry(self.state.active)
            .is_some_and(|entry| !entry.is_dir() && imageview::is_image(&entry.name));
        if image {
            self.open_image(sender);
        } else {
            self.open_lister(sender);
        }
    }

    fn open_image(&mut self, sender: &ComponentSender<Self>) {
        let Some(path) = self.file_under_cursor() else { return };
        let id = self.state.active;
        let files = self.image_files(id);
        let Some(index) = files.iter().position(|item| item == &path) else { return };
        self.close_lister();
        self.close_image();
        self.image_token = self.image_token.wrapping_add(1);
        let token = self.image_token;
        let window = gtk::Window::new();
        window.set_title(Some(&path.display().to_string()));
        window.set_transient_for(Some(&self.shell.window));
        window.set_default_width(800);
        window.set_default_height(600);
        let picture = gtk::Picture::new();
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_child(Some(&picture));
        scroll.set_hexpand(true);
        scroll.set_vexpand(true);
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        bar.add_css_class("fbar");
        bar.set_margin_all(4);
        let scale = gtk::Button::with_label("S Fit");
        scale.add_css_class("fkey");
        scale.set_hexpand(true);
        let tx_scale = sender.input_sender().clone();
        scale.connect_clicked(move |_| tx_scale.emit(Msg::ToggleImageScale));
        bar.append(&scale);
        bar.set_visible(self.cfg.mode_buttons);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.append(&scroll);
        column.append(&bar);
        window.set_child(Some(&column));
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let tx = sender.input_sender().clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            tx.emit(Msg::Key(key, mods));
            gtk::glib::Propagation::Stop
        });
        window.add_controller(keys);
        let tx = sender.input_sender().clone();
        window.connect_close_request(move |_| {
            tx.emit(Msg::ImageClosed(token));
            gtk::glib::Propagation::Proceed
        });
        window.present();
        let ui = ImageUi { window, picture, scroll, bar, scale, files, index, generation: 0, token, fit: true };
        apply_image_scale(&ui);
        self.image = Some(ui);
        self.load_image(sender);
    }

    fn load_image(&mut self, sender: &ComponentSender<Self>) {
        self.image_gen = self.image_gen.wrapping_add(1);
        let generation = self.image_gen;
        let Some(ui) = self.image.as_mut() else { return };
        ui.generation = generation;
        let Some(path) = ui.files.get(ui.index).cloned() else { return };
        let fit = ui.fit;
        ui.window.set_title(Some(&path.display().to_string()));
        let live = Arc::clone(&self.image_live);
        live.store(generation, Ordering::Relaxed);
        let tx = sender.input_sender().clone();
        std::thread::spawn(move || {
            if live.load(Ordering::Relaxed) != generation {
                return;
            }
            let edge = if fit { imageview::MAX_EDGE } else { u32::MAX };
            let body = preview_body(&path, edge);
            if live.load(Ordering::Relaxed) != generation {
                return;
            }
            tx.emit(Msg::ImageReady { generation, body });
        });
    }

    fn show_image(&mut self, generation: u64, body: PreviewBody) {
        let picture = self
            .image
            .as_ref()
            .filter(|ui| ui.generation == generation)
            .map(|ui| ui.picture.clone());
        let Some(picture) = picture else { return };
        let fit = self.image.as_ref().is_some_and(|ui| ui.fit);
        match body {
            PreviewBody::Image { width, height, pixels } => {
                set_texture(&picture, width, height, &pixels);
                picture.set_size_request(if fit { -1 } else { width as i32 }, if fit { -1 } else { height as i32 });
            }
            PreviewBody::Text(err) => self.status = err,
        }
    }

    fn step_image(&mut self, delta: isize, sender: &ComponentSender<Self>) {
        let Some(ui) = self.image.as_mut() else { return };
        let Some(next) = imageview::step_index(ui.files.len(), ui.index, delta) else { return };
        ui.index = next;
        self.load_image(sender);
    }

    fn close_image(&mut self) {
        self.image_gen = self.image_gen.wrapping_add(1);
        self.image_live.store(self.image_gen, Ordering::Relaxed);
        if let Some(ui) = self.image.take() {
            ui.window.close();
        }
    }

    fn step_quick_image(&mut self, delta: isize) {
        if !self.state.quick_view {
            return;
        }
        let id = self.state.active;
        let Some(entry) = self.state.cursor_entry(id) else { return };
        if entry.is_dir() || !imageview::is_image(&entry.name) {
            return;
        }
        let Some(path) = self.file_under_cursor() else { return };
        let files = self.image_files(id);
        let Some(index) = files.iter().position(|item| item == &path) else { return };
        let Some(next) = imageview::step_index(files.len(), index, delta) else { return };
        let Some(name) = files[next].file_name().and_then(|name| name.to_str()) else { return };
        let name = name.to_string();
        if self.point_at(id, &name) {
            self.focus_list();
        }
    }

    fn image_files(&self, id: PaneId) -> Vec<PathBuf> {
        let dir = &self.state.tab(id).path;
        let entries = &self.state.tab(id).entries;
        let order = self.state.visible(id);
        imageview::image_paths(dir, entries, &order)
    }

    fn open_lister(&mut self, sender: &ComponentSender<Self>) {
        self.close_image();
        let Some(path) = self.file_under_cursor() else { return };
        let Ok(mut reader) = lister::Reader::open(&path) else {
            self.status = format!("Cannot open {}", path.display());
            return;
        };
        let Ok(bytes) = reader.read_window() else {
            self.status = format!("Cannot open {}", path.display());
            return;
        };
        let text = lister::render(ViewMode::Text, self.charset, &bytes, reader.offset);
        self.close_lister();
        let window = gtk::Window::new();
        window.set_title(Some(&path.display().to_string()));
        window.set_transient_for(Some(&self.shell.window));
        window.set_default_width(800);
        window.set_default_height(600);
        let view = gtk::TextView::new();
        view.set_editable(false);
        view.set_monospace(true);
        view.buffer().set_text(&text);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_child(Some(&view));
        window.set_child(Some(&scroll));
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let tx = sender.input_sender().clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            tx.emit(Msg::Key(key, mods));
            gtk::glib::Propagation::Stop
        });
        window.add_controller(keys);
        window.present();
        self.lister = Some(ListerUi { window, view, reader, mode: ViewMode::Text });
    }

    fn lister_set_mode(&mut self, mode: ViewMode) {
        if let Some(lister) = self.lister.as_mut() {
            lister.mode = mode;
        }
        self.lister_redraw();
    }

    fn lister_scroll(&mut self, delta: i64) {
        if let Some(lister) = self.lister.as_mut() {
            if delta != 0 {
                lister.reader.scroll(delta);
            } else {
                lister.reader.offset = lister.reader.offset.min(lister.reader.len);
            }
        }
        self.lister_redraw();
    }

    fn lister_redraw(&mut self) {
        let charset = self.charset;
        let Some(lister) = self.lister.as_mut() else { return };
        let mode = lister.mode;
        if let Ok(bytes) = lister.reader.read_window() {
            let text = lister::render(mode, charset, &bytes, lister.reader.offset);
            lister.view.buffer().set_text(&text);
        }
    }

    fn set_charset(&mut self, charset: Charset, sender: &ComponentSender<Self>) {
        self.charset = charset;
        self.status = match charset {
            Charset::Utf8 => "UTF-8".into(),
            Charset::Ansi => "ANSI".into(),
            Charset::Ascii => "ASCII".into(),
        };
        self.lister_redraw();
        if self.state.quick_view {
            self.note_preview_target();
            self.kick_preview(sender);
        }
    }

    fn close_lister(&mut self) {
        if let Some(lister) = self.lister.take() {
            lister.window.close();
        }
    }

    fn note_preview_target(&mut self) {
        if self.state.quick_view {
            self.state.preview_gen = self.state.preview_gen.wrapping_add(1);
        }
    }

    fn kick_preview(&mut self, sender: &ComponentSender<Self>) {
        if !self.state.quick_view || self.preview_sent == self.state.preview_gen {
            return;
        }
        let generation = self.state.preview_gen;
        self.preview.generation = generation;
        self.preview_sent = generation;
        let Some(path) = self.file_under_cursor() else {
            let text = self
                .state
                .cursor_entry(self.state.active)
                .filter(|entry| entry.is_dir())
                .map(|entry| format!("{}/", entry.name))
                .unwrap_or_default();
            self.preview.set_body(PreviewBody::Text(text));
            self.paint_preview();
            return;
        };
        let tx = sender.input_sender().clone();
        let charset = self.charset;
        let live = Arc::clone(&self.preview_live);
        live.store(generation, Ordering::Relaxed);
        if path.file_name().and_then(|name| name.to_str()).is_some_and(imageview::is_image) {
            let edge = if self.quick_fit { imageview::MAX_EDGE } else { u32::MAX };
            std::thread::spawn(move || {
                if live.load(Ordering::Relaxed) != generation {
                    return;
                }
                let body = preview_body(&path, edge);
                if live.load(Ordering::Relaxed) != generation {
                    return;
                }
                tx.emit(Msg::Preview { generation, body });
            });
            return;
        }
        std::thread::spawn(move || {
            if live.load(Ordering::Relaxed) != generation {
                return;
            }
            let text = match lister::Reader::open(&path) {
                Ok(mut reader) => {
                    if live.load(Ordering::Relaxed) != generation {
                        return;
                    }
                    reader
                        .read_window()
                        .map(|bytes| lister::render(ViewMode::Text, charset, &bytes, 0))
                        .unwrap_or_else(|e| e.to_string())
                }
                Err(err) => err.to_string(),
            };
            if live.load(Ordering::Relaxed) != generation {
                return;
            }
            tx.emit(Msg::Preview { generation, body: PreviewBody::Text(text) });
        });
    }

    fn paint_preview(&mut self) {
        let pane = self.state.active.other().index();
        let ui = &self.shell.panes[pane];
        if let Some(rev) = show_slot(ui, &self.preview, self.preview_shown[pane], self.quick_fit) {
            self.preview_shown[pane] = rev;
        }
    }

    fn file_under_cursor(&self) -> Option<PathBuf> {
        let entry = self.state.cursor_entry(self.state.active)?;
        if entry.is_dir() {
            return None;
        }
        Some(comarchy::listing::entry_path(&self.state.tab(self.state.active).path, entry))
    }

    fn open_external(&mut self, viewer: bool) {
        let Some(path) = self.file_under_cursor().or_else(|| {
            self.state.cursor_entry(self.state.active).map(|e| {
                comarchy::listing::entry_path(&self.state.tab(self.state.active).path, e)
            })
        }) else { return };
        let argv = if viewer {
            comarchy::external::viewer_argv(&self.cfg.viewer, &path)
        } else {
            comarchy::external::editor_argv(&self.cfg.editor, std::env::var("EDITOR").ok().as_deref(), &path)
        };
        if argv.is_empty() {
            self.status = if viewer { "No viewer found".into() } else { "No editor found".into() };
            return;
        }
        let mut cmd = std::process::Command::new(&argv[0]);
        cmd.args(&argv[1..]);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        if let Err(err) = cmd.spawn() {
            let line = format!("Cannot start {}: {err}", argv[0]);
            comarchy::log::error(&line);
            self.status = line;
        }
    }

    fn show_places(&mut self, id: PaneId, sender: &ComponentSender<Self>) {
        let tx = sender.input_sender().clone();
        let pane = id.index();
        let places = comarchy::places::places(&home_dir(), &self.cfg.bookmarks);
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
            button.connect_clicked(move |_| {
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
        self.hot_stack = vec![None];
        self.present_hot(sender);
    }

    fn present_hot(&mut self, sender: &ComponentSender<Self>) {
        if let Some(win) = self.hot.take() {
            win.close();
        }
        let view = self.hot_stack.last().copied().flatten();
        let current = self.state.tab(self.state.active).path.clone();
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
        if !comarchy::hotlist::is_dir(&mark) || mark.path.as_os_str().is_empty() {
            return;
        }
        let source = self.state.active;
        let target = mark.target.clone();
        let path = mark.path.clone();
        self.close_hot();
        if !path.is_dir() {
            self.status = format!("Not a directory: {}", path.display());
            return;
        }
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
        if comarchy::hotlist::dir_index(&self.cfg.bookmarks, &path).is_some() {
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
        let Some(index) = comarchy::hotlist::dir_index(&self.cfg.bookmarks, &path) else { return };
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

    fn ask_text(&mut self, title: &str, initial: &str, tx: relm4::Sender<Msg>) {
        self.close_dialog();
        let win = gtk::Window::new();
        win.set_title(Some(title));
        win.set_modal(true);
        win.set_transient_for(Some(&self.shell.window));
        win.set_default_width(520);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        column.set_margin_all(12);
        let entry = gtk::Entry::new();
        entry.set_text(initial);
        if title == "Password" {
            entry.set_visibility(false);
        }
        let tx_ok = tx.clone();
        entry.connect_activate(move |entry| tx_ok.emit(Msg::Submit(entry.text().to_string())));
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let ok = gtk::Button::with_label("OK");
        let entry_ok = entry.clone();
        let tx_ok = tx.clone();
        ok.connect_clicked(move |_| tx_ok.emit(Msg::Submit(entry_ok.text().to_string())));
        let cancel = gtk::Button::with_label("Cancel");
        cancel.connect_clicked(move |_| tx.emit(Msg::CancelAsk));
        row.append(&ok);
        row.append(&cancel);
        column.append(&entry);
        column.append(&row);
        win.set_child(Some(&column));
        win.present();
        entry.grab_focus();
        self.dialog = Some(win);
    }

    fn ask_confirm(&mut self, title: &str, tx: relm4::Sender<Msg>) {
        self.close_dialog();
        let win = gtk::Window::new();
        win.set_title(Some(title));
        win.set_modal(true);
        win.set_transient_for(Some(&self.shell.window));
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.set_margin_all(12);
        let ok = gtk::Button::with_label("OK");
        let tx_ok = tx.clone();
        ok.connect_clicked(move |_| tx_ok.emit(Msg::Submit(String::new())));
        let cancel = gtk::Button::with_label("Cancel");
        cancel.connect_clicked(move |_| tx.emit(Msg::CancelAsk));
        row.append(&ok);
        row.append(&cancel);
        win.set_child(Some(&row));
        win.present();
        self.dialog = Some(win);
    }

    fn ask_conflict(&mut self, src: &Path, dst: &Path, tx: relm4::Sender<Msg>) {
        self.close_dialog();
        let win = gtk::Window::new();
        win.set_title(Some("File exists"));
        win.set_modal(true);
        win.set_transient_for(Some(&self.shell.window));
        let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        column.set_margin_all(12);
        let label = gtk::Label::new(Some(&format!("{}\nalready exists as\n{}", src.display(), dst.display())));
        label.set_wrap(true);
        column.append(&label);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        for (caption, choice) in [
            ("Overwrite", Choice::Overwrite),
            ("All", Choice::OverwriteAll),
            ("Skip", Choice::Skip),
            ("Skip all", Choice::SkipAll),
            ("Cancel", Choice::Cancel),
        ] {
            let button = gtk::Button::with_label(caption);
            let tx = tx.clone();
            button.connect_clicked(move |_| tx.emit(Msg::Conflict(choice)));
            row.append(&button);
        }
        column.append(&row);
        win.set_child(Some(&column));
        win.present();
        self.dialog = Some(win);
    }

    fn ensure_progress(&mut self) {
        if self.progress.is_some() {
            return;
        }
        let win = gtk::Window::new();
        win.set_title(Some("Working"));
        win.set_modal(true);
        win.set_transient_for(Some(&self.shell.window));
        win.set_default_width(420);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        column.set_margin_all(12);
        let label = gtk::Label::new(Some(if self.status.is_empty() { "Working" } else { &self.status }));
        label.set_wrap(true);
        let bar = gtk::ProgressBar::new();
        bar.set_show_text(false);
        let cancel = gtk::Button::with_label("Cancel");
        if let Some(job) = &self.job {
            let flag = Arc::clone(&job.cancel);
            cancel.connect_clicked(move |_| flag.store(true, Ordering::Relaxed));
        }
        column.append(&label);
        column.append(&bar);
        column.append(&cancel);
        win.set_child(Some(&column));
        win.present();
        self.progress = Some((win, label, bar));
    }

    fn close_dialog(&mut self) {
        if let Some(win) = self.dialog.take() {
            win.close();
        }
    }

    fn apply_css(&self, palette: &Palette) {
        let image = self.background_path();
        let sheet = format!(
            "window, label, entry, textview, button {{ font-family: \"JetBrainsMono Nerd Font\", \"JetBrains Mono\", monospace; font-size: 13px; }}\n{}{}",
            theme::stylesheet(palette),
            theme::list_background(palette, &self.cfg.background_color, &image, self.cfg.background_alpha),
        );
        self.shell.css.load_from_data(&sheet);
    }

    fn watch_columns(&self, sender: &ComponentSender<Self>) {
        for pane in 0..2 {
            let cols = self.shell.panes[pane].list.columns();
            for i in 0..cols.n_items() {
                let Some(col) = cols.item(i).and_then(|obj| obj.downcast::<gtk::ColumnViewColumn>().ok()) else { continue };
                let tx = sender.input_sender().clone();
                col.connect_fixed_width_notify(move |_| {
                    if SIZING.load(Ordering::Relaxed) {
                        return;
                    }
                    tx.emit(Msg::ColumnSized(pane));
                });
            }
        }
    }

    fn char_unit(&self) -> i32 {
        let layout = self.shell.panes[0].path.create_pango_layout(Some("00000000"));
        (layout.pixel_size().0 / 8).max(7)
    }

    fn layout_columns(&mut self, pane: usize, entries: &[Entry]) {
        let widths = if self.cfg.auto_columns {
            let px = comarchy::listing::column_pixels(comarchy::listing::column_chars(entries), self.char_unit());
            [px[0], px[1], px[2], px[3], px[4]]
        } else if self.cfg.column_widths.len() == 5 {
            let w = &self.cfg.column_widths;
            [w[0], w[1], w[2], w[3], w[4]]
        } else {
            return;
        };
        self.apply_widths(pane, widths);
    }

    fn apply_widths(&mut self, pane: usize, widths: [i32; 5]) {
        SIZING.store(true, Ordering::Relaxed);
        let cols = self.shell.panes[pane].list.columns();
        let mut data = 0;
        for i in 0..cols.n_items() {
            let Some(col) = cols.item(i).and_then(|obj| obj.downcast::<gtk::ColumnViewColumn>().ok()) else { continue };
            if col.title().map(|title| title.is_empty()).unwrap_or(true) {
                continue;
            }
            let Some(width) = widths.get(data) else { break };
            data += 1;
            if col.fixed_width() != *width {
                col.set_fixed_width(*width);
            }
        }
        SIZING.store(false, Ordering::Relaxed);
    }

    fn pane_widths(&self, pane: usize) -> Vec<i32> {
        let cols = self.shell.panes[pane].list.columns();
        let mut out = Vec::new();
        for i in 0..cols.n_items() {
            if let Some(col) = cols.item(i).and_then(|obj| obj.downcast::<gtk::ColumnViewColumn>().ok()) {
                if col.title().map(|title| title.is_empty()).unwrap_or(true) {
                    continue;
                }
                out.push(col.fixed_width());
            }
        }
        out
    }

    fn on_column_sized(&mut self, pane: usize) {
        let widths = self.pane_widths(pane);
        if widths == self.cfg.column_widths {
            return;
        }
        let turned_off = self.cfg.auto_columns;
        self.cfg.auto_columns = false;
        self.cfg.column_widths = widths.clone();
        let _ = config::save(&self.cfg);
        if widths.len() == 5 {
            let other = 1 - pane;
            self.apply_widths(other, [widths[0], widths[1], widths[2], widths[3], widths[4]]);
        }
        if turned_off {
            self.status = "Column widths kept".into();
        }
        self.paint_settings_switch();
    }

    fn toggle_settings(&mut self, sender: &ComponentSender<Self>) {
        if self.settings.is_some() {
            self.close_settings();
            return;
        }
        self.open_settings(sender);
    }

    fn open_settings(&mut self, sender: &ComponentSender<Self>) {
        let tx = sender.input_sender().clone();
        let win = gtk::Window::new();
        win.set_title(Some("Settings"));
        win.set_modal(true);
        win.set_transient_for(Some(&self.shell.window));
        win.set_default_width(540);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let (editor_row, editor, editor_ids, editor_guard) = editor_picker(&self.cfg.editor, tx.clone());
        column.append(&editor_row);
        let (columns_row, columns) = settings_row("Auto column size", self.cfg.auto_columns);
        let tx_columns = tx.clone();
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| tx_columns.emit(Msg::ToggleAutoColumns));
        columns_row.add_controller(click);
        let (hidden_row, hidden) = settings_row("Show hidden files    Ctrl+H", self.state.show_hidden);
        let tx_hidden = tx.clone();
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| tx_hidden.emit(Msg::ToggleHidden));
        hidden_row.add_controller(click);
        let (hints_row, hints) = settings_row("Mode buttons", self.cfg.mode_buttons);
        let tx_hints = tx.clone();
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| tx_hints.emit(Msg::ToggleModeButtons));
        hints_row.add_controller(click);
        column.append(&columns_row);
        column.append(&hidden_row);
        column.append(&hints_row);
        let palette = theme::load_named(&theme::active_name(&self.cfg.theme, &config::active_theme_path()));
        let color_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        color_row.add_css_class("settings-row");
        let color_name = gtk::Label::new(Some("Background color"));
        color_name.set_xalign(0.0);
        color_name.set_hexpand(true);
        let color_btn = gtk::ColorButton::new();
        let shown = if self.cfg.background_color.is_empty() { palette.background.as_str() } else { self.cfg.background_color.as_str() };
        if let Ok(rgba) = gtk::gdk::RGBA::parse(shown) {
            color_btn.set_rgba(&rgba);
        }
        let tx_color = tx.clone();
        color_btn.connect_color_set(move |btn| tx_color.emit(Msg::BackgroundColor(hex_rgb(&btn.rgba()))));
        color_row.append(&color_name);
        color_row.append(&color_btn);
        let image_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        image_row.add_css_class("settings-row");
        let image_name = gtk::Label::new(Some("Background image"));
        image_name.set_xalign(0.0);
        image_name.set_hexpand(true);
        let image = gtk::Label::new(Some(&image_caption(&self.cfg.background)));
        image.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        image.set_max_width_chars(22);
        image_row.append(&image_name);
        image_row.append(&image);
        let tx_image = tx.clone();
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| tx_image.emit(Msg::PickBackground));
        image_row.add_controller(click);
        let strength_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        strength_row.add_css_class("settings-row");
        let strength_name = gtk::Label::new(Some("Image strength"));
        strength_name.set_xalign(0.0);
        let strength = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
        strength.set_value(f64::from(self.cfg.background_alpha));
        strength.set_digits(0);
        strength.set_hexpand(true);
        strength.set_draw_value(true);
        strength.set_value_pos(gtk::PositionType::Right);
        let tx_strength = tx.clone();
        strength.connect_value_changed(move |scale| tx_strength.emit(Msg::BackgroundAlpha(scale.value().round() as u8)));
        strength_row.append(&strength_name);
        strength_row.append(&strength);
        column.append(&color_row);
        column.append(&image_row);
        column.append(&strength_row);
        column.append(&tool_row("7-Zip", comarchy::pack::SEVEN_ZIP_HINT, &self.cfg.seven_zip, "/usr/bin/7z", tx.clone(), true));
        column.append(&tool_row("RAR", comarchy::pack::RAR_HINT, &self.cfg.rar, "/usr/bin/rar", tx.clone(), false));
        column.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        column.append(&commands_row(&win));
        win.set_child(Some(&column));
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let keys_win = win.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            let on_control = focused_control(&keys_win);
            match key {
                Key::Escape | Key::F12 => tx.emit(Msg::CloseSettings),
                Key::Return | Key::KP_Enter | Key::space | Key::KP_Space if !on_control => tx.emit(Msg::ToggleAutoColumns),
                Key::h | Key::H if !on_control => tx.emit(Msg::ToggleHidden),
                _ if on_control => return gtk::glib::Propagation::Proceed,
                _ => {}
            }
            gtk::glib::Propagation::Stop
        });
        win.add_controller(keys);
        win.present();
        win.grab_focus();
        self.settings = Some(SettingsUi { window: win, columns, hidden, hints, image, strength, editor, editor_ids, editor_guard });
    }

    fn set_editor(&mut self, id: String) {
        if id == comarchy::external::EDITOR_BROWSE {
            return;
        }
        if self.cfg.editor != id {
            self.cfg.editor = id;
            let _ = config::save(&self.cfg);
        }
        self.paint_editor();
    }

    fn paint_editor(&self) {
        let Some(ui) = &self.settings else { return };
        fill_editor(&ui.editor, &ui.editor_ids, &ui.editor_guard, &self.cfg.editor);
    }

    fn pick_editor(&mut self, sender: &ComponentSender<Self>) {
        let Some(parent) = self.settings.as_ref().map(|ui| ui.window.clone()) else { return };
        let dialog = gtk::FileChooserNative::new(
            Some("Editor program"),
            Some(&parent),
            gtk::FileChooserAction::Open,
            Some("Choose"),
            Some("Cancel"),
        );
        dialog.set_modal(true);
        let bin = PathBuf::from("/usr/bin");
        if bin.is_dir() {
            let _ = dialog.set_current_folder(Some(&gtk::gio::File::for_path(&bin)));
        }
        let tx = sender.input_sender().clone();
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept {
                if let Some(path) = dialog.file().and_then(|file| file.path()) {
                    tx.emit(Msg::SetEditor(comarchy::external::editor_id_for_path(&path)));
                }
            } else {
                tx.emit(Msg::EditorPickCancelled);
            }
            dialog.hide();
        });
        dialog.show();
        self.chooser = Some(dialog);
    }

    fn close_settings(&mut self) {
        if let Some(ui) = self.settings.take() {
            ui.window.close();
        }
    }

    fn toggle_auto_columns(&mut self) {
        self.cfg.auto_columns = !self.cfg.auto_columns;
        if self.cfg.auto_columns {
            self.cfg.column_widths.clear();
            self.relayout_columns();
        } else {
            self.cfg.column_widths = self.pane_widths(self.state.active.index());
        }
        let _ = config::save(&self.cfg);
        self.paint_settings_switch();
    }

    fn toggle_hidden(&mut self) {
        self.state.show_hidden = !self.state.show_hidden;
        self.cfg.show_hidden = self.state.show_hidden;
        let _ = config::save(&self.cfg);
        self.touch_both();
        self.paint_settings_switch();
    }

    fn toggle_mode_buttons(&mut self) {
        self.cfg.mode_buttons = !self.cfg.mode_buttons;
        let _ = config::save(&self.cfg);
        if let Some(ui) = &self.image {
            ui.bar.set_visible(self.cfg.mode_buttons);
        }
        self.paint_settings_switch();
    }

    fn toggle_image_scale(&mut self, sender: &ComponentSender<Self>) {
        let Some(ui) = self.image.as_mut() else { return };
        ui.fit = !ui.fit;
        apply_image_scale(ui);
        self.load_image(sender);
    }

    fn toggle_shown_scale(&mut self, sender: &ComponentSender<Self>) {
        if self.image.is_some() {
            self.toggle_image_scale(sender);
        } else {
            self.toggle_quick_scale();
        }
    }

    fn toggle_quick_scale(&mut self) {
        if !self.cursor_is_image() {
            return;
        }
        self.quick_fit = !self.quick_fit;
        self.note_preview_target();
    }

    fn step_shown_image(&mut self, delta: i32, sender: &ComponentSender<Self>) {
        if self.image.is_some() {
            self.step_image(delta as isize, sender);
        } else {
            self.step_quick_image(delta as isize);
        }
    }

    fn cursor_is_image(&self) -> bool {
        self.state
            .cursor_entry(self.state.active)
            .is_some_and(|entry| !entry.is_dir() && imageview::is_image(&entry.name))
    }

    fn paint_mode_bar(&self) {
        let image = self.cfg.mode_buttons && (self.image.is_some() || (self.state.quick_view && self.cursor_is_image()));
        let page = if image { "image" } else { "files" };
        if self.shell.mode_bar.visible_child_name().as_deref() != Some(page) {
            self.shell.mode_bar.set_visible_child_name(page);
        }
        let fit = self.image.as_ref().map(|ui| ui.fit).unwrap_or(self.quick_fit);
        let label = if fit { "S Fit" } else { "S 1:1" };
        if self.shell.image_scale.label().as_deref() != Some(label) {
            self.shell.image_scale.set_label(label);
        }
    }

    fn relayout_columns(&mut self) {
        for pane in 0..2 {
            let id = PaneId::from_index(pane);
            let entries: Vec<Entry> = self
                .state
                .visible(id)
                .into_iter()
                .map(|i| self.state.tab(id).entries[i].clone())
                .collect();
            self.layout_columns(pane, &entries);
        }
    }

    fn paint_settings_switch(&self) {
        let Some(ui) = &self.settings else { return };
        paint_switch(&ui.columns, self.cfg.auto_columns);
        paint_switch(&ui.hidden, self.state.show_hidden);
        paint_switch(&ui.hints, self.cfg.mode_buttons);
        ui.image.set_text(&image_caption(&self.cfg.background));
    }

    fn background_path(&self) -> String {
        let path = self.cfg.background.trim();
        if Path::new(path).is_file() { path.to_string() } else { String::new() }
    }

    fn restyle(&self) {
        let name = theme::active_name(&self.cfg.theme, &config::active_theme_path());
        self.apply_css(&theme::load_named(&name));
    }

    fn pick_background(&mut self, sender: &ComponentSender<Self>) {
        let Some(parent) = self.settings.as_ref().map(|ui| ui.window.clone()) else { return };
        let dialog = gtk::FileChooserNative::new(
            Some("Background image"),
            Some(&parent),
            gtk::FileChooserAction::Open,
            Some("Open"),
            Some("Cancel"),
        );
        dialog.set_modal(true);
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Images"));
        filter.add_mime_type("image/png");
        filter.add_mime_type("image/jpeg");
        filter.add_mime_type("image/webp");
        filter.add_mime_type("image/gif");
        dialog.set_filter(&filter);
        let path = PathBuf::from(&self.cfg.background);
        if path.is_file() {
            let _ = dialog.set_file(&gtk::gio::File::for_path(&path));
        } else {
            let pictures = home_dir().join("Pictures");
            if pictures.is_dir() {
                let _ = dialog.set_current_folder(Some(&gtk::gio::File::for_path(&pictures)));
            }
        }
        let tx = sender.input_sender().clone();
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept {
                if let Some(path) = dialog.file().and_then(|file| file.path()) {
                    tx.emit(Msg::BackgroundImage(path));
                }
            }
            dialog.hide();
        });
        dialog.show();
        self.chooser = Some(dialog);
    }

    fn set_background_image(&mut self, path: PathBuf) {
        self.chooser = None;
        self.cfg.background = path.display().to_string();
        if self.cfg.background_alpha == 0 {
            self.cfg.background_alpha = 70;
            if let Some(ui) = &self.settings {
                ui.strength.set_value(70.0);
            }
        }
        let _ = config::save(&self.cfg);
        self.restyle();
        self.paint_settings_switch();
    }

    fn set_background_color(&mut self, color: String) {
        if self.cfg.background_color == color {
            return;
        }
        self.cfg.background_color = color;
        let _ = config::save(&self.cfg);
        self.restyle();
    }

    fn set_background_alpha(&mut self, alpha: u8) {
        let alpha = alpha.min(100);
        if self.cfg.background_alpha == alpha {
            return;
        }
        self.cfg.background_alpha = alpha;
        let _ = config::save(&self.cfg);
        self.restyle();
    }

    fn sync(&mut self) {
        SYNCING.store(true, Ordering::Relaxed);
        let mut pending: [Option<Vec<Entry>>; 2] = [None, None];
        for pane in 0..2 {
            let id = PaneId::from_index(pane);
            let active = self.state.active == id;
            let show_preview = self.state.quick_view && !active;
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
                    let path = comarchy::listing::entry_path(&dir, &entry);
                    (entry, marked.contains(&path))
                }).collect()
            } else {
                Vec::new()
            };
            let ui = &self.shell.panes[pane];
            let class = if active { "pane-active" } else { "pane" };
            if !ui.frame.has_css_class(class) {
                ui.frame.set_css_classes(&[class]);
            }
            if ui.path.text().as_str() != path {
                ui.path.set_text(&path);
            }
            let show_filter = filter_on || ui.filter.has_focus();
            if gtk::prelude::WidgetExt::is_visible(&ui.filter) != show_filter {
                ui.filter.set_visible(show_filter);
            }
            let page = if show_preview { "preview" } else { "list" };
            if ui.stack.visible_child_name().as_deref() != Some(page) {
                ui.stack.set_visible_child_name(page);
            }
            if show_preview {
                if let Some(rev) = show_slot(ui, &self.preview, self.preview_shown[pane], self.quick_fit) {
                    self.preview_shown[pane] = rev;
                }
            }
            let labels = tab_labels(&tab_titles, tab_active);
            if tabs_current(&ui.tabs) != labels {
                comarchy::log::info(&format!("tabs rebuild pane={pane}"));
                while let Some(child) = ui.tabs.first_child() {
                    ui.tabs.remove(&child);
                }
                for label in &labels {
                    ui.tabs.append(&gtk::Button::with_label(label));
                }
            }
            if pane_dirty {
                comarchy::log::info(&format!("list reload pane={pane} rows={}", rows.len()));
                ui.store.remove_all();
                for (entry, marked) in &rows {
                    ui.store.append(&EntryObject::from_entry(entry, *marked));
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
        if self.shell.panes[0].path.text().as_str() != left.join("sub").display().to_string()
            || self.shell.panes[1].path.text().as_str() != right.display().to_string()
        {
            errors.push("navigate".into());
        }
        self.state.navigate(PaneId::Left, left.clone(), true);
        self.reload_pane(PaneId::Left);
        self.state.tab_mut(PaneId::Left).marked.insert(left.join("note.txt"));
        self.state.tab_mut(PaneId::Left).marked.insert(left.join("sub"));
        self.on_fkey(5, false, sender);
        self.on_submit(&right.display().to_string(), sender);
        if !self.wait_job(sender) || !right.join("note.txt").is_file() || !right.join("sub/b.txt").is_file() {
            errors.push("copy failed".into());
        }
        self.reload_pane(PaneId::Left);
        self.reload_pane(PaneId::Right);
        self.state.tab_mut(PaneId::Left).marked.clear();
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
        let mut trashed = false;
        for _ in 0..300 {
            if !trash_me.exists() {
                trashed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if !trashed {
            errors.push("trash failed".into());
        }

        self.reload_pane(PaneId::Left);
        if !self.point_at(PaneId::Left, "note.txt") {
            errors.push("missing note".into());
        }
        self.open_lister(sender);
        let lister_text = self.lister.as_ref().map(|ui| {
            let buf = ui.view.buffer();
            buf.text(&buf.start_iter(), &buf.end_iter(), false).to_string()
        });
        if !lister_text.unwrap_or_default().contains("hello comarchy") {
            errors.push("lister text".into());
        }
        self.close_lister();

        self.state.quick_view = true;
        self.preview.generation = self.state.preview_gen;
        self.preview.set_body(PreviewBody::Text("hello comarchy\n".into()));
        self.sync();
        if self.shell.panes[1].stack.visible_child_name().as_deref() != Some("preview") {
            errors.push("quick view hidden".into());
        }
        let preview = self.shell.panes[1].preview.buffer();
        if !preview.text(&preview.start_iter(), &preview.end_iter(), false).contains("hello comarchy") {
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
            if self.job.is_none() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    fn reload_pane(&mut self, id: PaneId) {
        let path = self.state.tab(id).path.clone();
        let Ok(entries) = comarchy::listing::read_dir(&path) else { return };
        let tab = self.state.pane(id).active;
        let token = self.state.tab(id).token;
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
            let path = comarchy::listing::entry_path(&self.state.tab(id).path, entry);
            self.state.tab(id).marked.contains(&path)
        });
        obj.set_marked(on);
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

fn preview_body(path: &Path, max_edge: u32) -> PreviewBody {
    match imageview::decode(path, max_edge) {
        Ok(img) => PreviewBody::Image { width: img.width, height: img.height, pixels: img.pixels },
        Err(err) => PreviewBody::Text(err),
    }
}

fn apply_image_scale(ui: &ImageUi) {
    let fit = ui.fit;
    ui.picture.set_content_fit(if fit { gtk::ContentFit::Contain } else { gtk::ContentFit::ScaleDown });
    ui.picture.set_can_shrink(fit);
    ui.picture.set_hexpand(fit);
    ui.picture.set_vexpand(fit);
    ui.picture.set_halign(if fit { gtk::Align::Fill } else { gtk::Align::Center });
    ui.picture.set_valign(if fit { gtk::Align::Fill } else { gtk::Align::Center });
    if fit {
        ui.picture.set_size_request(-1, -1);
    }
    let policy = if fit { gtk::PolicyType::Never } else { gtk::PolicyType::Automatic };
    ui.scroll.set_policy(policy, policy);
    ui.scale.set_label(if fit { "S Fit" } else { "S 1:1" });
}

fn show_slot(ui: &super::shell::PaneUi, slot: &lister::PreviewSlot, shown: u64, fit: bool) -> Option<u64> {
    match &slot.body {
        PreviewBody::Text(text) => {
            if ui.preview_kind.visible_child_name().as_deref() != Some("text") {
                ui.preview_kind.set_visible_child_name("text");
            }
            let buf = ui.preview.buffer();
            let current = buf.text(&buf.start_iter(), &buf.end_iter(), false);
            if current.as_str() != text {
                buf.set_text(text);
            }
            None
        }
        PreviewBody::Image { width, height, pixels } => {
            if ui.preview_kind.visible_child_name().as_deref() != Some("image") {
                ui.preview_kind.set_visible_child_name("image");
            }
            ui.picture.set_content_fit(if fit { gtk::ContentFit::Contain } else { gtk::ContentFit::ScaleDown });
            ui.picture.set_can_shrink(fit);
            ui.picture.set_size_request(if fit { -1 } else { *width as i32 }, if fit { -1 } else { *height as i32 });
            let policy = if fit { gtk::PolicyType::Never } else { gtk::PolicyType::Automatic };
            ui.picture_scroll.set_policy(policy, policy);
            if shown == slot.rev {
                return None;
            }
            set_texture(&ui.picture, *width, *height, pixels);
            Some(slot.rev)
        }
    }
}

fn set_texture(picture: &gtk::Picture, width: u32, height: u32, pixels: &[u8]) {
    let stride = width as usize * 4;
    if height == 0 || stride == 0 || pixels.len() < stride * height as usize {
        return;
    }
    let bytes = gtk::glib::Bytes::from(pixels);
    let texture = gtk::gdk::MemoryTexture::new(
        width as i32,
        height as i32,
        gtk::gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        stride,
    );
    picture.set_paintable(Some(&texture));
}

// Rows live in the inner list, which owns list.scroll-to-item.
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

fn image_caption(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() {
        return "none".into();
    }
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

fn hex_rgb(rgba: &gtk::gdk::RGBA) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        (rgba.red() * 255.0).round() as u8,
        (rgba.green() * 255.0).round() as u8,
        (rgba.blue() * 255.0).round() as u8,
    )
}

fn editor_picker(configured: &str, tx: relm4::Sender<Msg>) -> (gtk::Box, gtk::DropDown, Rc<RefCell<Vec<String>>>, Rc<Cell<bool>>) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("settings-row");
    let name = gtk::Label::new(Some("Editor    F4"));
    name.set_xalign(0.0);
    let choices = comarchy::external::editor_choices(configured);
    let labels: Vec<&str> = choices.iter().map(|choice| choice.label.as_str()).collect();
    let drop = gtk::DropDown::from_strings(&labels);
    drop.add_css_class("editor-pick");
    drop.set_hexpand(true);
    let ids = Rc::new(RefCell::new(choices.iter().map(|choice| choice.id.clone()).collect::<Vec<_>>()));
    let guard = Rc::new(Cell::new(false));
    let selected = choices.iter().position(|choice| choice.id == configured).unwrap_or(0) as u32;
    drop.set_selected(selected);
    let ids_notify = ids.clone();
    let guard_notify = guard.clone();
    drop.connect_selected_notify(move |drop| {
        if guard_notify.get() {
            return;
        }
        let id = ids_notify.borrow().get(drop.selected() as usize).cloned().unwrap_or_default();
        if id == comarchy::external::EDITOR_BROWSE {
            tx.emit(Msg::PickEditor);
            return;
        }
        tx.emit(Msg::SetEditor(id));
    });
    row.append(&name);
    row.append(&drop);
    (row, drop, ids, guard)
}

fn fill_editor(drop: &gtk::DropDown, ids: &Rc<RefCell<Vec<String>>>, guard: &Rc<Cell<bool>>, configured: &str) {
    let choices = comarchy::external::editor_choices(configured);
    let labels: Vec<&str> = choices.iter().map(|choice| choice.label.as_str()).collect();
    let selected = choices.iter().position(|choice| choice.id == configured).unwrap_or(0) as u32;
    guard.set(true);
    drop.set_model(Some(&gtk::StringList::new(&labels)));
    drop.set_selected(selected);
    *ids.borrow_mut() = choices.into_iter().map(|choice| choice.id).collect();
    guard.set(false);
}

fn focused_control(win: &gtk::Window) -> bool {
    let Some(mut widget) = relm4::gtk::prelude::GtkWindowExt::focus(win) else { return false };
    loop {
        if widget.downcast_ref::<gtk::Scale>().is_some()
            || widget.downcast_ref::<gtk::ColorButton>().is_some()
            || widget.downcast_ref::<gtk::DropDown>().is_some()
            || widget.downcast_ref::<gtk::Entry>().is_some()
            || widget.downcast_ref::<gtk::Button>().is_some()
        {
            return true;
        }
        match widget.parent() {
            Some(parent) => widget = parent,
            None => return false,
        }
    }
}

fn commands_row(parent: &gtk::Window) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("settings-row");
    let name = gtk::Label::new(Some("Keyboard"));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    let open = gtk::Button::with_label("Commands");
    open.add_css_class("fkey");
    let parent = parent.clone();
    open.connect_clicked(move |_| show_commands(&parent));
    row.append(&name);
    row.append(&open);
    row
}

fn show_commands(parent: &gtk::Window) {
    let win = gtk::Window::new();
    win.set_title(Some("Keyboard"));
    win.set_modal(true);
    win.set_transient_for(Some(parent));
    win.set_default_width(680);
    win.set_default_height(560);
    let columns = gtk::Box::new(gtk::Orientation::Horizontal, 36);
    columns.set_halign(gtk::Align::Start);
    columns.set_valign(gtk::Align::Start);
    columns.set_margin_start(16);
    columns.set_margin_end(16);
    columns.set_margin_bottom(16);
    columns.append(&command_groups(&[
        ("Sort", &[
            ("Ctrl+F3", "Name"),
            ("Ctrl+F4", "Extension"),
            ("Ctrl+F5", "Date"),
            ("Ctrl+F6", "Size"),
            ("Ctrl+F7", "Attributes"),
        ]),
        ("Navigate", &[
            ("Tab", "Other pane"),
            ("Ctrl+Tab", "Next tab"),
            ("Ctrl+T", "New tab"),
            ("Ctrl+W", "Close tab"),
            ("BackSpace", "Up"),
            ("Alt+Left", "Back"),
            ("Alt+Right", "Forward"),
            ("Ctrl+Left/Right", "Mirror folder"),
            ("Alt+F1", "Left places"),
            ("Alt+F2", "Right places"),
            ("Ctrl+D", "Hotlist"),
            ("Enter", "Open"),
            ("Insert", "Mark"),
            ("Space", "Mark"),
        ]),
        ("Lister", &[
            ("1", "Text"),
            ("2", "Binary"),
            ("3", "Hex"),
            ("A", "Ansi"),
            ("S / D", "Ascii"),
        ]),
    ]));
    columns.append(&command_groups(&[
        ("Files", &[
            ("F2", "Refresh"),
            ("F3", "View"),
            ("F4", "Edit"),
            ("F5", "Copy"),
            ("F6", "Move"),
            ("F7", "New folder"),
            ("F8 / Delete", "Trash"),
            ("Shift+F5", "Copy as"),
            ("Shift+F6", "Rename"),
            ("Shift+F8", "Delete"),
            ("F12", "Settings"),
        ]),
        ("Archive", &[
            ("Alt+F5", "Pack"),
            ("Alt+F9", "Unpack list"),
            ("Ctrl+Alt+F10", "Unpack here"),
            ("Shift+F9", "Create SFV"),
            ("Enter", "Verify SFV"),
        ]),
        ("View", &[
            ("Ctrl+S", "Filter"),
            ("Ctrl+H", "Hidden"),
            ("Ctrl+Q", "Quick view"),
            ("Alt+F3", "Viewer"),
            ("Escape", "Close view"),
            ("S", "Fit or 1:1"),
            ("Left/Right", "Previous or next"),
        ]),
    ]));
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_child(Some(&columns));
    win.set_child(Some(&scroll));
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let close = win.clone();
    keys.connect_key_pressed(move |_, key, _, _| {
        if matches!(key, Key::Escape | Key::F12) {
            close.close();
        }
        gtk::glib::Propagation::Stop
    });
    win.add_controller(keys);
    win.present();
}

fn command_groups(groups: &[(&str, &[(&str, &str)])]) -> gtk::Grid {
    let grid = gtk::Grid::new();
    grid.set_column_spacing(18);
    grid.set_row_spacing(3);
    let mut y = 0;
    for (title, rows) in groups {
        let head = gtk::Label::new(Some(title));
        head.set_xalign(0.0);
        head.add_css_class("cmd-head");
        grid.attach(&head, 0, y, 2, 1);
        y += 1;
        for (key, action) in *rows {
            let key_label = gtk::Label::new(Some(key));
            key_label.set_xalign(0.0);
            key_label.add_css_class("cmd-key");
            let action_label = gtk::Label::new(Some(action));
            action_label.set_xalign(0.0);
            action_label.set_hexpand(true);
            grid.attach(&key_label, 0, y, 1, 1);
            grid.attach(&action_label, 1, y, 1, 1);
            y += 1;
        }
    }
    grid
}

fn tool_row(title: &str, hint: &str, value: &str, placeholder: &str, tx: relm4::Sender<Msg>, seven: bool) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
    column.add_css_class("settings-row");
    column.set_margin_start(12);
    column.set_margin_end(12);
    column.set_margin_top(8);
    let name = gtk::Label::new(Some(title));
    name.set_xalign(0.0);
    let entry = gtk::Entry::new();
    entry.set_text(value);
    entry.set_placeholder_text(Some(placeholder));
    entry.connect_changed(move |entry| {
        let path = entry.text().to_string();
        if seven {
            tx.emit(Msg::SetSevenZip(path));
        } else {
            tx.emit(Msg::SetRar(path));
        }
    });
    let help = gtk::Label::new(Some(hint));
    help.set_wrap(true);
    help.set_xalign(0.0);
    help.set_max_width_chars(72);
    column.append(&name);
    column.append(&entry);
    column.append(&help);
    column
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or(text).to_string()
}

fn settings_row(title: &str, on: bool) -> (gtk::Box, gtk::Label) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let name = gtk::Label::new(Some(title));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    let value = gtk::Label::new(Some(if on { "on" } else { "off" }));
    if on {
        value.add_css_class("settings-on");
    }
    row.append(&name);
    row.append(&value);
    row.add_css_class("settings-row");
    (row, value)
}

fn paint_switch(label: &gtk::Label, on: bool) {
    label.set_text(if on { "on" } else { "off" });
    if on {
        label.add_css_class("settings-on");
    } else {
        label.remove_css_class("settings-on");
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

fn items_into(state: &AppState, dest_dir: &Path) -> Vec<(PathBuf, PathBuf)> {
    state.selected_paths(state.active).into_iter().map(|src| {
        let name = src.file_name().unwrap_or_default().to_os_string();
        (src, dest_dir.join(name))
    }).collect()
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

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).filter(|p| p.exists()).unwrap_or_else(|| PathBuf::from("/"))
}

fn smoke_dirs() -> (PathBuf, PathBuf) {
    let base = home_dir().join("comarchy-smoke");
    let left = base.join("left");
    let right = base.join("right");
    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::create_dir_all(left.join("sub"));
    let _ = std::fs::create_dir_all(&right);
    let _ = std::fs::write(left.join("note.txt"), "hello comarchy\n");
    let _ = std::fs::write(left.join("sub").join("b.txt"), "nested\n");
    let _ = std::fs::write(left.join("move-me.txt"), "move\n");
    (left, right)
}

fn gio_trash(path: &Path) -> std::io::Result<()> {
    let file = gtk::gio::File::for_path(path);
    if file.trash(gtk::gio::Cancellable::NONE).is_ok() {
        return Ok(());
    }
    let root = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".local/share"));
    let files = root.join("Trash/files");
    std::fs::create_dir_all(&files)?;
    let name = path.file_name().unwrap_or_default();
    let dest = files.join(name);
    if std::fs::rename(path, &dest).is_err() {
        std::fs::copy(path, &dest)?;
        let meta = std::fs::symlink_metadata(path)?;
        if meta.is_dir() && !meta.file_type().is_symlink() {
            std::fs::remove_dir_all(path)?;
        } else {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}

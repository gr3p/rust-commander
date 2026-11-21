// The disk usage mode: a bar and a percentage per row, from a count of every folder below the one on screen.
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use relm4::RelmWidgetExt;
use relm4::gtk;
use relm4::gtk::prelude::*;
use relm4::prelude::*;

use rust_commander::config;
use rust_commander::listing::format_size;
use rust_commander::places;
use rust_commander::state::PaneId;
use rust_commander::usage::{self, Medium, Mode, Options, Report, Snapshot, View, Watchdog};

use super::App;
use crate::ui::msg::Msg;
use crate::ui::row;

// A count shorter than this shows no window at all; the bars fill in by themselves.
const POPUP_AFTER: Duration = Duration::from_millis(300);
// A count that has read nothing for this long is waiting for a disk or a server.
const STALL_AFTER: Duration = Duration::from_secs(10);
const TIMER_EVERY: Duration = Duration::from_millis(250);

pub(in crate::ui::app) struct UsageMode {
    pub tab: usize,
    pub view: View,
    pub medium: Medium,
}

struct Popup {
    window: gtk::Window,
    label: gtk::Label,
    bar: gtk::ProgressBar,
}

pub(in crate::ui::app) struct UsageScan {
    serial: u64,
    pane: usize,
    root: PathBuf,
    mode: Mode,
    medium: Medium,
    cancel: Arc<AtomicBool>,
    // Set when the count is over or replaced, so its timer stops.
    over: Arc<AtomicBool>,
    started: Instant,
    watchdog: Watchdog,
    entries: u64,
    total: u64,
    // How much of the disk is in use, when this folder is the whole disk. Gives the window a true percentage.
    disk_used: Option<u64>,
    popup: Option<Popup>,
}

impl App {
    /// The mode of a pane, while the folder and tab it was counted for are still on screen.
    pub(in crate::ui::app) fn usage_valid(&self, pane: usize) -> Option<&UsageMode> {
        let id = PaneId::from_index(pane);
        self.usage_modes[pane]
            .as_ref()
            .filter(|mode| mode.tab == self.state.pane(id).active && mode.view.path == self.state.tab(id).path)
    }

    pub(in crate::ui::app) fn usage_scan_pane(&self) -> Option<usize> {
        self.usage_scan.as_ref().map(|scan| scan.pane)
    }

    // First press counts. While counting it cancels, and once the bars are there it switches the mode off.
    pub(in crate::ui::app) fn toggle_usage(&mut self, sender: &ComponentSender<Self>) {
        let pane = self.state.active.index();
        if self.usage_scan.as_ref().is_some_and(|scan| scan.pane == pane) {
            self.cancel_usage();
            return;
        }
        if self.usage_valid(pane).is_some() {
            self.usage_off(pane);
            self.status = "Disk usage off".into();
            return;
        }
        self.begin_usage(pane, sender);
    }

    fn begin_usage(&mut self, pane: usize, sender: &ComponentSender<Self>) {
        let id = PaneId::from_index(pane);
        let tab = self.state.tab(id);
        if tab.feed {
            self.status = "Disk usage needs a folder, not a list of search hits".into();
            return;
        }
        let path = tab.path.clone();
        let medium = usage::medium_of(&path);
        if medium == Medium::Network {
            let (mount, kind) = usage::mount_of(&path).unwrap_or_else(|| (path.clone(), "network".into()));
            let mount = mount.display().to_string();
            if !self.cfg.usage_network_ok.contains(&mount) {
                self.ask_network_count(pane, mount, &kind);
                return;
            }
        }
        self.start_usage(pane, medium, sender);
    }

    fn ask_network_count(&mut self, pane: usize, mount: String, kind: &str) {
        let dialog = gtk::AlertDialog::builder()
            .modal(true)
            .message("Count folder sizes on a network disk?")
            .detail(format!(
                "{mount} is a network disk ({kind}). Counting reads every folder below this one, which can be slow and \
                 puts load on the server. A server that stops answering can make it wait."
            ))
            .buttons(["Cancel", "Count", "Always for this disk"])
            .default_button(0)
            .cancel_button(0)
            .build();
        let tx = self.tx.clone();
        dialog.choose(Some(&self.shell.window), None::<&gtk::gio::Cancellable>, move |choice| match choice {
            Ok(1) => tx.emit(Msg::UsageGo(pane, None)),
            Ok(2) => tx.emit(Msg::UsageGo(pane, Some(mount.clone()))),
            _ => {}
        });
    }

    pub(in crate::ui::app) fn usage_go(&mut self, pane: usize, remember: Option<String>, sender: &ComponentSender<Self>) {
        if let Some(mount) = remember {
            if !self.cfg.usage_network_ok.contains(&mount) {
                self.cfg.usage_network_ok.push(mount);
                let _ = config::save(&self.cfg);
            }
        }
        self.start_usage(pane, Medium::Network, sender);
    }

    pub(in crate::ui::app) fn start_usage(&mut self, pane: usize, medium: Medium, sender: &ComponentSender<Self>) {
        let _ = sender;
        self.start_usage_now(pane, medium);
    }

    fn start_usage_now(&mut self, pane: usize, medium: Medium) {
        self.stop_scan();
        let id = PaneId::from_index(pane);
        let root = self.state.tab(id).path.clone();
        let mode = Mode { apparent: self.cfg.usage_apparent, same_device: true };
        // Until the count says anything, show what the cache kept from an earlier one.
        let known: Vec<(String, bool, u64)> = self
            .state
            .tab(id)
            .entries
            .iter()
            .filter(|entry| entry.name.as_ref() != "..")
            .map(|entry| (entry.name.to_string(), entry.is_dir(), entry.size))
            .collect();
        let view = View::prefilled(&root, mode, &known, &self.usage_cache, Instant::now());
        self.usage_modes[pane] = Some(UsageMode { tab: self.state.pane(id).active, view, medium });
        self.apply_usage_sizes(pane);
        self.touch(id);

        self.usage_gen += 1;
        let serial = self.usage_gen;
        let cancel = Arc::new(AtomicBool::new(false));
        let over = Arc::new(AtomicBool::new(false));
        let opts = Options { root: root.clone(), mode, threads: usage::threads_for(medium, usage::cores()) };
        let (tx, flag, threads) = (self.tx.clone(), Arc::clone(&cancel), opts.threads);
        std::thread::spawn(move || {
            // A disk read from its mount point has a known amount in use, which makes a real percentage.
            if let Ok(info) = places::disk_info(&opts.root) {
                if info.mount == opts.root {
                    tx.emit(Msg::UsageDisk(serial, info.used()));
                }
            }
            let count = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                usage::run(&opts, &|| flag.load(Ordering::Relaxed), &mut |snap| tx.emit(Msg::UsageTick(serial, snap.clone())))
            }));
            let result = match count {
                Ok(Ok(report)) => Ok(report),
                Ok(Err(err)) => Err(format!("{}: {err}", opts.root.display())),
                Err(_) => Err("The count stopped unexpectedly".to_string()),
            };
            tx.emit(Msg::UsageDone(serial, result));
        });
        let (tx, stop) = (self.tx.clone(), Arc::clone(&over));
        gtk::glib::timeout_add_local(TIMER_EVERY, move || {
            if stop.load(Ordering::Relaxed) {
                return gtk::glib::ControlFlow::Break;
            }
            tx.emit(Msg::UsageTimer(serial));
            gtk::glib::ControlFlow::Continue
        });
        rust_commander::log::info(&format!("usage start {} threads={threads}", root.display()));
        self.status = format!("Counting {}", root.display());
        self.usage_scan = Some(UsageScan {
            serial,
            pane,
            root,
            mode,
            medium,
            cancel,
            over,
            started: Instant::now(),
            watchdog: Watchdog::new(Instant::now()),
            entries: 0,
            total: 0,
            disk_used: None,
            popup: None,
        });
    }

    // The thread of a stopped count may live on, stuck in the kernel. Its answers carry an old number and are dropped.
    fn stop_scan(&mut self) {
        let Some(scan) = self.usage_scan.take() else { return };
        scan.cancel.store(true, Ordering::Relaxed);
        scan.over.store(true, Ordering::Relaxed);
        if let Some(popup) = scan.popup {
            popup.window.close();
        }
    }

    pub(in crate::ui::app) fn cancel_usage(&mut self) {
        let Some(pane) = self.usage_scan.as_ref().map(|scan| scan.pane) else { return };
        self.stop_scan();
        self.usage_off(pane);
        self.status = "Disk usage cancelled".into();
    }

    pub(in crate::ui::app) fn usage_off(&mut self, pane: usize) {
        self.usage_follow[pane] = false;
        if self.usage_scan.as_ref().is_some_and(|scan| scan.pane == pane) {
            self.stop_scan();
        }
        if self.usage_modes[pane].take().is_none() {
            return;
        }
        let id = PaneId::from_index(pane);
        for entry in self.state.tab_mut(id).entries.iter_mut().filter(|entry| entry.is_dir()) {
            entry.size = 0;
        }
        self.touch(id);
    }

    pub(in crate::ui::app) fn on_usage_disk(&mut self, serial: u64, used: u64) {
        if let Some(scan) = self.usage_scan.as_mut().filter(|scan| scan.serial == serial) {
            scan.disk_used = Some(used);
        }
    }

    pub(in crate::ui::app) fn on_usage_tick(&mut self, serial: u64, snap: Snapshot) {
        let Some(scan) = self.usage_scan.as_mut().filter(|scan| scan.serial == serial) else { return };
        scan.watchdog.feed(snap.entries, Instant::now());
        scan.entries = snap.entries;
        scan.total = snap.total;
        let (pane, mode) = (scan.pane, scan.mode);
        let Some(held) = self.usage_modes[pane].as_mut() else { return };
        held.view = View::from_counts(&held.view.path, mode, &snap.children, &snap.bytes, &snap.open, true);
        self.paint_usage_rows(pane);
        self.paint_usage_popup();
    }

    pub(in crate::ui::app) fn on_usage_timer(&mut self, serial: u64) {
        let Some(scan) = self.usage_scan.as_ref().filter(|scan| scan.serial == serial) else { return };
        let pane = scan.pane;
        // The folder or tab changed under the count: nobody looks at it any more.
        if self.usage_valid(pane).is_none() {
            self.stop_scan();
            self.usage_modes[pane] = None;
            self.touch(PaneId::from_index(pane));
            return;
        }
        // Turns the spinners of the folders still being counted.
        self.paint_usage_rows(pane);
        let Some(scan) = self.usage_scan.as_ref() else { return };
        let stalled = scan.watchdog.stalled(Instant::now(), STALL_AFTER);
        // A network disk is slow by nature, so it gets its window at once. A stalled count always does.
        let due = scan.started.elapsed() >= POPUP_AFTER || scan.medium == Medium::Network || stalled;
        if due && scan.popup.is_none() {
            let popup = self.usage_popup();
            if let Some(scan) = self.usage_scan.as_mut() {
                scan.popup = Some(popup);
            }
        }
        self.paint_usage_popup();
    }

    fn usage_popup(&self) -> Popup {
        let window = gtk::Window::new();
        window.set_title(Some("Disk usage"));
        window.set_transient_for(Some(&self.shell.window));
        window.set_default_width(520);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        column.set_margin_all(12);
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.set_wrap(true);
        label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        label.set_lines(4);
        let bar = gtk::ProgressBar::new();
        bar.add_css_class("job");
        bar.set_show_text(false);
        let cancel = gtk::Button::with_label("Cancel");
        cancel.set_halign(gtk::Align::End);
        let tx = self.tx.clone();
        crate::ui::shell::on_click(&cancel, move || tx.emit(Msg::UsageCancel));
        let tx = self.tx.clone();
        // Closing the window is the same as Cancel.
        window.connect_close_request(move |_| {
            tx.emit(Msg::UsageCancel);
            gtk::glib::Propagation::Proceed
        });
        let esc = window.clone();
        crate::ui::shell::on_escape(&window, move || esc.close());
        column.append(&label);
        column.append(&bar);
        column.append(&cancel);
        window.set_child(Some(&column));
        window.present();
        Popup { window, label, bar }
    }

    fn paint_usage_popup(&self) {
        let Some(scan) = &self.usage_scan else { return };
        let Some(popup) = &scan.popup else { return };
        let mut text = format!("{}\n{} items, {}", scan.root.display(), scan.entries, format_size(scan.total));
        let now = Instant::now();
        if scan.watchdog.stalled(now, STALL_AFTER) {
            text.push_str(&format!(
                "\nNo answer from the disk or server for {} s. Still waiting, or press Cancel.",
                scan.watchdog.quiet_for(now).as_secs()
            ));
        }
        popup.label.set_text(&text);
        match scan.disk_used {
            Some(used) if used > 0 => popup.bar.set_fraction((scan.total as f64 / used as f64).min(0.99)),
            _ => popup.bar.pulse(),
        }
    }

    pub(in crate::ui::app) fn on_usage_done(&mut self, serial: u64, result: Result<Report, String>) {
        if self.usage_scan.as_ref().is_none_or(|scan| scan.serial != serial) {
            return;
        }
        let Some(scan) = self.usage_scan.take() else { return };
        scan.over.store(true, Ordering::Relaxed);
        let spent = scan.started.elapsed();
        if let Some(popup) = &scan.popup {
            popup.window.close();
        }
        let pane = scan.pane;
        let id = PaneId::from_index(pane);
        let report = match result {
            Ok(report) if !report.cancelled => report,
            Ok(_) => {
                self.usage_off(pane);
                self.status = "Disk usage cancelled".into();
                return;
            }
            Err(err) => {
                rust_commander::log::error(&format!("usage {err}"));
                self.usage_off(pane);
                self.status = err;
                return;
            }
        };
        rust_commander::log::info(&format!(
            "usage done {} entries={} total={} unreadable={} in {:.2}s",
            scan.root.display(),
            report.entries,
            report.total,
            report.unreadable,
            spent.as_secs_f64()
        ));
        if !report.failed {
            self.usage_cache.store(&report, scan.mode, scan.medium == Medium::Network, Instant::now());
        }
        if let Some(held) = self.usage_modes[pane].as_mut() {
            held.view = View::from_counts(&scan.root, scan.mode, &report.children, &report.bytes, &[], report.failed);
        }
        self.apply_usage_sizes(pane);
        self.touch(id);
        let mut note = format!("{} in {} items", format_size(report.total), report.entries);
        // The share of the whole disk. A network disk is not asked here, since that call could wait on the server.
        if scan.medium != Medium::Network {
            if let Some(info) = places::disk_info(&scan.root).ok().filter(|info| info.total > 0) {
                note.push_str(&format!(" ({} of {})", usage::percent_text(report.total, info.total), format_size(info.total)));
            }
        }
        if report.unreadable > 0 {
            note.push_str(&format!(", {} could not be read", report.unreadable));
        }
        if report.skipped_mounts > 0 {
            note.push_str(&format!(", {} other mounts left out", report.skipped_mounts));
        }
        if report.too_deep > 0 {
            note.push_str(", some folders too deep");
        }
        if report.failed {
            note.push_str(". The count stopped early, the numbers are partial");
        }
        self.status = format!("Disk usage: {note}");
    }

    // Folder sizes go into the entries too, so sorting by size and the Size column follow the count.
    pub(in crate::ui::app) fn apply_usage_sizes(&mut self, pane: usize) {
        let id = PaneId::from_index(pane);
        let Some(held) = self.usage_modes[pane].as_ref() else { return };
        for entry in self.state.tab_mut(id).entries.iter_mut().filter(|entry| entry.is_dir() && entry.name.as_ref() != "..") {
            entry.size = held.view.cell(&entry.name).filter(|cell| !cell.foreign).map_or(0, |cell| cell.bytes);
        }
    }

    // A listing arrived. The same folder keeps its numbers, another one ends the mode.
    pub(in crate::ui::app) fn usage_after_listing(&mut self, pane: usize) {
        if self.usage_modes[pane].is_none() {
            return;
        }
        if self.usage_valid(pane).is_some() {
            self.apply_usage_sizes(pane);
        } else if self.usage_follow[pane] {
            self.follow_into(pane);
        } else {
            self.usage_off(pane);
        }
    }

    // Ctrl+R: count every folder the pane opens from now on. Pressed again it stops following and keeps the bars.
    pub(in crate::ui::app) fn toggle_usage_follow(&mut self, sender: &ComponentSender<Self>) {
        let pane = self.state.active.index();
        if self.usage_follow[pane] {
            self.usage_follow[pane] = false;
            self.status = "Disk usage no longer follows folders".into();
            return;
        }
        let counting = self.usage_scan.as_ref().is_some_and(|scan| scan.pane == pane);
        if !counting && self.usage_valid(pane).is_none() {
            self.begin_usage(pane, sender);
            // A network disk asks first. Following waits for the answer and the count.
            if self.usage_scan.as_ref().is_none_or(|scan| scan.pane != pane) {
                return;
            }
        }
        self.usage_follow[pane] = true;
        self.status = "Disk usage follows folders. Ctrl+R stops following, Ctrl+U turns it off".into();
    }

    // A folder opened in follow mode. A network disk is only counted on its own when it has been allowed for good.
    fn follow_into(&mut self, pane: usize) {
        let path = self.state.tab(PaneId::from_index(pane)).path.clone();
        if self.state.tab(PaneId::from_index(pane)).feed {
            self.usage_off(pane);
            return;
        }
        let medium = usage::medium_of(&path);
        if medium == Medium::Network {
            let mount = usage::mount_of(&path).map(|(mount, _)| mount.display().to_string()).unwrap_or_default();
            if !self.cfg.usage_network_ok.contains(&mount) {
                self.usage_off(pane);
                self.status = "Disk usage stopped following: this is a network disk. Ctrl+U counts it".into();
                return;
            }
        }
        self.start_usage_now(pane, medium);
    }

    pub(in crate::ui::app) fn paint_usage_rows(&self, pane: usize) {
        let view = self.usage_valid(pane).map(|held| &held.view);
        let store = &self.shell.panes[pane].store;
        for at in 0..store.n_items() {
            if let Some(obj) = store.item(at).and_then(|obj| obj.downcast::<row::EntryObject>().ok()) {
                row::paint_usage_row(&obj, view);
            }
        }
    }

    // Our own copies, moves and deletes change sizes. Both panes' folders are marked as out of date in the cache, and
    // a pane showing bars counts again, except on a network disk where that is the user's call.
    pub(in crate::ui::app) fn usage_after_job(&mut self, sender: &ComponentSender<Self>) {
        for id in [PaneId::Left, PaneId::Right] {
            let path = self.state.tab(id).path.clone();
            self.usage_cache.touch(&path);
        }
        for pane in 0..2 {
            let Some(medium) = self.usage_valid(pane).map(|held| held.medium) else { continue };
            if medium != Medium::Network {
                self.start_usage(pane, medium, sender);
            }
        }
    }

    // Refresh in the mode counts again.
    pub(in crate::ui::app) fn usage_after_refresh(&mut self, pane: usize, sender: &ComponentSender<Self>) {
        let Some(medium) = self.usage_valid(pane).map(|held| held.medium) else { return };
        let path = self.state.tab(PaneId::from_index(pane)).path.clone();
        self.usage_cache.touch(&path);
        self.start_usage(pane, medium, sender);
    }
}

// The disk usage mode through its real handlers. The count runs in this thread, so no waiting is needed, and the
// messages of the thread that start_usage also launches arrive with an old number and are dropped.
impl App {
    pub(in crate::ui::app) fn smoke_usage(&mut self, errors: &mut Vec<String>, sender: &ComponentSender<Self>) {
        let id = PaneId::Left;
        self.state.active = id;
        self.reload_pane(id);
        self.sync();
        // `sub` holds files. A folder made by an earlier step may be empty, and an empty one has no share.
        let Some(folder) = self.state.tab(id).entries.iter().find(|e| e.is_dir() && e.name.as_ref() == "sub").map(|e| e.name.to_string()) else {
            errors.push("usage: no folder to count".into());
            return;
        };
        let row = |app: &App, name: &str| {
            let store = &app.shell.panes[0].store;
            (0..store.n_items())
                .filter_map(|at| store.item(at).and_then(|obj| obj.downcast::<row::EntryObject>().ok()))
                .find(|obj| obj.entry_name() == name)
        };
        let count = |app: &mut App, errors: &mut Vec<String>| {
            let Some(scan) = app.usage_scan.as_ref() else {
                errors.push("usage: no count started".into());
                return;
            };
            let (serial, opts) = (scan.serial, Options { root: scan.root.clone(), mode: scan.mode, threads: 2 });
            let mut last = None;
            let report = usage::run(&opts, &|| false, &mut |snap| last = Some(snap.clone()));
            let Ok(report) = report else {
                errors.push("usage: count failed".into());
                return;
            };
            if let Some(snap) = last {
                app.on_usage_tick(serial, snap);
            }
            app.on_usage_done(serial, Ok(report));
            app.sync();
        };

        self.toggle_usage(sender);
        self.sync();
        if self.usage_scan.is_none() || self.usage_valid(0).is_none() {
            errors.push("usage: mode did not start".into());
            return;
        }
        if !self.shell.panes[0].usage_col.is_visible() {
            errors.push("usage: column hidden while counting".into());
        }
        count(self, errors);
        if self.usage_scan.is_some() {
            errors.push("usage: count never finished".into());
        }
        match row(self, &folder) {
            Some(obj) if obj.usage_on() && !obj.usage_partial() && obj.usage_text().ends_with('%') => {
                if obj.usage_frac() <= 0.0 || obj.usage_frac() > 1.0 {
                    errors.push(format!("usage: share {}", obj.usage_frac()));
                }
                if obj.size_text() == "<DIR>" || obj.size_text().starts_with('~') {
                    errors.push(format!("usage: size column {}", obj.size_text()));
                }
            }
            _ => errors.push("usage: folder row has no finished bar".into()),
        }
        if row(self, "..").is_some_and(|obj| obj.usage_on()) {
            errors.push("usage: bar on ..".into());
        }
        let hints = |app: &App| -> Vec<String> {
            app.shell.hints.iter().filter(|hint| hint.button.is_visible()).filter_map(|hint| hint.button.label().map(|l| l.to_string())).collect()
        };
        let shown = hints(self);
        if !shown.iter().any(|label| label.contains("Follow folders")) || !shown.iter().any(|label| label.contains("Disk usage off")) {
            errors.push(format!("usage: context row lacks the next steps: {shown:?}"));
        }

        // Our own file jobs and Refresh count again. The old numbers stay on screen, dimmed, until the new ones arrive.
        for (name, restart) in [("job", true), ("refresh", false)] {
            if restart {
                self.usage_after_job(sender);
            } else {
                self.refresh_active(sender);
            }
            self.sync();
            let dimmed = self.usage_modes[0].as_ref().is_some_and(|held| held.view.stale);
            if self.usage_scan.is_none() || !dimmed {
                errors.push(format!("usage: no new count after {name}"));
            }
            // A small folder is not in the cache, so a file shows the dimming.
            if row(self, "note.txt").is_some_and(|obj| !obj.usage_on() || !obj.usage_partial()) {
                errors.push(format!("usage: numbers not dimmed during the count after {name}"));
            }
            count(self, errors);
        }

        // An answer from an older count changes nothing.
        self.toggle_usage(sender);
        self.sync();
        if self.usage_valid(0).is_some() || self.shell.panes[0].usage_col.is_visible() {
            errors.push("usage: mode stayed on".into());
        }
        if row(self, &folder).is_some_and(|obj| obj.usage_on() || obj.size_text() != "<DIR>") {
            errors.push("usage: row kept its bar after off".into());
        }
        self.toggle_usage(sender);
        let serial = self.usage_scan.as_ref().map_or(0, |scan| scan.serial);
        // A slow count gets its window, and one that reads nothing says that it is waiting.
        if let Some(scan) = self.usage_scan.as_mut() {
            scan.started = Instant::now() - Duration::from_secs(2);
            scan.watchdog = Watchdog::new(Instant::now() - Duration::from_secs(60));
        }
        self.on_usage_timer(serial);
        let waiting = self.usage_scan.as_ref().and_then(|scan| scan.popup.as_ref()).map(|popup| popup.label.text().to_string());
        match waiting {
            None => errors.push("usage: no window for a slow count".into()),
            Some(text) if !text.contains("No answer") => errors.push(format!("usage: stall not shown: {text}")),
            Some(_) => {}
        }
        let stale = serial + 1000;
        self.on_usage_done(stale, Err("old".into()));
        if self.usage_scan.is_none() {
            errors.push("usage: an old answer ended the count".into());
        }
        // Escape and the cancel button end a running count and leave nothing behind.
        self.cancel_usage();
        self.sync();
        if self.usage_scan.is_some() || self.usage_modes[0].is_some() || self.shell.panes[0].usage_col.is_visible() {
            errors.push("usage: cancel left the mode on".into());
        }

        // Another folder ends the mode.
        self.toggle_usage(sender);
        count(self, errors);
        let home = self.state.tab(id).path.clone();
        self.state.tab_mut(id).path = std::path::PathBuf::from("/");
        self.usage_after_listing(0);
        if self.usage_modes[0].is_some() {
            errors.push("usage: kept after leaving the folder".into());
        }
        self.state.tab_mut(id).path = home.clone();
        self.reload_pane(id);
        self.sync();

        // Ctrl+R follows: a folder opened from here is counted on its own.
        self.toggle_usage_follow(sender);
        self.sync();
        if !hints(self).iter().any(|label| label.contains("Stop following")) {
            errors.push(format!("usage: context row does not offer to stop following: {:?}", hints(self)));
        }
        if !self.usage_follow[0] || self.usage_scan.is_none() {
            errors.push("usage: Ctrl+R did not start a following count".into());
        }
        count(self, errors);
        let open_folder = |app: &mut App, path: std::path::PathBuf| {
            let token = app.state.navigate(id, path.clone(), true);
            app.loads.push((token, Arc::new(AtomicBool::new(false))));
            let entries = rust_commander::listing::read_dir(&path).unwrap_or_default();
            let tab = app.state.pane(id).active;
            app.on_listed(0, tab, token, Ok(entries), None);
        };
        open_folder(self, home.join(&folder));
        let followed = self.usage_scan.as_ref().is_some_and(|scan| scan.root == home.join(&folder));
        if !followed || self.usage_valid(0).is_none() {
            errors.push("usage: following did not count the folder it opened".into());
        }
        count(self, errors);
        // Ctrl+R again stops following and keeps the bars. The next folder ends the mode.
        self.toggle_usage_follow(sender);
        if self.usage_follow[0] || self.usage_valid(0).is_none() {
            errors.push("usage: Ctrl+R off dropped the bars or kept following".into());
        }
        open_folder(self, home.clone());
        if self.usage_modes[0].is_some() || self.usage_scan.is_some() {
            errors.push("usage: counted after following was off".into());
        }
        self.sync();
        if hints(self).iter().any(|label| label.contains("Follow folders") || label.contains("Disk usage off")) {
            errors.push(format!("usage: context row kept the mode's steps: {:?}", hints(self)));
        }
        self.reload_pane(id);
        self.sync();
    }
}

// The count on a mount that is slow or goes silent, through the real message loop. RUST_COMMANDER_SMOKE_NETFS names
// the mount, and RUST_COMMANDER_SMOKE_NETFS_MODE is `slow` (the count must finish) or `hang` (it must say it waits,
// and Cancel must give the window back at once).
impl App {
    pub(in crate::ui::app) fn smoke_net(&mut self, step: u32, sender: &ComponentSender<Self>) {
        let Some(mount) = std::env::var_os("RUST_COMMANDER_SMOKE_NETFS").map(PathBuf::from) else { return };
        let hang = std::env::var("RUST_COMMANDER_SMOKE_NETFS_MODE").is_ok_and(|mode| mode == "hang");
        let fail = |why: &str| -> ! {
            eprintln!("SMOKE_NET_FAIL {why}");
            std::process::exit(1)
        };
        let id = PaneId::Left;
        match step {
            0 => {
                if usage::medium_of(&mount) != Medium::Network {
                    fail("the mount is not seen as a network disk");
                }
                let token = self.state.navigate(id, mount.clone(), true);
                let Ok(entries) = rust_commander::listing::read_dir(&mount) else { fail("cannot list the mount") };
                self.state.apply_listing(id, self.state.pane(id).active, token, entries);
                self.state.active = id;
                self.touch(id);
            }
            1 => {
                // The question about a network disk comes first, and nothing starts until it is answered.
                self.toggle_usage(sender);
                if self.usage_scan.is_some() || self.usage_modes[0].is_some() {
                    fail("a network count started without asking");
                }
            }
            2 => {
                // "Always for this disk" starts the count and remembers the disk.
                self.usage_go(0, Some(mount.display().to_string()), sender);
                if self.usage_scan.is_none() {
                    fail("the count did not start");
                }
                if !self.cfg.usage_network_ok.contains(&mount.display().to_string()) {
                    fail("the disk was not remembered");
                }
            }
            3..=60 => {
                let shown = self.usage_scan.as_ref().and_then(|scan| scan.popup.as_ref()).map(|popup| popup.label.text().to_string());
                if !hang {
                    if self.usage_scan.is_none() {
                        let ok = self.usage_valid(0).is_some_and(|held| !held.view.partial && held.view.total() > 0);
                        if !ok {
                            fail("the count ended without numbers");
                        }
                        println!("SMOKE_NET_OK finished: {}", self.status);
                        std::process::exit(0);
                    }
                    return;
                }
                if step == 6 && self.usage_scan.as_ref().is_none_or(|scan| scan.popup.is_none()) {
                    fail("a network count showed no window");
                }
                if shown.as_ref().is_some_and(|text| text.contains("No answer")) {
                    let start = Instant::now();
                    self.cancel_usage();
                    self.sync();
                    if start.elapsed() > Duration::from_millis(500) {
                        fail("Cancel was slow");
                    }
                    if self.usage_scan.is_some() || self.usage_modes[0].is_some() {
                        fail("Cancel left the count behind");
                    }
                    println!("SMOKE_NET_OK waiting shown, Cancel took {:?}", start.elapsed());
                    std::process::exit(0);
                }
            }
            _ => fail("nothing happened in time"),
        }
    }
}

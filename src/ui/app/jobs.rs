use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use relm4::gtk;
use relm4::gtk::prelude::*;
use relm4::prelude::*;
use relm4::RelmWidgetExt;

use rust_commander::config::{self};
use rust_commander::ops::{self, Choice, Kind, Report};
use rust_commander::state::PaneId;

use crate::ui::msg::Msg;

use super::{App, Ask, Job, JobOut, PendingUnpack, Phase, Queued, Work, home_dir};

impl App {
    pub(in crate::ui::app) fn spawn_transfer(&mut self, items: Vec<(PathBuf, PathBuf)>, kind: Kind, sender: &ComponentSender<Self>) {
        if items.is_empty() {
            self.status = "Nothing selected".into();
            return;
        }
        // One job at a time. A second job would take over the progress and conflict channels.
        let Some(pipes) = self.open_job(Work::Transfer) else { return };
        run_transfer(items, kind, Arc::new(std::sync::atomic::AtomicBool::new(false)), pipes);
        let _ = sender;
    }

    pub(in crate::ui::app) fn spawn_delete(&mut self, paths: Vec<PathBuf>, permanent: bool, sender: &ComponentSender<Self>) {
        if paths.is_empty() {
            self.status = "Nothing selected".into();
            return;
        }
        let Some(pipes) = self.open_job(Work::Delete) else { return };
        let JobOut { cancel, progress_tx, done_tx, .. } = pipes;
        spawn_worker(move || {
            let total = paths.len() as u64;
            for (done, path) in paths.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    let _ = done_tx.send(Ok(Report { cancelled: true, files: done as u64 }));
                    return;
                }
                let _ = progress_tx.send(ops::Progress { done: done as u64, total, current: path.clone(), note: String::new() });
                let result = ops::delete_paths(std::slice::from_ref(path), permanent, &mut |path| gio_trash(path));
                if let Err(err) = result {
                    let _ = done_tx.send(Err(format!("{}: {err}", path.display())));
                    return;
                }
            }
            let _ = done_tx.send(Ok(Report { cancelled: false, files: total }));
        });
        let _ = sender;
    }

    pub(in crate::ui::app) fn poll_job(&mut self, sender: &ComponentSender<Self>) {
        let polled = {
            let Some(job) = self.job.as_mut() else { return };
            let mut line = None;
            let mut broken: Option<(u64, String)> = None;
            let mut show = false;
            let mut fraction: Option<Option<f64>> = None;
            let mut amount = None;
            // Only the newest progress is shown. Each one carries the totals so far.
            if let Some(progress) = job.progress_rx.try_iter().last() {
                if matches!(job.work, Work::Verify) {
                    let (status, issues) = split_check(&progress.note);
                    line = Some(if status.is_empty() {
                        format!("{}/{}  {}", progress.done, progress.total, progress.current.display())
                    } else {
                        status
                    });
                    if let Some(issues) = issues {
                        broken = Some(issues);
                    }
                } else {
                    line = Some(if progress.note.is_empty() {
                        format!("{}/{}  {}", progress.done, progress.total, progress.current.display())
                    } else {
                        progress.note.clone()
                    });
                }
                fraction = Some(if progress.total == 0 {
                    None
                } else {
                    Some(progress.done as f64 / progress.total as f64)
                });
                if matches!(job.work, Work::Unpack | Work::Test) && progress.total > 0 {
                    amount = Some(format!(
                        "{:.0}%   {} / {}",
                        (progress.done as f64 / progress.total as f64 * 100.0).min(100.0),
                        rust_commander::listing::format_size(progress.done),
                        rust_commander::listing::format_size(progress.total)
                    ));
                } else if matches!(job.work, Work::Verify) && progress.total > 0 {
                    amount = Some(format!("{} / {}", progress.done, progress.total));
                }
                show = job.started.elapsed() > Duration::from_millis(200);
            }
            let conflict = if job.conflict_open { None } else { job.conflict_rx.try_recv().ok() };
            if conflict.is_some() {
                job.conflict_open = true;
            }
            let done = match job.done_rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => Some(Err("stopped".into())),
                Err(mpsc::TryRecvError::Empty) => None,
            };
            // A fast test only leaves the result. A longer one keeps this window and fills the bar.
            if matches!(job.work, Work::Test) {
                show = done.is_none() && job.started.elapsed() > Duration::from_millis(200);
            }
            (line, broken, show, fraction, amount, conflict, done)
        };
        let (line, broken, show, fraction, amount, conflict, done) = polled;
        if let Some(line) = &line {
            self.status = line.replace('\n', "   ");
        }
        if let Some(next) = fraction {
            self.meter = next;
        }
        if show {
            self.ensure_progress();
        }
        if let Some((_, label, bar, log)) = &self.progress {
            if let Some(line) = &line {
                label.set_text(line);
            }
            if let Some((count, text)) = &broken {
                if let Some((heading, view)) = log {
                    heading.set_text(&format!("Broken {count}"));
                    let buffer = view.buffer();
                    let now = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
                    if now.as_str() != text {
                        buffer.set_text(text);
                        if !text.is_empty() {
                            let mut end = buffer.end_iter();
                            view.scroll_to_iter(&mut end, 0.0, true, 0.0, 1.0);
                        }
                    }
                }
            }
            if let Some(amount) = &amount {
                bar.set_text(Some(amount));
                bar.set_show_text(true);
            }
            match self.meter {
                Some(value) => bar.set_fraction(value.clamp(0.0, 1.0)),
                None => bar.pulse(),
            }
        }
        if let Some((src, dst)) = conflict {
            self.ask_conflict(&src, &dst, sender.input_sender().clone());
        }
        if let Some(result) = done {
            self.finish_job(result, sender);
        }
    }

    pub(in crate::ui::app) fn finish_job(&mut self, result: Result<Report, String>, sender: &ComponentSender<Self>) {
        let work = self.job.as_ref().map(|job| job.work).unwrap_or(Work::Transfer);
        self.job = None;
        let retry_password = matches!(&result, Err(err) if err == "Password required" || err == "Wrong password")
            && self.pending.as_ref().is_some_and(|pending| pending.asked < 2);
        let cancelled = match &result {
            Ok(report) => report.cancelled,
            Err(err) => err.eq_ignore_ascii_case("cancelled"),
        };
        let show_test = matches!(work, Work::Test) && !cancelled && !retry_password;
        let mut test_dialog = false;
        if let Some((win, label, bar, log)) = self.progress.take() {
            if matches!(work, Work::Verify) && win.is_visible() {
                self.settle_sfv(&win, &label, &bar, log.as_ref(), &result);
                self.close_dialog();
                self.dialog = Some(win);
            } else if show_test && win.is_visible() {
                self.settle_test(&win, &label, &bar, &result);
                self.close_dialog();
                self.dialog = Some(win);
                test_dialog = true;
            } else {
                win.close();
            }
        }
        if show_test && !test_dialog {
            self.show_test_result(&result);
            test_dialog = true;
        }
        if let Err(err) = &result {
            if err.eq_ignore_ascii_case("cancelled") {
                self.pending = None;
                self.status = "Cancelled".into();
                self.refresh_both(sender);
                return;
            }
            if (err == "Password required" || err == "Wrong password") && self.offer_password(sender) {
                return;
            }
            self.pending = None;
            if !test_dialog && !matches!(work, Work::Verify | Work::Transfer | Work::Delete) {
                let title = match work {
                    Work::Unpack if err.starts_with("Archive is damaged") => "Damaged archive",
                    Work::Unpack if err.starts_with("Skipped ") => "Unpack",
                    Work::Unpack => "Unpack failed",
                    Work::Pack => "Pack failed",
                    Work::Sfv | Work::Sum => "Checksum failed",
                    Work::Shred => "Secure delete failed",
                    Work::Test => "Test failed",
                    Work::Encode => "Encode failed",
                    Work::Verify | Work::Transfer | Work::Delete => "Failed",
                };
                self.show_error(title, err);
            }
            rust_commander::log::error(&format!("job {err}"));
            self.status = first_line(err);
            self.refresh_both(sender);
            return;
        }
        self.pending = None;
        self.status = match result {
            Ok(report) if report.cancelled => "Cancelled".into(),
            Ok(_) if matches!(work, Work::Verify) => "Checksum OK".into(),
            Ok(_) if matches!(work, Work::Test) => "Archive OK".into(),
            Ok(_) => "Done".into(),
            Err(err) => err,
        };
        self.refresh_both(sender);
    }

    pub(in crate::ui::app) fn refresh_both(&mut self, sender: &ComponentSender<Self>) {
        for id in [PaneId::Left, PaneId::Right] {
            if self.state.tab(id).feed {
                self.state.refresh_hits(id);
                self.touch(id);
                continue;
            }
            let (path, token) = self.state.refresh(id);
            let tab = self.state.pane(id).active;
            self.spawn_load(sender, id, tab, path, token);
        }
    }

    pub(in crate::ui::app) fn open_job(&mut self, work: Work) -> Option<JobOut> {
        if self.job.is_some() {
            self.status = "Busy".into();
            return None;
        }
        let (job, out) = new_job(work);
        self.meter = None;
        self.job = Some(job);
        Some(out)
    }

    pub(in crate::ui::app) fn begin_unpack(&mut self, pick: bool, sender: &ComponentSender<Self>) {
        let Some(path) = self.file_under_cursor() else {
            self.status = "Not an archive".into();
            return;
        };
        if !rust_commander::archive::is_archive(&path) {
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

    pub(in crate::ui::app) fn spawn_archive_list(&mut self, sender: &ComponentSender<Self>) {
        let Some(pending) = &self.pending else { return };
        let path = pending.archive.clone();
        let password = pending.password.clone();
        self.status = "Reading archive".into();
        self.archive_wait = true;
        self.paint_wait();
        let tx = sender.input_sender().clone();
        std::thread::spawn(move || {
            let result = rust_commander::archive::list(&path, password.as_deref()).map_err(|err| err.to_string());
            tx.emit(Msg::ArchiveListed(result));
        });
    }

    pub(in crate::ui::app) fn on_archive_listed(&mut self, result: Result<rust_commander::archive::Listed, String>, sender: &ComponentSender<Self>) {
        self.archive_wait = false;
        self.paint_wait();
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

    pub(in crate::ui::app) fn on_unpack_pick(&mut self, dest: String, names: Vec<String>, sender: &ComponentSender<Self>) {
        self.close_dialog();
        if names.is_empty() {
            self.pending = None;
            self.status = "Nothing selected".into();
            return;
        }
        let dest = self.target_dir(&dest);
        if let Some(pending) = &mut self.pending {
            pending.dest = dest;
            pending.names = Some(names);
            pending.phase = Phase::Extract;
        }
        self.spawn_extract(sender);
    }

    pub(in crate::ui::app) fn spawn_extract(&mut self, sender: &ComponentSender<Self>) {
        let Some(pending) = &self.pending else { return };
        let archive = pending.archive.clone();
        let dest = pending.dest.clone();
        let names = pending.names.clone();
        let password = pending.password.clone();
        let Some(pipes) = self.open_job(Work::Unpack) else { return };
        self.status = "Unpacking".into();
        rust_commander::log::info(&format!("unpack {}", archive.display()));
        self.ensure_progress();
        let _ = sender;
        std::thread::spawn(move || {
            let set = names.map(|names| names.into_iter().collect::<std::collections::HashSet<_>>());
            let result = rust_commander::archive::extract(
                &archive,
                &dest,
                set.as_ref(),
                password.as_deref(),
                &pipes.cancel,
                &mut |progress| {
                    let mut progress = progress.clone();
                    if let Some((volume, file)) = progress.note.split_once('\n') {
                        progress.note = format!("From: {volume}\nTo:   {}\n{file}", dest.display());
                    }
                    let _ = pipes.progress_tx.send(progress);
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

    pub(in crate::ui::app) fn offer_password(&mut self, sender: &ComponentSender<Self>) -> bool {
        let Some(pending) = &mut self.pending else { return false };
        if pending.asked >= 2 {
            return false;
        }
        pending.asked += 1;
        self.ask = Some(Ask::Password);
        self.ask_text("Password", "", sender.input_sender().clone());
        true
    }

    pub(in crate::ui::app) fn retry_password(&mut self, text: String, sender: &ComponentSender<Self>) {
        let phase = {
            let Some(pending) = &mut self.pending else { return };
            pending.password = Some(text);
            match pending.phase {
                Phase::List => Phase::List,
                Phase::Extract => Phase::Extract,
                Phase::Test => Phase::Test,
            }
        };
        match phase {
            Phase::List => self.spawn_archive_list(sender),
            Phase::Extract => self.spawn_extract(sender),
            Phase::Test => self.spawn_test(sender),
        }
    }

    pub(in crate::ui::app) fn fail_unpack(&mut self, err: &str) {
        self.pending = None;
        self.show_error("Unpack failed", err);
        self.status = first_line(err);
    }

    pub(in crate::ui::app) fn ask_unpack(&mut self, members: &[rust_commander::archive::Member], dest: &str, tx: relm4::Sender<Msg>) {
        self.close_dialog();
        let win = gtk::Window::new();
        win.set_title(Some("Unpack"));
        win.set_modal(true);
        win.set_transient_for(Some(&self.shell.window));
        win.set_default_width(480);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
        column.set_margin_all(12);
        let heading = |text: &str| {
            let label = gtk::Label::new(Some(text));
            label.set_xalign(0.0);
            label.add_css_class("heading");
            label
        };
        let entry = gtk::Entry::new();
        entry.set_text(dest);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_propagate_natural_height(true);
        scroll.set_max_content_height(200);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        for member in members {
            let shown = if member.dir { format!("{}/", member.name) } else { member.name.clone() };
            let label = gtk::Label::new(Some(&shown));
            label.set_xalign(0.0);
            label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            label.set_margin_start(4);
            list.append(&label);
        }
        scroll.set_child(Some(&list));
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.set_margin_top(6);
        let ok = gtk::Button::with_label("Unpack");
        let entry_ok = entry.clone();
        let tx_ok = tx.clone();
        let names: Vec<String> = members.iter().map(|member| member.name.clone()).collect();
        crate::ui::shell::on_click(&ok, move || {
            tx_ok.emit(Msg::UnpackPick(entry_ok.text().to_string(), names.clone()));
        });
        let ok_entry = ok.clone();
        entry.connect_activate(move |_| ok_entry.emit_clicked());
        let cancel = gtk::Button::with_label("Cancel");
        let tx_esc = tx.clone();
        crate::ui::shell::on_escape(&win, move || tx_esc.emit(Msg::CancelAsk));
        crate::ui::shell::on_click(&cancel, move || tx.emit(Msg::CancelAsk));
        row.append(&ok);
        row.append(&cancel);
        column.append(&heading("Target location"));
        column.append(&entry);
        column.append(&heading(&format!("Contents ({})", members.len())));
        column.append(&scroll);
        column.append(&row);
        win.set_child(Some(&column));
        win.set_default_widget(Some(&ok));
        win.present();
        ok.grab_focus();
        self.dialog = Some(win);
    }

    pub(in crate::ui::app) fn start_pack(&mut self, here: bool, sender: &ComponentSender<Self>) {
        let sources = self.state.selected_paths(self.state.active);
        if sources.is_empty() {
            self.status = "Nothing selected".into();
            return;
        }
        if self.job.is_some() {
            self.status = "Busy".into();
            return;
        }
        let dest = self.pack_suggestion(here);
        self.ask_pack(&sources, &dest, sender.input_sender().clone());
    }

    pub(in crate::ui::app) fn ask_pack(&mut self, sources: &[PathBuf], dest: &str, tx: relm4::Sender<Msg>) {
        use rust_commander::pack::{self, Format, Options};
        self.close_dialog();
        let win = gtk::Window::new();
        win.set_title(Some("Pack"));
        win.set_modal(true);
        win.set_transient_for(Some(&self.shell.window));
        win.set_default_width(480);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
        column.set_margin_all(12);
        let heading = |text: &str| {
            let label = gtk::Label::new(Some(text));
            label.set_xalign(0.0);
            label.add_css_class("heading");
            label
        };
        let entry = gtk::Entry::new();
        entry.set_text(dest);
        const FORMATS: [Format; 3] = [Format::Zip, Format::SevenZ, Format::Rar];
        let format = gtk::DropDown::from_strings(&["ZIP", "7z", "RAR"]);
        let start = pack::format_of(Path::new(dest)).and_then(|have| FORMATS.iter().position(|f| *f == have)).unwrap_or(0);
        format.set_selected(start as u32);
        let entry_format = entry.clone();
        format.connect_selected_notify(move |drop| {
            let Some(&chosen) = FORMATS.get(drop.selected() as usize) else { return };
            let path = PathBuf::from(entry_format.text().as_str());
            entry_format.set_text(&path.with_extension(pack::extension(chosen)).display().to_string());
        });
        let level_names: Vec<&str> = pack::LEVELS.iter().map(|(name, _)| *name).collect();
        let level = gtk::DropDown::from_strings(&level_names);
        level.set_selected(pack::DEFAULT_LEVEL as u32);
        let options = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        options.append(&gtk::Label::new(Some("Format")));
        options.append(&format);
        options.append(&gtk::Label::new(Some("Compression")));
        options.append(&level);
        let volume = gtk::Entry::new();
        volume.set_placeholder_text(Some("Volume MB, empty = one file"));
        volume.set_sensitive(start != 0);
        let volume_format = volume.clone();
        format.connect_selected_notify(move |drop| {
            let zip = drop.selected() == 0;
            volume_format.set_sensitive(!zip);
            if zip {
                volume_format.set_text("");
            }
        });
        let password = gtk::PasswordEntry::new();
        password.set_show_peek_icon(true);
        password.set_property("placeholder-text", "Password (empty = no encryption)");
        let repeat = gtk::PasswordEntry::new();
        repeat.set_show_peek_icon(true);
        repeat.set_property("placeholder-text", "Repeat password");
        let note = gtk::Label::new(Some("ZIP uses AES-256. 7z and RAR also hide the file names."));
        note.set_xalign(0.0);
        note.add_css_class("dim-label");
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_propagate_natural_height(true);
        scroll.set_max_content_height(200);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        for source in sources {
            let name = source.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            let label = gtk::Label::new(Some(&name));
            label.set_xalign(0.0);
            label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            label.set_margin_start(4);
            list.append(&label);
        }
        scroll.set_child(Some(&list));
        let error = gtk::Label::new(None);
        error.set_xalign(0.0);
        error.add_css_class("error");
        error.set_visible(false);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.set_margin_top(6);
        let ok = gtk::Button::with_label("Pack");
        let (entry_ok, level_ok, password_ok, repeat_ok, volume_ok, error_ok, tx_ok) =
            (entry.clone(), level.clone(), password.clone(), repeat.clone(), volume.clone(), error.clone(), tx.clone());
        crate::ui::shell::on_click(&ok, move || {
            let dest = entry_ok.text().trim().to_string();
            let secret = password_ok.text().to_string();
            let volume_text = volume_ok.text().trim().to_string();
            let volume = if volume_text.is_empty() {
                None
            } else {
                match volume_text.parse::<u64>() {
                    Ok(mb) if mb > 0 => Some(mb.saturating_mul(1024 * 1024)),
                    _ => {
                        error_ok.set_text("Volume size is a number of MB");
                        error_ok.set_visible(true);
                        return;
                    }
                }
            };
            let problem = if pack::format_of(Path::new(&dest)).is_none() {
                Some("Use a .zip, .7z or .rar name")
            } else if volume.is_some() && pack::format_of(Path::new(&dest)) == Some(Format::Zip) {
                Some("Zip is one file. Volume size needs 7z or rar.")
            } else if secret != repeat_ok.text().as_str() {
                Some("Passwords do not match")
            } else {
                None
            };
            if let Some(problem) = problem {
                error_ok.set_text(problem);
                error_ok.set_visible(true);
                return;
            }
            let level = pack::LEVELS.get(level_ok.selected() as usize).map(|(_, n)| *n).unwrap_or(5);
            tx_ok.emit(Msg::PackPick(dest, Options { level, password: (!secret.is_empty()).then_some(secret), volume }));
        });
        let ok_entry = ok.clone();
        entry.connect_activate(move |_| ok_entry.emit_clicked());
        let ok_password = ok.clone();
        password.connect_activate(move |_| ok_password.emit_clicked());
        let ok_repeat = ok.clone();
        repeat.connect_activate(move |_| ok_repeat.emit_clicked());
        let cancel = gtk::Button::with_label("Cancel");
        let tx_esc = tx.clone();
        crate::ui::shell::on_escape(&win, move || tx_esc.emit(Msg::CancelAsk));
        crate::ui::shell::on_click(&cancel, move || tx.emit(Msg::CancelAsk));
        row.append(&ok);
        row.append(&cancel);
        column.append(&heading("Target location"));
        column.append(&entry);
        column.append(&options);
        column.append(&volume);
        column.append(&heading("Encryption"));
        column.append(&password);
        column.append(&repeat);
        column.append(&note);
        column.append(&heading(&format!("Contents ({})", sources.len())));
        column.append(&scroll);
        column.append(&error);
        column.append(&row);
        win.set_child(Some(&column));
        win.set_default_widget(Some(&ok));
        win.present();
        ok.grab_focus();
        self.dialog = Some(win);
    }

    pub(in crate::ui::app) fn pack_suggestion(&self, here: bool) -> String {
        let sources = self.state.selected_paths(self.state.active);
        let stem = if sources.len() == 1 {
            stem_of(&sources[0]).unwrap_or("archive")
        } else {
            self.state.tab(self.state.active).path.file_name().and_then(|name| name.to_str()).unwrap_or("archive")
        };
        let pane = if here { self.state.active } else { self.state.active.other() };
        self.state.tab(pane).path.join(format!("{stem}.zip")).display().to_string()
    }

    pub(in crate::ui::app) fn confirm_pack(&mut self, dest: PathBuf, opts: rust_commander::pack::Options, sender: &ComponentSender<Self>) {
        if rust_commander::pack::format_of(&dest).is_none() {
            self.status = "Use a .zip, .7z or .rar name".into();
            return;
        }
        if dest.exists() {
            self.queued = Some(Queued::Pack(dest, opts));
            self.ask = Some(Ask::Overwrite);
            self.ask_confirm("Overwrite archive?", sender.input_sender().clone());
            return;
        }
        self.spawn_pack(dest, opts, sender);
    }

    pub(in crate::ui::app) fn spawn_pack(&mut self, dest: PathBuf, opts: rust_commander::pack::Options, sender: &ComponentSender<Self>) {
        let Some(format) = rust_commander::pack::format_of(&dest) else {
            self.status = "Use a .zip, .7z or .rar name".into();
            return;
        };
        let bin = match format {
            rust_commander::pack::Format::Zip => None,
            other => {
                let configured = match other {
                    rust_commander::pack::Format::SevenZ => self.cfg.seven_zip.as_str(),
                    rust_commander::pack::Format::Rar => self.cfg.rar.as_str(),
                    rust_commander::pack::Format::Zip => "",
                };
                match rust_commander::pack::find_program(configured, rust_commander::pack::names(other)) {
                    Some(path) => Some(path),
                    None => {
                        self.show_error("Pack failed", rust_commander::pack::hint(other));
                        self.status = first_line(rust_commander::pack::hint(other));
                        return;
                    }
                }
            }
        };
        let sources = self.state.selected_paths(self.state.active);
        let cwd = self.state.tab(self.state.active).path.clone();
        let Some(pipes) = self.open_job(Work::Pack) else { return };
        self.status = "Compressing...".into();
        rust_commander::log::info(&format!("pack {}", dest.display()));
        self.ensure_progress();
        let _ = sender;
        spawn_worker(move || {
            let result = rust_commander::pack::pack(format, bin.as_deref(), &sources, &cwd, &dest, &opts, &pipes.cancel, &mut |progress| {
                let _ = pipes.progress_tx.send(progress.clone());
            });
            let _ = pipes.done_tx.send(result.map_err(|err| err.to_string()));
        });
    }

    pub(in crate::ui::app) fn start_sfv(&mut self, sender: &ComponentSender<Self>) {
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

    pub(in crate::ui::app) fn sfv_dest(&self, sources: &[PathBuf]) -> PathBuf {
        let stem = if sources.len() == 1 {
            stem_of(&sources[0]).unwrap_or("checksum")
        } else {
            self.state.tab(self.state.active).path.file_name().and_then(|name| name.to_str()).unwrap_or("checksum")
        };
        self.state.tab(self.state.active).path.join(format!("{stem}.sfv"))
    }

    pub(in crate::ui::app) fn spawn_sfv_create(&mut self, dest: PathBuf, sender: &ComponentSender<Self>) {
        let root = self.state.tab(self.state.active).path.clone();
        let sources = self.state.selected_paths(self.state.active);
        let Some(pipes) = self.open_job(Work::Sfv) else { return };
        let _ = sender;
        spawn_worker(move || {
            let result = rust_commander::sfv::create(&root, &sources, &dest, &pipes.cancel, &mut |progress| {
                let _ = pipes.progress_tx.send(progress.clone());
            });
            let _ = pipes.done_tx.send(result.map_err(|err| err.to_string()));
        });
    }

    pub(in crate::ui::app) fn start_verify(&mut self, sender: &ComponentSender<Self>) {
        let path = if let Some(path) = self.file_under_cursor().filter(|path| is_check_file(path)) {
            path
        } else {
            match self.sfv_to_check() {
                Ok(path) => path,
                Err(status) => {
                    self.status = status.into();
                    return;
                }
            }
        };
        let summed = rust_commander::sum::kind_of(&path).is_some();
        let Some(pipes) = self.open_job(Work::Verify) else { return };
        self.status = "Checking checksum".into();
        rust_commander::log::info(&format!("checksum {}", path.display()));
        self.ensure_progress();
        let _ = sender;
        std::thread::spawn(move || {
            let result = if summed {
                rust_commander::sum::verify(&path, &pipes.cancel, &mut |progress| {
                    let _ = pipes.progress_tx.send(progress.clone());
                })
            } else {
                rust_commander::sfv::verify(&path, &pipes.cancel, &mut |progress| {
                    let _ = pipes.progress_tx.send(progress.clone());
                })
            };
            let _ = pipes.done_tx.send(result.map_err(|err| err.to_string()));
        });
    }

    pub(in crate::ui::app) fn spawn_sum(&mut self, dest: PathBuf, kind: rust_commander::sum::Kind, sender: &ComponentSender<Self>) {
        let root = self.state.tab(self.state.active).path.clone();
        let sources = self.state.selected_paths(self.state.active);
        let Some(pipes) = self.open_job(Work::Sum) else { return };
        let _ = sender;
        spawn_worker(move || {
            let result = rust_commander::sum::create(kind, &root, &sources, &dest, &pipes.cancel, &mut |progress| {
                let _ = pipes.progress_tx.send(progress.clone());
            });
            let _ = pipes.done_tx.send(result.map_err(|err| err.to_string()));
        });
    }

    pub(in crate::ui::app) fn spawn_shred(&mut self, paths: Vec<PathBuf>, sender: &ComponentSender<Self>) {
        if paths.is_empty() {
            return;
        }
        let Some(pipes) = self.open_job(Work::Shred) else { return };
        self.status = "Overwriting...".into();
        self.ensure_progress();
        let _ = sender;
        spawn_worker(move || {
            let total = paths.len() as u64;
            for (done, path) in paths.iter().enumerate() {
                if pipes.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = pipes.done_tx.send(Ok(rust_commander::ops::Report { cancelled: true, files: done as u64 }));
                    return;
                }
                let _ = pipes.progress_tx.send(rust_commander::ops::Progress {
                    done: done as u64,
                    total,
                    current: path.clone(),
                    note: String::new(),
                });
                match rust_commander::ops::shred(path, &pipes.cancel) {
                    Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {
                        let _ = pipes.done_tx.send(Ok(rust_commander::ops::Report { cancelled: true, files: done as u64 }));
                        return;
                    }
                    Err(err) => {
                        let _ = pipes.done_tx.send(Err(format!("{}: {err}", path.display())));
                        return;
                    }
                    Ok(()) => {}
                }
            }
            let _ = pipes.done_tx.send(Ok(rust_commander::ops::Report { cancelled: false, files: total }));
        });
    }

    pub(in crate::ui::app) fn spawn_test(&mut self, sender: &ComponentSender<Self>) {
        let Some(pending) = self.pending.clone() else { return };
        let seven = rust_commander::pack::find_program(&self.cfg.seven_zip, rust_commander::pack::names(rust_commander::pack::Format::SevenZ));
        let Some(pipes) = self.open_job(Work::Test) else {
            self.pending = None;
            return;
        };
        self.status = "Testing archive".into();
        let _ = sender;
        spawn_worker(move || {
            let result = rust_commander::archive::test(
                &pending.archive,
                pending.password.as_deref(),
                seven.as_deref(),
                &pipes.cancel,
                &mut |progress| {
                    let _ = pipes.progress_tx.send(progress.clone());
                },
            );
            let _ = pipes.done_tx.send(result.map_err(|err| err.to_string()));
        });
    }

    pub(in crate::ui::app) fn spawn_encode(
        &mut self,
        codec: rust_commander::encode::Codec,
        source: PathBuf,
        dest: PathBuf,
        sender: &ComponentSender<Self>,
    ) {
        let Some(pipes) = self.open_job(Work::Encode) else { return };
        let _ = sender;
        spawn_worker(move || {
            let result = rust_commander::encode::encode(codec, &source, &dest, &pipes.cancel, &mut |progress| {
                let _ = pipes.progress_tx.send(progress.clone());
            });
            let _ = pipes.done_tx.send(result.map_err(|err| err.to_string()));
        });
    }

    pub(in crate::ui::app) fn spawn_decode(&mut self, source: PathBuf, dest_dir: PathBuf, sender: &ComponentSender<Self>) {
        let Some(pipes) = self.open_job(Work::Encode) else { return };
        let _ = sender;
        spawn_worker(move || {
            let result = rust_commander::encode::decode(&source, &dest_dir, &pipes.cancel, &mut |progress| {
                let _ = pipes.progress_tx.send(progress.clone());
            });
            let _ = pipes.done_tx.send(result.map(|(report, _)| report).map_err(|err| err.to_string()));
        });
    }

    fn sfv_to_check(&self) -> Result<PathBuf, &'static str> {
        let tab = self.state.tab(self.state.active);
        let mut found = None;
        for entry in &tab.entries {
            if entry.is_dir() || !is_check_file(Path::new(entry.name.as_ref())) {
                continue;
            }
            if found.is_some() {
                return Err("Select a checksum file");
            }
            found = Some(rust_commander::listing::entry_path(&tab.path, entry));
        }
        found.ok_or("No checksum file")
    }

    fn settle_sfv(
        &self,
        win: &gtk::Window,
        label: &gtk::Label,
        bar: &gtk::ProgressBar,
        log: Option<&(gtk::Label, gtk::TextView)>,
        result: &Result<Report, String>,
    ) {
        win.set_title(Some("Checksum"));
        label.set_ellipsize(gtk::pango::EllipsizeMode::None);
        label.set_lines(-1);
        let cancelled = match result {
            Ok(report) => report.cancelled,
            Err(err) => err.eq_ignore_ascii_case("cancelled"),
        };
        if !cancelled {
            bar.set_fraction(1.0);
            bar.set_show_text(false);
        }
        match result {
            Ok(report) if report.cancelled => label.set_text("Cancelled"),
            Ok(report) => {
                label.set_markup(&format!(
                    "<span foreground=\"{}\" size=\"x-large\">✔</span>  All files OK, {} files",
                    rust_commander::theme::current().green,
                    report.files
                ));
                if let Some((heading, _)) = log {
                    heading.set_text("Broken 0");
                }
            }
            Err(err) if err.eq_ignore_ascii_case("cancelled") => label.set_text("Cancelled"),
            Err(err) => {
                let (head, body) = err.split_once('\n').unwrap_or((err.as_str(), ""));
                label.set_text(head);
                if let Some((heading, view)) = log {
                    heading.set_text(&format!("Broken {}", broken_total(head)));
                    view.buffer().set_text(body);
                }
            }
        }
        if let Some(button) = window_button(win) {
            button.set_label("Close");
            let weak = win.downgrade();
            button.connect_clicked(move |_| {
                if let Some(win) = weak.upgrade() {
                    win.close();
                }
            });
        }
    }

    fn settle_test(&self, win: &gtk::Window, label: &gtk::Label, bar: &gtk::ProgressBar, result: &Result<Report, String>) {
        win.set_title(Some(test_title(result)));
        apply_test_label(label, result);
        if result.is_ok() {
            bar.set_fraction(1.0);
            bar.set_show_text(false);
        }
        place_ok(win);
    }

    pub(in crate::ui::app) fn show_test_result(&mut self, result: &Result<Report, String>) {
        self.close_dialog();
        let win = gtk::Window::new();
        win.set_title(Some(test_title(result)));
        win.set_modal(true);
        win.set_transient_for(Some(&self.shell.window));
        win.set_default_width(520);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        column.set_margin_all(12);
        let label = gtk::Label::new(None);
        apply_test_label(&label, result);
        let ok = gtk::Button::with_label("OK");
        let win_ok = win.clone();
        let win_esc = win.clone();
        crate::ui::shell::on_escape(&win, move || win_esc.close());
        crate::ui::shell::on_click(&ok, move || win_ok.close());
        column.append(&label);
        column.append(&ok);
        win.set_child(Some(&column));
        win.present();
        focus_ok(&win, &ok);
        self.dialog = Some(win);
    }

    pub(in crate::ui::app) fn confirm_overwrite(&mut self, sender: &ComponentSender<Self>) {
        match self.queued.take() {
            Some(Queued::Pack(dest, opts)) => self.spawn_pack(dest, opts, sender),
            Some(Queued::Sfv(dest)) => self.spawn_sfv_create(dest, sender),
            Some(Queued::Sum(dest, kind)) => self.spawn_sum(dest, kind, sender),
            None => {}
        }
    }

    pub(in crate::ui::app) fn set_tool(&mut self, seven: bool, path: String) {
        if seven {
            self.cfg.seven_zip = path;
        } else {
            self.cfg.rar = path;
        }
        let _ = config::save(&self.cfg);
    }

    pub(in crate::ui::app) fn show_error(&mut self, title: &str, text: &str) {
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
        let win_esc = win.clone();
        crate::ui::shell::on_escape(&win, move || win_esc.close());
        crate::ui::shell::on_click(&ok, move || win_ok.close());
        column.append(&label);
        column.append(&ok);
        win.set_child(Some(&column));
        win.present();
        self.dialog = Some(win);
    }

    pub(in crate::ui::app) fn sync_hints(&self) {
        let cue = self.cue();
        let side = if self.state.active == PaneId::Left { "right" } else { "left" };
        let ranked = rust_commander::commands::rank(&cue);
        for (slot, hint) in self.shell.hints.iter().enumerate() {
            if let Some(act) = ranked.get(slot).copied() {
                let label = rust_commander::commands::hint_label(act, side);
                if hint.button.label().as_deref() != Some(label.as_str()) {
                    hint.button.set_label(&label);
                }
                if hint.act.get() != Some(act) {
                    hint.act.set(Some(act));
                }
                hint.button.set_visible(true);
            } else {
                hint.act.set(None);
                hint.button.set_visible(false);
            }
        }
    }

    pub(in crate::ui::app) fn ask_text(&mut self, title: &str, initial: &str, tx: relm4::Sender<Msg>) {
        self.close_dialog();
        let win = gtk::Window::new();
        win.set_title(Some(title));
        win.set_modal(true);
        win.set_transient_for(Some(&self.shell.window));
        win.set_default_width(520);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        column.set_margin_all(12);
        let heading = gtk::Label::new(Some(title));
        heading.set_xalign(0.0);
        heading.set_wrap(true);
        heading.set_max_width_chars(60);
        heading.add_css_class("dialog-title");
        column.append(&heading);
        let entry = gtk::Entry::new();
        entry.set_text(initial);
        if title == "Password" {
            entry.set_visibility(false);
        }
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let ok = gtk::Button::with_label("OK");
        let ok_entry = ok.clone();
        entry.connect_activate(move |_| ok_entry.emit_clicked());
        let entry_ok = entry.clone();
        let tx_ok = tx.clone();
        crate::ui::shell::on_click(&ok, move || tx_ok.emit(Msg::Submit(entry_ok.text().to_string())));
        row.append(&ok);
        if matches!(self.ask, Some(Ask::Copy(_) | Ask::Move(_))) {
            let queue = gtk::Button::with_label("F2 Queue");
            let entry_queue = entry.clone();
            let tx_queue = tx.clone();
            crate::ui::shell::on_click(&queue, move || tx_queue.emit(Msg::Enqueue(entry_queue.text().to_string())));
            let keys = gtk::EventControllerKey::new();
            keys.set_propagation_phase(gtk::PropagationPhase::Capture);
            let queue_key = queue.clone();
            keys.connect_key_pressed(move |_, key, _, _| {
                if key != gtk::gdk::Key::F2 {
                    return gtk::glib::Propagation::Proceed;
                }
                queue_key.emit_clicked();
                gtk::glib::Propagation::Stop
            });
            win.add_controller(keys);
            row.append(&queue);
        }
        let cancel = gtk::Button::with_label("Cancel");
        let tx_esc = tx.clone();
        crate::ui::shell::on_escape(&win, move || tx_esc.emit(Msg::CancelAsk));
        crate::ui::shell::on_click(&cancel, move || tx.emit(Msg::CancelAsk));
        row.append(&cancel);
        column.append(&entry);
        column.append(&row);
        win.set_child(Some(&column));
        win.present();
        entry.grab_focus();
        self.dialog = Some(win);
    }

    pub(in crate::ui::app) fn ask_confirm(&mut self, title: &str, tx: relm4::Sender<Msg>) {
        self.close_dialog();
        let win = gtk::Window::new();
        win.set_title(Some(title));
        win.set_modal(true);
        win.set_transient_for(Some(&self.shell.window));
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let ok = gtk::Button::with_label("OK");
        let tx_ok = tx.clone();
        crate::ui::shell::on_click(&ok, move || tx_ok.emit(Msg::Submit(String::new())));
        let cancel = gtk::Button::with_label("Cancel");
        let tx_esc = tx.clone();
        crate::ui::shell::on_escape(&win, move || tx_esc.emit(Msg::CancelAsk));
        crate::ui::shell::on_click(&cancel, move || tx.emit(Msg::CancelAsk));
        row.append(&ok);
        row.append(&cancel);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        column.set_margin_all(12);
        let heading = gtk::Label::new(Some(title));
        heading.set_xalign(0.0);
        heading.set_wrap(true);
        heading.set_max_width_chars(60);
        heading.add_css_class("dialog-title");
        column.append(&heading);
        column.append(&row);
        win.set_child(Some(&column));
        win.present();
        self.dialog = Some(win);
    }

    pub(in crate::ui::app) fn ask_confirm_detail(&mut self, title: &str, detail: &str, tx: relm4::Sender<Msg>) {
        self.close_dialog();
        let win = gtk::Window::new();
        win.set_title(Some("Secure delete"));
        win.set_modal(true);
        win.set_transient_for(Some(&self.shell.window));
        win.set_default_width(480);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let ok = gtk::Button::with_label("Delete");
        let tx_ok = tx.clone();
        crate::ui::shell::on_click(&ok, move || tx_ok.emit(Msg::Submit(String::new())));
        let cancel = gtk::Button::with_label("Cancel");
        let tx_esc = tx.clone();
        crate::ui::shell::on_escape(&win, move || tx_esc.emit(Msg::CancelAsk));
        crate::ui::shell::on_click(&cancel, move || tx.emit(Msg::CancelAsk));
        row.append(&ok);
        row.append(&cancel);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        column.set_margin_all(12);
        let heading = gtk::Label::new(Some(title));
        heading.set_xalign(0.0);
        heading.set_wrap(true);
        heading.set_max_width_chars(60);
        heading.add_css_class("dialog-title");
        let body = gtk::Label::new(Some(detail));
        body.set_xalign(0.0);
        body.set_wrap(true);
        body.set_max_width_chars(60);
        column.append(&heading);
        column.append(&body);
        column.append(&row);
        win.set_child(Some(&column));
        win.present();
        self.dialog = Some(win);
    }

    pub(in crate::ui::app) fn ask_conflict(&mut self, src: &Path, dst: &Path, tx: relm4::Sender<Msg>) {
        self.close_dialog();
        let win = conflict_window(src, dst, &self.shell.window, move |choice| tx.emit(Msg::Conflict(choice)));
        win.present();
        self.dialog = Some(win);
    }

    pub(in crate::ui::app) fn ensure_progress(&mut self) {
        if self.progress.is_some() {
            return;
        }
        let win = gtk::Window::new();
        let testing = matches!(self.job.as_ref().map(|job| job.work), Some(Work::Test));
        let caption = if testing {
            "Testing archive"
        } else if self.status.is_empty() {
            "Working"
        } else {
            self.status.as_str()
        };
        win.set_title(Some(caption));
        win.set_modal(true);
        win.set_transient_for(Some(&self.shell.window));
        win.set_default_width(520);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        column.set_margin_all(12);
        let label = gtk::Label::new(Some(caption));
        label.set_wrap(true);
        label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        label.set_xalign(0.0);
        label.set_lines(4);
        label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        let bar = gtk::ProgressBar::new();
        bar.add_css_class("job");
        bar.set_show_text(false);
        let cancel = gtk::Button::with_label("Cancel");
        let flag = self.job.as_ref().map(|job| Arc::clone(&job.cancel));
        let close_flag = flag.clone();
        crate::ui::shell::on_click(&cancel, move || {
            if let Some(flag) = &flag {
                flag.store(true, Ordering::Relaxed);
            }
        });
        win.connect_close_request(move |_| {
            if let Some(flag) = &close_flag {
                flag.store(true, Ordering::Relaxed);
            }
            gtk::glib::Propagation::Proceed
        });
        let win_esc = win.clone();
        crate::ui::shell::on_escape(&win, move || win_esc.close());
        let broken = if matches!(self.job.as_ref().map(|job| job.work), Some(Work::Verify)) {
            let heading = gtk::Label::new(Some("Broken 0"));
            heading.set_xalign(0.0);
            let view = gtk::TextView::new();
            view.set_editable(false);
            view.set_cursor_visible(false);
            view.set_monospace(true);
            view.set_wrap_mode(gtk::WrapMode::WordChar);
            let scroll = gtk::ScrolledWindow::new();
            scroll.set_min_content_height(96);
            scroll.set_propagate_natural_height(true);
            scroll.set_max_content_height(220);
            scroll.set_child(Some(&view));
            Some((heading, scroll, view))
        } else {
            None
        };
        column.append(&label);
        column.append(&bar);
        let log = if let Some((heading, scroll, view)) = broken {
            column.append(&heading);
            column.append(&scroll);
            Some((heading, view))
        } else {
            None
        };
        column.append(&cancel);
        win.set_child(Some(&column));
        win.present();
        self.progress = Some((win, label, bar, log));
    }

    pub(in crate::ui::app) fn close_dialog(&mut self) {
        self.mount_ui = None;
        if let Some(win) = self.dialog.take() {
            win.close();
        }
    }
}

pub(in crate::ui::app) fn is_check_file(path: &Path) -> bool {
    rust_commander::sfv::is_sfv(path) || rust_commander::sum::kind_of(path).is_some()
}

// A folder keeps its whole name: release-1.2 is not cut to release-1.
pub(in crate::ui::app) fn stem_of(path: &Path) -> Option<&str> {
    let name = if path.is_dir() { path.file_name() } else { path.file_stem() };
    name.and_then(|name| name.to_str())
}

fn split_check(note: &str) -> (String, Option<(u64, String)>) {
    let Some((status, rest)) = note.split_once("\n\n") else {
        return (note.to_string(), None);
    };
    let mut lines = rest.lines();
    let count = lines.next().unwrap_or("0").parse().unwrap_or(0);
    (status.to_string(), Some((count, lines.collect::<Vec<_>>().join("\n"))))
}

fn broken_total(summary: &str) -> u64 {
    let mut count = 0u64;
    for part in summary.split(',') {
        let part = part.trim();
        let rest = part.strip_prefix("failed ").or_else(|| part.strip_prefix("missing "));
        if let Some(rest) = rest {
            if let Ok(value) = rest.parse::<u64>() {
                count += value;
            }
        }
    }
    count
}

fn test_title(result: &Result<Report, String>) -> &'static str {
    if result.is_ok() { "Archive OK" } else { "Test failed" }
}

fn apply_test_label(label: &gtk::Label, result: &Result<Report, String>) {
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_xalign(0.0);
    label.set_selectable(true);
    label.set_ellipsize(gtk::pango::EllipsizeMode::None);
    label.set_lines(-1);
    match result {
        Ok(report) => {
            let word = if report.files == 1 { "file" } else { "files" };
            label.set_markup(&format!(
                "<span foreground=\"{}\" size=\"x-large\">✔</span>  Archive OK, {} {word}",
                rust_commander::theme::current().green,
                report.files
            ));
        }
        Err(err) => label.set_text(err),
    }
}

fn place_ok(win: &gtk::Window) {
    let Some(cancel) = window_button(win) else { return };
    let ok = gtk::Button::with_label("OK");
    if let Some(row) = cancel.parent().and_then(|parent| parent.downcast::<gtk::Box>().ok()) {
        cancel.set_visible(false);
        row.append(&ok);
        let win_ok = win.clone();
        crate::ui::shell::on_click(&ok, move || win_ok.close());
        focus_ok(win, &ok);
        return;
    }
    cancel.set_label("OK");
    let win_ok = win.clone();
    cancel.connect_clicked(move |_| win_ok.close());
    focus_ok(win, &cancel);
}

fn focus_ok(win: &gtk::Window, button: &gtk::Button) {
    win.set_default_widget(Some(button));
    button.grab_focus();
    let button = button.clone();
    gtk::glib::idle_add_local_once(move || {
        button.grab_focus();
    });
}

fn window_button(win: &gtk::Window) -> Option<gtk::Button> {
    let mut child = win.child().and_then(|column| column.last_child());
    while let Some(widget) = child {
        if let Ok(button) = widget.clone().downcast::<gtk::Button>() {
            return Some(button);
        }
        child = widget.prev_sibling();
    }
    None
}

pub(in crate::ui::app) fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or(text).to_string()
}

pub(in crate::ui::app) fn new_job(work: Work) -> (Job, JobOut) {
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (progress_tx, progress_rx) = mpsc::channel();
    let (conflict_tx, conflict_rx) = mpsc::channel();
    let (answer_tx, answer_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let job = Job {
        work,
        cancel: Arc::clone(&cancel),
        progress_rx,
        conflict_rx,
        answer_tx,
        done_rx,
        started: Instant::now(),
        conflict_open: false,
    };
    (job, JobOut { cancel, progress_tx, conflict_tx, answer_rx, done_tx })
}

pub(in crate::ui::app) fn run_transfer(
    items: Vec<(PathBuf, PathBuf)>,
    kind: Kind,
    pause: Arc<std::sync::atomic::AtomicBool>,
    pipes: JobOut,
) {
    let JobOut { cancel, progress_tx, conflict_tx, answer_rx, done_tx } = pipes;
    spawn_worker(move || {
        let result = ops::transfer(
            &items,
            kind,
            &cancel,
            &pause,
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
}

// Copy, delete, pack and SFV recurse once per folder level, and PATH_MAX allows about 2000 levels.
fn spawn_worker(work: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new().stack_size(64 << 20).spawn(work).expect("cannot start a worker thread");
}

pub(in crate::ui::app) fn conflict_window(
    src: &Path,
    dst: &Path,
    parent: &gtk::Window,
    answer: impl Fn(Choice) + Clone + 'static,
) -> gtk::Window {
    let win = gtk::Window::new();
    win.set_title(Some("File exists"));
    win.set_modal(true);
    win.set_transient_for(Some(parent));
    let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
    column.set_margin_all(12);
    let label = gtk::Label::new(Some(&format!("{}\nalready exists as\n{}", src.display(), dst.display())));
    label.set_wrap(true);
    column.append(&label);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let answered = std::rc::Rc::new(std::cell::Cell::new(false));
    for (caption, choice) in [
        ("Overwrite", Choice::Overwrite),
        ("All", Choice::OverwriteAll),
        ("Skip", Choice::Skip),
        ("Skip all", Choice::SkipAll),
        ("Cancel", Choice::Cancel),
    ] {
        let button = gtk::Button::with_label(caption);
        let answer = answer.clone();
        let answered = answered.clone();
        crate::ui::shell::on_click(&button, move || {
            answered.set(true);
            answer(choice);
        });
        row.append(&button);
    }
    // Closing the window must answer too, or the worker waits forever and every new job is Busy.
    win.connect_close_request(move |_| {
        if !answered.get() {
            answered.set(true);
            answer(Choice::Cancel);
        }
        gtk::glib::Propagation::Proceed
    });
    let win_esc = win.clone();
    crate::ui::shell::on_escape(&win, move || win_esc.close());
    column.append(&row);
    win.set_child(Some(&column));
    win
}

pub(in crate::ui::app) fn items_into(sources: Vec<PathBuf>, dest_dir: &Path) -> Vec<(PathBuf, PathBuf)> {
    sources.into_iter().map(|src| {
        let name = src.file_name().unwrap_or_default().to_os_string();
        (src, dest_dir.join(name))
    }).collect()
}

pub(in crate::ui::app) fn gio_trash(path: &Path) -> std::io::Result<()> {
    use std::io::Write;
    let file = gtk::gio::File::for_path(path);
    if file.trash(gtk::gio::Cancellable::NONE).is_ok() {
        return Ok(());
    }
    let root = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".local/share"));
    let files = root.join("Trash/files");
    let info = root.join("Trash/info");
    std::fs::create_dir_all(&files)?;
    std::fs::create_dir_all(&info)?;
    let name = path.file_name().unwrap_or_default().to_os_string();
    let uri = gtk::gio::File::for_path(std::path::absolute(path)?).uri();
    let when = gtk::glib::DateTime::now_local()
        .ok()
        .and_then(|now| now.format("%Y-%m-%dT%H:%M:%S").ok())
        .map(|text| text.to_string())
        .unwrap_or_default();
    // Claim the .trashinfo first, so an earlier trashed file with the same name is never overwritten.
    for n in 1..10_000u32 {
        let mut pick = name.clone();
        if n > 1 {
            pick.push(format!(".{n}"));
        }
        let dest = files.join(&pick);
        let mut info_name = pick.clone();
        info_name.push(".trashinfo");
        let info_path = info.join(info_name);
        if std::fs::symlink_metadata(&dest).is_ok() {
            continue;
        }
        let mut note = match std::fs::OpenOptions::new().write(true).create_new(true).open(&info_path) {
            Ok(note) => note,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        };
        let moved = (|| {
            write!(note, "[Trash Info]\nPath={}\nDeletionDate={when}\n", uri.trim_start_matches("file://"))?;
            // Another disk stays where it is. Copying it home would fill this computer.
            std::fs::rename(path, &dest)
        })();
        if moved.is_err() {
            let _ = std::fs::remove_file(&info_path);
        }
        return moved;
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "no free name in the trash"))
}

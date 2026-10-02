use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

use relm4::gtk;
use relm4::gtk::prelude::*;
use relm4::prelude::*;
use relm4::RelmWidgetExt;

use rust_commander::ops::{Choice, Kind};

use crate::ui::msg::Msg;

use super::{jobs, App, Ask, Job, Work};

pub(in crate::ui::app) struct QueueUi {
    window: gtk::Window,
    list: gtk::ListBox,
    label: gtk::Label,
    bar: gtk::ProgressBar,
    pause_button: gtk::Button,
    tasks: VecDeque<Task>,
    running: Option<(String, Job)>,
    pause: Arc<AtomicBool>,
    conflict: Option<gtk::Window>,
    closed: gtk::glib::SignalHandlerId,
}

struct Task {
    title: String,
    items: Vec<(PathBuf, PathBuf)>,
    kind: Kind,
}

impl App {
    // Sources and target are fixed here, so the panels can move on while the job waits.
    pub(in crate::ui::app) fn enqueue(&mut self, text: &str, sender: &ComponentSender<Self>) {
        let (sources, kind) = match self.ask.take() {
            Some(Ask::Copy(sources)) => (sources, Kind::Copy),
            Some(Ask::Move(sources)) => (sources, Kind::Move),
            other => {
                self.ask = other;
                return;
            }
        };
        self.close_dialog();
        let dest = self.target_dir(text);
        let what = match sources.as_slice() {
            [one] => one.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default(),
            many => format!("{} items", many.len()),
        };
        let verb = if kind == Kind::Copy { "Copy" } else { "Move" };
        let title = format!("{verb} {what} to {}", dest.display());
        let items = jobs::items_into(sources, &dest);
        let tx = sender.input_sender().clone();
        let queue = self.queue.get_or_insert_with(|| queue_window(&self.shell.window, tx));
        queue.tasks.push_back(Task { title: title.clone(), items, kind });
        self.status = format!("Queued: {title}");
        self.next_task();
    }

    pub(in crate::ui::app) fn poll_queue(&mut self, sender: &ComponentSender<Self>) {
        let Some(queue) = self.queue.as_mut() else { return };
        let Some((_, job)) = queue.running.as_mut() else { return };
        if let Some(p) = job.progress_rx.try_iter().last() {
            queue.label.set_text(&format!("{}/{}  {}", p.done, p.total, p.current.display()));
            if p.total > 0 {
                queue.bar.set_fraction((p.done as f64 / p.total as f64).min(1.0));
            }
        }
        if !job.conflict_open {
            if let Ok((src, dst)) = job.conflict_rx.try_recv() {
                job.conflict_open = true;
                let tx = sender.input_sender().clone();
                let win = jobs::conflict_window(&src, &dst, &queue.window, move |choice| tx.emit(Msg::QueueConflict(choice)));
                // Not modal: the panels stay usable while the queue waits for an answer.
                win.set_modal(false);
                win.present();
                queue.conflict = Some(win);
            }
        }
        let result = match job.done_rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Disconnected) => Err("stopped".into()),
            Err(mpsc::TryRecvError::Empty) => return,
        };
        let title = queue.running.take().map(|(title, _)| title).unwrap_or_default();
        match result {
            Ok(report) if report.cancelled => self.status = format!("Cancelled: {title}"),
            Err(err) if err.eq_ignore_ascii_case("cancelled") => self.status = format!("Cancelled: {title}"),
            Ok(_) => self.status = format!("Done: {title}"),
            Err(err) => {
                // The rest waits, so a failure is seen before more files are touched.
                queue.pause.store(true, Ordering::Relaxed);
                queue.pause_button.set_label("Start");
                queue.label.set_text(&format!("Failed: {}", jobs::first_line(&err)));
                rust_commander::log::error(&format!("queue {title}: {err}"));
                self.status = jobs::first_line(&err);
            }
        }
        self.refresh_both(sender);
        self.next_task();
    }

    pub(in crate::ui::app) fn toggle_queue_pause(&mut self) {
        let Some(queue) = &self.queue else { return };
        let paused = !queue.pause.load(Ordering::Relaxed);
        queue.pause.store(paused, Ordering::Relaxed);
        queue.pause_button.set_label(if paused { "Start" } else { "Pause" });
        if paused {
            queue.label.set_text("Paused");
        }
        self.next_task();
    }

    pub(in crate::ui::app) fn remove_queued(&mut self) {
        let Some(queue) = self.queue.as_mut() else { return };
        let Some(row) = queue.list.selected_row() else { return };
        let at = row.index() - queue.running.is_some() as i32;
        match usize::try_from(at) {
            Ok(at) if at < queue.tasks.len() => {
                queue.tasks.remove(at);
            }
            _ => {
                self.status = "The running job stops with Cancel".into();
                return;
            }
        }
        self.next_task();
    }

    pub(in crate::ui::app) fn cancel_queued(&mut self) {
        let Some(queue) = self.queue.as_mut() else { return };
        if let Some((_, job)) = &queue.running {
            job.cancel.store(true, Ordering::Relaxed);
        }
        // Its close handler answers Cancel, which frees a worker that waits on the question.
        if let Some(win) = queue.conflict.take() {
            win.close();
        }
    }

    pub(in crate::ui::app) fn answer_queue_conflict(&mut self, choice: Choice) {
        let Some(queue) = self.queue.as_mut() else { return };
        // A late answer from a closed dialog must not answer the next question.
        if let Some((_, job)) = queue.running.as_mut() {
            if std::mem::replace(&mut job.conflict_open, false) {
                let _ = job.answer_tx.send(choice);
            }
        }
        if let Some(win) = queue.conflict.take() {
            win.close();
        }
    }

    // Dropping the job closes its channels. A worker that waits on a question then gets Cancel and ends.
    pub(in crate::ui::app) fn close_queue(&mut self) {
        let Some(queue) = self.queue.take() else { return };
        if let Some((_, job)) = &queue.running {
            job.cancel.store(true, Ordering::Relaxed);
            self.status = "Queue cancelled".into();
        }
        if let Some(win) = queue.conflict {
            win.close();
        }
    }

    fn next_task(&mut self) {
        let Some(queue) = self.queue.as_mut() else { return };
        let paused = queue.pause.load(Ordering::Relaxed);
        if queue.running.is_none() && !paused {
            if let Some(task) = queue.tasks.pop_front() {
                let (job, pipes) = jobs::new_job(Work::Transfer);
                jobs::run_transfer(task.items, task.kind, Arc::clone(&queue.pause), pipes);
                queue.running = Some((task.title, job));
                queue.bar.set_fraction(0.0);
            }
        }
        if queue.running.is_some() || !queue.tasks.is_empty() || paused {
            paint(queue);
            return;
        }
        // Its close handler goes first, so a late QueueClosed can never tear down a newer queue.
        if let Some(QueueUi { window, closed, .. }) = self.queue.take() {
            window.disconnect(closed);
            window.close();
        }
    }
}

fn paint(queue: &QueueUi) {
    while let Some(child) = queue.list.first_child() {
        queue.list.remove(&child);
    }
    let running = queue.running.iter().map(|(title, _)| format!("▶ {title}"));
    for text in running.chain(queue.tasks.iter().map(|task| task.title.clone())) {
        let label = gtk::Label::new(Some(&text));
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        queue.list.append(&label);
    }
}

fn queue_window(parent: &gtk::Window, tx: relm4::Sender<Msg>) -> QueueUi {
    let window = gtk::Window::new();
    window.set_title(Some("Queue"));
    window.set_transient_for(Some(parent));
    window.set_default_width(560);
    let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
    column.set_margin_all(12);
    let label = gtk::Label::new(None);
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    let bar = gtk::ProgressBar::new();
    bar.add_css_class("job");
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_min_content_height(160);
    scroll.set_vexpand(true);
    scroll.set_child(Some(&list));
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let pause_button = gtk::Button::with_label("Pause");
    let remove = gtk::Button::with_label("Remove");
    let cancel = gtk::Button::with_label("Cancel current");
    let tx_pause = tx.clone();
    crate::ui::shell::on_click(&pause_button, move || tx_pause.emit(Msg::QueuePause));
    let tx_remove = tx.clone();
    crate::ui::shell::on_click(&remove, move || tx_remove.emit(Msg::QueueRemove));
    let tx_cancel = tx.clone();
    crate::ui::shell::on_click(&cancel, move || tx_cancel.emit(Msg::QueueCancel));
    // Closing the window cancels the running job and drops the rest.
    let closed = window.connect_close_request(move |_| {
        tx.emit(Msg::QueueClosed);
        gtk::glib::Propagation::Proceed
    });
    row.append(&pause_button);
    row.append(&remove);
    row.append(&cancel);
    column.append(&label);
    column.append(&bar);
    column.append(&scroll);
    column.append(&row);
    window.set_child(Some(&column));
    window.present();
    QueueUi {
        window,
        list,
        label,
        bar,
        pause_button,
        tasks: VecDeque::new(),
        running: None,
        pause: Arc::new(AtomicBool::new(false)),
        conflict: None,
        closed,
    }
}

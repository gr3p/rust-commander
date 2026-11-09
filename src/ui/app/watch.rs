use relm4::gtk;
use relm4::gtk::prelude::*;
use relm4::prelude::*;
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use rust_commander::state::PaneId;

use super::App;
use crate::ui::msg::Msg;
use std::time::{Duration, Instant, SystemTime};

const MAX_WAIT: Duration = Duration::from_secs(2);
const DIR_POLL: Duration = Duration::from_secs(1);

pub(super) struct DirWatch {
    pub path: PathBuf,
    // mtime from the last listing. Compared on a timer because CIFS never wakes the monitor.
    seen: Option<SystemTime>,
    _monitor: Option<gtk::gio::FileMonitor>,
}

enum DirFollow {
    Keep,
    Remember(SystemTime),
    Reload,
}

fn dir_follow(seen: Option<SystemTime>, mtime: Option<SystemTime>) -> DirFollow {
    match (seen, mtime) {
        (Some(old), Some(now)) if old != now => DirFollow::Reload,
        (None, Some(now)) => DirFollow::Remember(now),
        _ => DirFollow::Keep,
    }
}

impl App {
    pub(in crate::ui::app) fn arm_watches(&mut self, sender: &ComponentSender<Self>) {
        for id in [PaneId::Left, PaneId::Right] {
            let path = self.state.tab(id).path.clone();
            let pane = id.index();
            if self.watches[pane]
                .as_ref()
                .is_some_and(|watch| watch.path == path)
            {
                continue;
            }
            let file = gtk::gio::File::for_path(&path);
            let monitor = file
                .monitor_directory(
                    gtk::gio::FileMonitorFlags::NONE,
                    gtk::gio::Cancellable::NONE,
                )
                .ok();
            if let Some(monitor) = &monitor {
                let generation = Rc::new(Cell::new(0u64));
                let first = Rc::new(Cell::new(None::<Instant>));
                let tx = sender.input_sender().clone();
                let watched = path.clone();
                monitor.connect_changed(move |_, _, _, _| {
                    let next = generation.get().wrapping_add(1);
                    generation.set(next);
                    if first.get().is_none() {
                        first.set(Some(Instant::now()));
                    }
                    let (generation, first) = (Rc::clone(&generation), Rc::clone(&first));
                    let tx = tx.clone();
                    let watched = watched.clone();
                    gtk::glib::timeout_add_local_once(Duration::from_millis(350), move || {
                        // A file that keeps changing (a download) would otherwise hold the reload back for good.
                        let due = generation.get() == next
                            || first.get().is_some_and(|at| at.elapsed() >= MAX_WAIT);
                        if due {
                            first.set(None);
                            tx.emit(Msg::DirChanged(pane, watched));
                        }
                    });
                });
            }
            self.watches[pane] = Some(DirWatch {
                path,
                seen: None,
                _monitor: monitor,
            });
        }
    }

    pub(in crate::ui::app) fn remember_dir(&mut self, pane: usize, stamp: SystemTime) {
        let Some(watch) = self.watches[pane].as_mut() else {
            return;
        };
        if watch.path == self.state.tab(PaneId::from_index(pane)).path {
            watch.seen = Some(stamp);
        }
    }

    // Ask the next poll to look now. Used when the other pane is entered or the filter opens.
    pub(in crate::ui::app) fn dirs_soon(&mut self) {
        self.dir_poll = Instant::now() - DIR_POLL;
    }

    pub(in crate::ui::app) fn poll_dirs(&mut self, sender: &ComponentSender<Self>) {
        if self.dir_busy || self.dir_poll.elapsed() < DIR_POLL {
            return;
        }
        self.dir_poll = Instant::now();
        let mut jobs = Vec::new();
        for pane in 0..2 {
            let Some(watch) = &self.watches[pane] else {
                continue;
            };
            let id = PaneId::from_index(pane);
            if self.state.tab(id).feed || self.state.tab(id).path != watch.path {
                continue;
            }
            let token = self.state.tab(id).token;
            if self.loads.iter().any(|(live, _)| *live == token) {
                continue;
            }
            jobs.push((pane, watch.path.clone(), watch.seen));
        }
        if jobs.is_empty() {
            return;
        }
        self.dir_busy = true;
        let tx = sender.input_sender().clone();
        std::thread::spawn(move || {
            let stamps = jobs
                .into_iter()
                .map(|(pane, path, seen)| {
                    let mtime = std::fs::metadata(&path)
                        .and_then(|meta| meta.modified())
                        .ok();
                    (pane, path, seen, mtime)
                })
                .collect();
            tx.emit(Msg::DirStamp(stamps));
        });
    }

    pub(in crate::ui::app) fn apply_dir_stamps(
        &mut self,
        stamps: Vec<(usize, PathBuf, Option<SystemTime>, Option<SystemTime>)>,
        sender: &ComponentSender<Self>,
    ) {
        self.dir_busy = false;
        for (pane, path, seen, mtime) in stamps {
            let id = PaneId::from_index(pane);
            if self.state.tab(id).feed || self.state.tab(id).path != path {
                continue;
            }
            let follow = {
                let Some(watch) = self.watches[pane].as_mut() else {
                    continue;
                };
                // A listing finished while the stat was out. Its stamp wins.
                if watch.path != path || watch.seen != seen {
                    continue;
                }
                let follow = dir_follow(watch.seen, mtime);
                if let DirFollow::Remember(mtime) = follow {
                    watch.seen = Some(mtime);
                }
                follow
            };
            if matches!(follow, DirFollow::Reload) {
                rust_commander::log::info(&format!("dir changed pane={pane}"));
                self.reload_watched(pane, &path, sender);
            }
        }
    }

    pub(in crate::ui::app) fn reload_watched(
        &mut self,
        pane: usize,
        path: &std::path::Path,
        sender: &ComponentSender<Self>,
    ) {
        let id = PaneId::from_index(pane);
        if self.state.tab(id).feed || self.state.tab(id).path != path {
            return;
        }
        let (path, token) = self.state.refresh(id);
        let tab = self.state.pane(id).active;
        self.spawn_load(sender, id, tab, path, token);
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::{DirFollow, dir_follow};

    #[test]
    fn same_mtime_stays_a_changed_one_reloads() {
        let t0 = SystemTime::UNIX_EPOCH;
        let t1 = t0 + Duration::from_secs(1);
        assert!(matches!(dir_follow(Some(t0), Some(t0)), DirFollow::Keep));
        assert!(matches!(dir_follow(Some(t0), Some(t1)), DirFollow::Reload));
        assert!(matches!(
            dir_follow(None, Some(t0)),
            DirFollow::Remember(_)
        ));
        assert!(matches!(dir_follow(Some(t0), None), DirFollow::Keep));
    }
}

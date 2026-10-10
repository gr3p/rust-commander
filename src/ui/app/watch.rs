use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use relm4::gtk;
use relm4::gtk::prelude::*;
use relm4::prelude::*;

use rust_commander::state::PaneId;

use super::App;
use crate::ui::msg::Msg;
use std::time::Duration;

pub(super) struct DirWatch {
    pub path: PathBuf,
    _monitor: Option<gtk::gio::FileMonitor>,
}

impl App {
    pub(in crate::ui::app) fn arm_watches(&mut self, sender: &ComponentSender<Self>) {
        for id in [PaneId::Left, PaneId::Right] {
            let path = self.state.tab(id).path.clone();
            let pane = id.index();
            if self.watches[pane].as_ref().is_some_and(|watch| watch.path == path) {
                continue;
            }
            let file = gtk::gio::File::for_path(&path);
            let monitor = file.monitor_directory(gtk::gio::FileMonitorFlags::NONE, gtk::gio::Cancellable::NONE).ok();
            if let Some(monitor) = &monitor {
                let generation = Rc::new(Cell::new(0u64));
                let tx = sender.input_sender().clone();
                let watched = path.clone();
                monitor.connect_changed(move |_, _, _, _| {
                    let next = generation.get().wrapping_add(1);
                    generation.set(next);
                    let generation = Rc::clone(&generation);
                    let tx = tx.clone();
                    let watched = watched.clone();
                    gtk::glib::timeout_add_local_once(Duration::from_millis(350), move || {
                        if generation.get() == next {
                            tx.emit(Msg::DirChanged(pane, watched));
                        }
                    });
                });
            }
            self.watches[pane] = Some(DirWatch { path, _monitor: monitor });
        }
    }

    pub(in crate::ui::app) fn reload_watched(&mut self, pane: usize, path: &std::path::Path, sender: &ComponentSender<Self>) {
        let id = PaneId::from_index(pane);
        if self.state.tab(id).feed || self.state.tab(id).path != path {
            return;
        }
        let (path, token) = self.state.refresh(id);
        let tab = self.state.pane(id).active;
        self.spawn_load(sender, id, tab, path, token);
    }
}

use relm4::gtk;
use relm4::gtk::gdk::prelude::DisplayExt;
use relm4::gtk::gio::prelude::InputStreamExt;
use relm4::ComponentSender;

use rust_commander::clipboard::desktop_files;
use rust_commander::ops::Kind;

use crate::ui::msg::Msg;

use super::{jobs, App};

const FILE_CLIP: &[&str] = &[
    "x-special/gnome-copied-files",
    "x-special/mate-copied-files",
    "text/uri-list",
];

impl App {
    // The folder the list is showing. A file copied in Files lands here.
    pub(in crate::ui::app) fn paste_desktop(&mut self, sender: &ComponentSender<Self>) {
        let dest = self.state.tab(self.state.active).path.clone();
        let Some(clipboard) = gtk::gdk::Display::default().map(|display| display.clipboard()) else {
            self.status = "Nothing to paste".into();
            return;
        };
        let tx = sender.input_sender().clone();
        clipboard.read_async(FILE_CLIP, gtk::glib::Priority::DEFAULT, gtk::gio::Cancellable::NONE, move |result| {
            let msg = match result {
                Ok((stream, mime)) => match read_clipboard(&stream) {
                    Ok(bytes) => Msg::DesktopPaste { mime: mime.to_string(), bytes, dest },
                    Err(err) => Msg::Status(err),
                },
                Err(_) => Msg::Status("Nothing to paste".into()),
            };
            tx.emit(msg);
        });
    }

    pub(in crate::ui::app) fn paste_desktop_files(
        &mut self,
        mime: &str,
        bytes: &[u8],
        dest: &std::path::Path,
        sender: &ComponentSender<Self>,
    ) {
        let files = desktop_files(mime, bytes);
        if files.paths.is_empty() {
            self.status = "Nothing to paste".into();
            return;
        }
        let items: Vec<_> = jobs::items_into(files.paths, dest).into_iter().filter(|(src, dst)| src != dst).collect();
        if items.is_empty() {
            self.status = "Already here".into();
            return;
        }
        let cut = files.cut;
        let kind = if cut { Kind::Move } else { Kind::Copy };
        let started = self.job.is_none();
        self.spawn_transfer(items, kind, sender);
        // A cut is one paste. Leaving it would move the same files again.
        if cut && started && self.job.is_some() {
            if let Some(clipboard) = gtk::gdk::Display::default().map(|display| display.clipboard()) {
                if clipboard.set_content(None::<&gtk::gdk::ContentProvider>).is_err() {
                    clipboard.set_text("");
                }
            }
        }
    }
}

fn read_clipboard(stream: &gtk::gio::InputStream) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let chunk = stream.read_bytes(64 * 1024, gtk::gio::Cancellable::NONE).map_err(|err| err.to_string())?;
        if chunk.is_empty() {
            break;
        }
        out.extend_from_slice(&chunk);
        if out.len() > 1024 * 1024 {
            return Err("Clipboard is too large".into());
        }
    }
    let _ = stream.close(gtk::gio::Cancellable::NONE);
    Ok(out)
}

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use relm4::gtk;
use relm4::gtk::prelude::*;
use relm4::prelude::*;

use rust_commander::listing::format_size;
use rust_commander::mount::{self, Candidate, Job};

use super::App;
use crate::ui::msg::Msg;
use crate::ui::shell;

pub(in crate::ui::app) struct MountUi {
    pub note: gtk::Label,
    pub actions: Vec<gtk::Button>,
}

impl App {
    pub(in crate::ui::app) fn open_mount(&mut self, sender: &ComponentSender<Self>) {
        self.close_dialog();
        self.mount_gen += 1;
        let generation = self.mount_gen;
        let tx = sender.input_sender().clone();
        let (win, ui) = dialog(&self.shell.window, generation, tx.clone());
        win.connect_close_request(move |_| {
            tx.emit(Msg::MountClosed(generation));
            gtk::glib::Propagation::Proceed
        });
        let win_esc = win.clone();
        shell::on_escape(&win, move || win_esc.close());
        win.present();
        self.mount_ui = Some(ui);
        self.dialog = Some(win);
    }

    pub(in crate::ui::app) fn mount_finished(
        &mut self,
        generation: u64,
        point: PathBuf,
        open: bool,
        error: Option<String>,
        sender: &ComponentSender<Self>,
    ) {
        // The dialog may have been closed, or replaced by another one, while pkexec was asking.
        let shown = self.mount_gen == generation && self.mount_ui.is_some();
        if let Some(error) = error {
            if let Some(ui) = self.mount_ui.as_ref().filter(|_| shown) {
                for button in &ui.actions {
                    button.set_sensitive(true);
                }
                ui.note.set_text(&error);
                ui.note.set_visible(true);
            }
            self.status = error;
            return;
        }
        self.status = if open {
            format!("Mounted {}", point.display())
        } else {
            format!("Unmounted {}", point.display())
        };
        if shown {
            self.close_dialog();
        }
        self.refresh_drives(sender);
        if open {
            self.goto_mount(self.state.active.index(), point, sender);
        }
    }
}

fn dialog(parent: &gtk::Window, generation: u64, tx: relm4::Sender<Msg>) -> (gtk::Window, MountUi) {
    let win = gtk::Window::new();
    win.set_title(Some("Mount"));
    win.set_modal(true);
    win.set_transient_for(Some(parent));
    win.set_default_width(480);
    let note = gtk::Label::new(None);
    note.set_xalign(0.0);
    note.set_wrap(true);
    note.set_visible(false);
    let mut actions = Vec::new();

    let (cands, disk_error) = match mount::candidates() {
        Ok(list) => (list, None),
        Err(err) => (Vec::new(), Some(err)),
    };
    let disk = page();
    let mut disk_wire = None;
    if cands.is_empty() {
        disk.append(&plain(disk_error.as_deref().unwrap_or("No unmounted disks")));
    } else {
        let labels: Vec<String> = cands.iter().map(candidate_line).collect();
        let shown: Vec<&str> = labels.iter().map(String::as_str).collect();
        let choice = gtk::DropDown::from_strings(&shown);
        choice.set_hexpand(true);
        let (point_row, _, point) = labeled("Mount point", false);
        let (pass_row, _, pass) = labeled("Password", true);
        pass_row.set_visible(false);
        let mount_btn = action_button("Mount");
        disk.append(&row("Disk", &choice));
        disk.append(&point_row);
        disk.append(&pass_row);
        disk.append(&mount_btn);
        actions.push(mount_btn.clone());
        follow_disk(&choice, &cands, &point, &pass, &pass_row);
        disk_wire = Some((mount_btn, choice, cands, point, pass));
    }

    let net = page();
    let kind = gtk::DropDown::from_strings(&["SMB", "NFS"]);
    kind.set_hexpand(true);
    let (server_row, _, server) = labeled("Server", false);
    let (share_row, share_name, share) = labeled("Share", false);
    let (user_row, _, user) = labeled("User", false);
    let (secret_row, _, secret) = labeled("Password", true);
    let (net_point_row, _, net_point) = labeled("Mount point", false);
    net_point.set_text("/mnt/share");
    let net_btn = action_button("Mount");
    net.append(&row("Type", &kind));
    net.append(&server_row);
    net.append(&share_row);
    net.append(&user_row);
    net.append(&secret_row);
    net.append(&net_point_row);
    net.append(&net_btn);
    actions.push(net_btn.clone());
    watch_share(&share, &net_point);
    watch_kind(&kind, &share_name, &user_row, &secret_row);

    let held: Vec<_> = mount::held()
        .into_iter()
        .filter(|row| !crate::ui::drives::managed(Path::new(&row.point)))
        .collect();
    let release = page();
    let mut release_wire = None;
    if held.is_empty() {
        release.append(&plain("Nothing mounted under /mnt"));
    } else {
        let labels: Vec<String> = held.iter().map(|row| format!("{}    {}    {}", row.point, row.source, row.kind)).collect();
        let shown: Vec<&str> = labels.iter().map(String::as_str).collect();
        let choice = gtk::DropDown::from_strings(&shown);
        choice.set_hexpand(true);
        let points: Vec<String> = held.iter().map(|row| row.point.clone()).collect();
        let button = action_button("Unmount");
        release.append(&row("Mount", &choice));
        release.append(&button);
        actions.push(button.clone());
        release_wire = Some((button, choice, points));
    }

    let tabs = gtk::Notebook::new();
    for (title, page) in [("Disk", &disk), ("Network", &net), ("Unmount", &release)] {
        tabs.append_page(page, Some(&tab(title)));
    }
    tabs.connect_switch_page(|tabs, page, _| {
        if let Some(tab) = tabs.tab_label(page) {
            shell::blink_tab(&tab);
        }
    });

    let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
    column.set_margin_all(12);
    column.append(&tabs);
    column.append(&note);
    win.set_child(Some(&column));

    if let Some((button, choice, cands, point, pass)) = disk_wire {
        wire_disk(&button, &choice, &cands, &point, &pass, &note, &actions, generation, tx.clone());
    }
    wire_net(&net_btn, &kind, &server, &share, &user, &secret, &net_point, &note, &actions, generation, tx.clone());
    if let Some((button, choice, points)) = release_wire {
        wire_release(&button, &choice, &points, &note, &actions, generation, tx);
    }
    (win, MountUi { note, actions })
}

fn follow_disk(choice: &gtk::DropDown, cands: &[Candidate], point: &gtk::Entry, pass: &gtk::Entry, pass_row: &gtk::Box) {
    let choice_apply = choice.clone();
    let cands = Rc::new(cands.to_vec());
    let point = point.clone();
    let pass = pass.clone();
    let pass_row = pass_row.clone();
    let quiet = Rc::new(std::cell::Cell::new(true));
    let quiet_notify = quiet.clone();
    let cands_notify = Rc::clone(&cands);
    let point_notify = point.clone();
    let pass_notify = pass.clone();
    let pass_row_notify = pass_row.clone();
    choice.connect_selected_notify(move |choice| {
        show_disk(choice, &cands_notify, &point_notify, &pass_notify, &pass_row_notify, !quiet_notify.get());
    });
    show_disk(&choice_apply, &cands, &point, &pass, &pass_row, false);
    quiet.set(false);
}

fn show_disk(choice: &gtk::DropDown, cands: &[Candidate], point: &gtk::Entry, pass: &gtk::Entry, pass_row: &gtk::Box, blink: bool) {
    let Some(cand) = cands.get(choice.selected() as usize) else { return };
    point.set_text(&cand.point);
    pass.set_text("");
    pass_row.set_visible(cand.locked);
    if blink && choice.is_mapped() {
        shell::blink_widget(choice.upcast_ref());
    }
}

fn wire_disk(
    button: &gtk::Button,
    choice: &gtk::DropDown,
    cands: &[Candidate],
    point: &gtk::Entry,
    pass: &gtk::Entry,
    note: &gtk::Label,
    actions: &[gtk::Button],
    generation: u64,
    tx: relm4::Sender<Msg>,
) {
    let choice = choice.clone();
    let cands = Rc::new(cands.to_vec());
    let point = point.clone();
    let pass = pass.clone();
    let note = note.clone();
    let actions = actions.to_vec();
    let disk = button.clone();
    point.connect_activate(move |_| {
        disk.emit_clicked();
    });
    let disk = button.clone();
    pass.connect_activate(move |_| {
        disk.emit_clicked();
    });
    shell::on_click(button, move || {
        let Some(cand) = cands.get(choice.selected() as usize) else {
            show_note(&note, "Select a disk");
            return;
        };
        let (uid, gid) = mount::ids();
        let text = point.text();
        let job = if cand.locked {
            mount::luks_job(&cand.device, text.as_str(), uid, gid, pass.text().as_str())
        } else {
            mount::block_job(&cand.device, &cand.mount_as, text.as_str(), uid, gid)
        };
        start(job, &text, &note, &actions, generation, &tx);
    });
}

fn wire_release(
    button: &gtk::Button,
    choice: &gtk::DropDown,
    points: &[String],
    note: &gtk::Label,
    actions: &[gtk::Button],
    generation: u64,
    tx: relm4::Sender<Msg>,
) {
    let choice = choice.clone();
    let points = Rc::new(points.to_vec());
    let note = note.clone();
    let actions = actions.to_vec();
    shell::on_click(button, move || {
        let Some(path) = points.get(choice.selected() as usize) else {
            show_note(&note, "Select a mount");
            return;
        };
        match mount::umount_job(path) {
            Ok(job) => launch(&actions, &note, job, PathBuf::from(path), false, generation, tx.clone()),
            Err(err) => show_note(&note, &err),
        }
    });
}

fn wire_net(
    button: &gtk::Button,
    kind: &gtk::DropDown,
    server: &gtk::Entry,
    share: &gtk::Entry,
    user: &gtk::Entry,
    secret: &gtk::Entry,
    point: &gtk::Entry,
    note: &gtk::Label,
    actions: &[gtk::Button],
    generation: u64,
    tx: relm4::Sender<Msg>,
) {
    let kind = kind.clone();
    let server = server.clone();
    let share = share.clone();
    let user = user.clone();
    let secret = secret.clone();
    let point = point.clone();
    let note = note.clone();
    let actions = actions.to_vec();
    for entry in [&server, &share, &user, &secret, &point] {
        let button = button.clone();
        entry.connect_activate(move |_| {
            button.emit_clicked();
        });
    }
    shell::on_click(button, move || {
        let (uid, gid) = mount::ids();
        let text = point.text();
        let job = if kind.selected() == 1 {
            mount::nfs_job(server.text().as_str(), share.text().as_str(), text.as_str())
        } else {
            mount::cifs_job(
                server.text().as_str(),
                share.text().as_str(),
                user.text().as_str(),
                secret.text().as_str(),
                text.as_str(),
                uid,
                gid,
            )
        };
        start(job, &text, &note, &actions, generation, &tx);
    });
}

fn start(
    job: Result<Job, String>,
    text: &str,
    note: &gtk::Label,
    actions: &[gtk::Button],
    generation: u64,
    tx: &relm4::Sender<Msg>,
) {
    match job {
        Ok(job) => {
            let path = mount::check_point(text).unwrap_or_else(|_| PathBuf::from(text.trim()));
            launch(actions, note, job, path, true, generation, tx.clone());
        }
        Err(err) => show_note(note, &err),
    }
}

fn launch(
    actions: &[gtk::Button],
    note: &gtk::Label,
    job: Job,
    point: PathBuf,
    open: bool,
    generation: u64,
    tx: relm4::Sender<Msg>,
) {
    note.set_visible(false);
    for button in actions {
        button.set_sensitive(false);
    }
    std::thread::spawn(move || {
        let error = mount::pkexec(&job).err();
        job.wipe();
        tx.emit(Msg::MountFinished {
            generation,
            point,
            open,
            error,
        });
    });
}

fn watch_share(share: &gtk::Entry, point: &gtk::Entry) {
    let point = point.clone();
    let auto = Rc::new(RefCell::new(String::from("/mnt/share")));
    share.connect_changed(move |entry| {
        if entry.text().trim().is_empty() {
            return;
        }
        let next = format!("/mnt/{}", mount::point_name(entry.text().as_str()));
        if point.text().as_str() == auto.borrow().as_str() {
            point.set_text(&next);
            *auto.borrow_mut() = next;
        }
    });
}

fn watch_kind(
    kind: &gtk::DropDown,
    share_name: &gtk::Label,
    user_row: &gtk::Box,
    secret_row: &gtk::Box,
) {
    let share_name = share_name.clone();
    let user_row = user_row.clone();
    let secret_row = secret_row.clone();
    kind.connect_selected_notify(move |drop| {
        let nfs = drop.selected() == 1;
        user_row.set_visible(!nfs);
        secret_row.set_visible(!nfs);
        share_name.set_text(if nfs { "Export" } else { "Share" });
    });
}

fn candidate_line(cand: &Candidate) -> String {
    let dev = cand.device.rsplit('/').next().unwrap_or(&cand.device);
    let label = cand.label.as_deref().unwrap_or("-");
    let lock = if cand.locked { "    locked" } else { "" };
    format!(
        "{dev}    {label}    {}    {}{lock}",
        cand.kind,
        format_size(cand.size)
    )
}

fn show_note(note: &gtk::Label, text: &str) {
    note.set_text(text);
    note.set_visible(true);
}

fn page() -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    page.set_margin_top(12);
    page.set_margin_bottom(4);
    page.set_margin_start(12);
    page.set_margin_end(12);
    page
}

fn plain(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label
}

fn row(title: &str, child: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let name = gtk::Label::new(Some(title));
    name.set_xalign(0.0);
    name.set_width_chars(12);
    name.add_css_class("dim-label");
    child.set_hexpand(true);
    row.append(&name);
    row.append(child);
    row
}

fn tab(title: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(title));
    let flash = label.clone();
    let click = gtk::GestureClick::new();
    click.connect_released(move |_, _, _, _| shell::blink_tab(flash.upcast_ref()));
    label.add_controller(click);
    label
}

fn labeled(title: &str, secret: bool) -> (gtk::Box, gtk::Label, gtk::Entry) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let name = gtk::Label::new(Some(title));
    name.set_xalign(0.0);
    name.set_width_chars(12);
    name.add_css_class("dim-label");
    let entry = gtk::Entry::new();
    entry.set_hexpand(true);
    if secret {
        entry.set_visibility(false);
    }
    row.append(&name);
    row.append(&entry);
    (row, name, entry)
}

fn action_button(label: &str) -> gtk::Button {
    let button = gtk::Button::with_label(label);
    button.add_css_class("fkey");
    button.set_halign(gtk::Align::End);
    button
}


use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use relm4::gtk::gio::{self, prelude::*};
use relm4::gtk::glib;

// A removable volume the desktop could mount but has not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Idle {
    pub id: String,
    pub label: String,
    pub optical: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Actions {
    pub unmount: bool,
    pub eject: bool,
}

fn volume_id(volume: &gio::Volume) -> String {
    volume
        .identifier(gio::VOLUME_IDENTIFIER_KIND_UNIX_DEVICE)
        .or_else(|| volume.uuid())
        .map(|id| id.to_string())
        .unwrap_or_else(|| volume.name().to_string())
}

fn can_release(drive: &gio::Drive) -> bool {
    drive.can_eject() || drive.can_stop()
}

pub fn idle_volumes() -> Vec<Idle> {
    let mut out: Vec<Idle> = gio::VolumeMonitor::get()
        .volumes()
        .into_iter()
        .filter(|volume| volume.get_mount().is_none() && volume.can_mount())
        .filter(|volume| {
            volume
                .drive()
                .is_some_and(|drive| drive.is_removable() || drive.is_media_removable() || can_release(&drive))
        })
        .map(|volume| {
            let id = volume_id(&volume);
            Idle { optical: rust_commander::places::is_optical_device(&id), id, label: volume.name().to_string() }
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

fn find_volume(id: &str) -> Option<gio::Volume> {
    gio::VolumeMonitor::get().volumes().into_iter().find(|volume| volume_id(volume) == id)
}

fn find_mount(path: &Path) -> Option<gio::Mount> {
    gio::VolumeMonitor::get().mounts().into_iter().find(|mount| mount.root().path().is_some_and(|root| root == path))
}

fn drive_of(mount: &gio::Mount) -> Option<gio::Drive> {
    mount.drive().or_else(|| mount.volume().and_then(|volume| volume.drive()))
}

pub fn managed(path: &Path) -> bool {
    find_mount(path).is_some()
}

pub fn actions(path: &Path) -> Actions {
    let Some(mount) = find_mount(path) else { return Actions::default() };
    Actions { unmount: mount.can_unmount(), eject: drive_of(&mount).is_some_and(|drive| can_release(&drive)) }
}

pub fn idle_can_eject(id: &str) -> bool {
    find_volume(id).and_then(|volume| volume.drive()).is_some_and(|drive| can_release(&drive))
}

fn message(err: glib::Error) -> String {
    err.message().to_string()
}

// The mount point may appear after the call returns, so a failed call still counts when the volume ended up mounted.
pub fn mount_volume(id: &str, done: impl FnOnce(Result<PathBuf, String>) + 'static) {
    let Some(volume) = find_volume(id) else {
        done(Err("The volume is gone".into()));
        return;
    };
    let again = volume.clone();
    volume.mount(gio::MountMountFlags::NONE, Some(&gio::MountOperation::new()), gio::Cancellable::NONE, move |result| {
        let root = again.get_mount().and_then(|mount| mount.root().path());
        done(match (root, result) {
            (Some(path), _) => Ok(path),
            (None, Err(err)) => Err(message(err)),
            (None, Ok(())) => Err("Mounted, but not as a local folder".into()),
        });
    });
}

fn eject_drive(drive: &gio::Drive, done: impl FnOnce(Result<(), String>) + 'static) {
    let op = gio::MountOperation::new();
    let finish = move |result: Result<(), glib::Error>| done(result.map_err(message));
    if drive.can_eject() {
        drive.eject_with_operation(gio::MountUnmountFlags::NONE, Some(&op), gio::Cancellable::NONE, finish);
    } else {
        drive.stop(gio::MountUnmountFlags::NONE, Some(&op), gio::Cancellable::NONE, finish);
    }
}

pub fn eject_volume(id: &str, done: impl FnOnce(Result<(), String>) + 'static) {
    match find_volume(id).and_then(|volume| volume.drive()).filter(can_release) {
        Some(drive) => eject_drive(&drive, done),
        None => done(Err("This volume cannot be ejected".into())),
    }
}

// `eject` also powers off or opens the tray; otherwise the drive stays and only the volume is unmounted.
pub fn release(path: &Path, eject: bool, done: impl FnOnce(Result<(), String>) + 'static) {
    let Some(mount) = find_mount(path) else {
        done(Err("The desktop does not manage this mount".into()));
        return;
    };
    if eject {
        if let Some(drive) = drive_of(&mount).filter(can_release) {
            eject_drive(&drive, done);
            return;
        }
    }
    mount.unmount_with_operation(
        gio::MountUnmountFlags::NONE,
        Some(&gio::MountOperation::new()),
        gio::Cancellable::NONE,
        move |result| done(result.map_err(message)),
    );
}

// `notify` runs on the main loop whenever drives, volumes or mounts may have changed.
pub fn watch(notify: impl Fn() + 'static) {
    let notify = Rc::new(notify);
    let monitor = gio::VolumeMonitor::get();
    macro_rules! on {
        ($connect:ident) => {{
            let notify = Rc::clone(&notify);
            monitor.$connect(move |_, _| notify());
        }};
    }
    on!(connect_drive_connected);
    on!(connect_drive_disconnected);
    on!(connect_drive_changed);
    on!(connect_volume_added);
    on!(connect_volume_removed);
    on!(connect_volume_changed);
    on!(connect_mount_added);
    on!(connect_mount_removed);
    on!(connect_mount_changed);
    // Without a gvfs volume monitor nothing signals a mount, so /proc/mounts is also watched.
    let last = std::cell::RefCell::new(std::fs::read_to_string("/proc/mounts").unwrap_or_default());
    glib::timeout_add_local(Duration::from_secs(2), move || {
        let now = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
        if *last.borrow() != now {
            *last.borrow_mut() = now;
            notify();
        }
        glib::ControlFlow::Continue
    });
}

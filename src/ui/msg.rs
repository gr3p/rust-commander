use std::sync::atomic::{AtomicBool, Ordering};

use relm4::gtk::gdk::{Key, ModifierType};

use comarchy::ops::{self, Choice};

pub static SYNCING: AtomicBool = AtomicBool::new(false);

pub fn syncing() -> bool {
    SYNCING.load(Ordering::Relaxed)
}

#[derive(Debug)]
pub enum Msg {
    Key(Key, ModifierType),
    FKey(u8, bool),
    Listed(usize, usize, u64, Result<Vec<comarchy::entry::Entry>, String>),
    Cursor(usize, u32),
    Activate(usize),
    FilterChanged(usize, String),
    Sort(usize, comarchy::listing::SortKey),
    Submit(String),
    CancelAsk,
    Hotlist,
    HotlistPick(usize),
    HotlistMenu(usize),
    HotlistBack,
    HotlistAdd,
    HotlistRemove,
    HotlistConfigure,
    HotlistAdded(String, String, bool),
    HotlistStored(Vec<comarchy::config::Bookmark>),
    Conflict(Choice),
    Poll,
    Preview { generation: u64, text: String },
    ReloadTheme,
    PickPlace(usize, std::path::PathBuf),
    Mount(std::path::PathBuf),
    ColumnSized(usize),
    ToggleAutoColumns,
    ToggleHidden,
    PickBackground,
    BackgroundImage(std::path::PathBuf),
    BackgroundColor(String),
    BackgroundAlpha(u8),
    CloseSettings,
    Smoke,
    JobDone(Result<ops::Report, String>),
}

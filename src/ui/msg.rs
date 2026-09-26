use std::sync::atomic::{AtomicBool, Ordering};

use relm4::gtk::gdk::{Key, ModifierType};

use rust_commander::ops::{self, Choice};

pub static SYNCING: AtomicBool = AtomicBool::new(false);

pub fn syncing() -> bool {
    SYNCING.load(Ordering::Relaxed)
}

#[derive(Debug)]
pub enum Msg {
    Key(Key, ModifierType),
    FKey(u8, bool),
    Listed(usize, usize, u64, Result<Vec<rust_commander::entry::Entry>, String>),
    Cursor(usize, u32),
    Activate(usize),
    FilterChanged(usize, String),
    CloseSearch,
    SearchEscape,
    SearchStart,
    SearchCancel,
    SearchGo(usize),
    SearchEvent(u64, rust_commander::search::Event),
    Sort(usize, rust_commander::listing::SortKey),
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
    HotlistStored(Vec<rust_commander::config::Bookmark>),
    Conflict(Choice),
    Poll,
    Preview { generation: u64, body: rust_commander::lister::PreviewBody },
    ImageReady { generation: u64, body: rust_commander::lister::PreviewBody },
    ImageClosed(u64),
    ReloadTheme,
    PickPlace(usize, std::path::PathBuf),
    Mount(usize, std::path::PathBuf),
    FocusPane(usize),
    ColumnSized(usize),
    ToggleAutoColumns,
    ToggleHidden,
    ToggleModeButtons,
    ToggleFileIcons,
    ToggleFlashFeedback,
    ToggleImageScale,
    ImageStep(i32),
    SetCharset(rust_commander::lister::Charset),
    ToggleQuickEdit,
    SaveQuick,
    PickBackground,
    ClearBackground,
    BackgroundImage(std::path::PathBuf),
    BackgroundColor(String),
    BackgroundAlpha(u8),
    CloseSettings,
    ArchiveListed(Result<rust_commander::archive::Listed, String>),
    UnpackPick(String, Vec<String>),
    PackPick(String, rust_commander::pack::Options),
    UnpackList,
    UnpackHere,
    VerifySfv,
    SetSevenZip(String),
    SetRar(String),
    SetEditor(String),
    EditorPickCancelled,
    PickEditor,
    Smoke,
    JobDone(Result<ops::Report, String>),
}

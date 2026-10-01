use crate::listing;

// Higher is more likely on the context row. Several marked files outrank the
// file type. Unpack still follows an archive under the cursor.
const HINTS: &[Act] = &[
    Act::Copy,
    Act::Move,
    Act::Pack,
    Act::PackHere,
    Act::Unpack,
    Act::UnpackHere,
    Act::Verify,
    Act::View,
    Act::Edit,
    Act::CopyPaths,
    Act::Properties,
    Act::CopyDir,
    Act::Filter,
    Act::Search,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Files,
    Archive,
    Navigate,
    Clipboard,
    View,
}

impl Group {
    pub const ALL: [Group; 5] = [Group::Files, Group::Archive, Group::Navigate, Group::Clipboard, Group::View];

    pub fn name(self) -> &'static str {
        match self {
            Group::Files => "Files",
            Group::Archive => "Archive",
            Group::Navigate => "Navigate",
            Group::Clipboard => "Clipboard",
            Group::View => "View",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Refresh,
    View,
    Edit,
    NewFile,
    Copy,
    Move,
    Queue,
    Mkdir,
    Trash,
    Delete,
    CopyAs,
    Rename,
    SelectAll,
    Properties,
    Settings,
    Pack,
    PackHere,
    Unpack,
    UnpackHere,
    MakeSfv,
    Verify,
    OtherPane,
    NextTab,
    NewTab,
    CloseTab,
    Up,
    Back,
    Forward,
    Mirror,
    PlacesLeft,
    PlacesRight,
    Hotlist,
    Open,
    Mark,
    MarkStay,
    SortName,
    SortExt,
    SortDate,
    SortSize,
    SortAttr,
    CopyPaths,
    CopyDir,
    Filter,
    Search,
    Hidden,
    QuickView,
    Terminal,
    NewShell,
    CloseShell,
    SwitchShell,
    ExternalView,
    CloseView,
    ImageFit,
    ImageStep,
    ListerText,
    ListerBinary,
    ListerHex,
    ListerAnsi,
    ListerAscii,
}

impl Act {
    pub const ALL: [Act; 59] = [
        Act::Refresh,
        Act::View,
        Act::Edit,
        Act::NewFile,
        Act::Copy,
        Act::Move,
        Act::Queue,
        Act::Mkdir,
        Act::Trash,
        Act::Delete,
        Act::CopyAs,
        Act::Rename,
        Act::SelectAll,
        Act::Properties,
        Act::Settings,
        Act::Pack,
        Act::PackHere,
        Act::Unpack,
        Act::UnpackHere,
        Act::MakeSfv,
        Act::Verify,
        Act::OtherPane,
        Act::NextTab,
        Act::NewTab,
        Act::CloseTab,
        Act::Up,
        Act::Back,
        Act::Forward,
        Act::Mirror,
        Act::PlacesLeft,
        Act::PlacesRight,
        Act::Hotlist,
        Act::Open,
        Act::Mark,
        Act::MarkStay,
        Act::SortName,
        Act::SortExt,
        Act::SortDate,
        Act::SortSize,
        Act::SortAttr,
        Act::CopyPaths,
        Act::CopyDir,
        Act::Filter,
        Act::Search,
        Act::Hidden,
        Act::QuickView,
        Act::Terminal,
        Act::NewShell,
        Act::CloseShell,
        Act::SwitchShell,
        Act::ExternalView,
        Act::CloseView,
        Act::ImageFit,
        Act::ImageStep,
        Act::ListerText,
        Act::ListerBinary,
        Act::ListerHex,
        Act::ListerAnsi,
        Act::ListerAscii,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cue {
    pub archive: bool,
    pub text: bool,
    pub image: bool,
    pub sfv: bool,
    pub marks: usize,
    pub file: bool,
    pub dir: bool,
    pub lister: bool,
    pub image_view: bool,
}

impl Cue {
    pub const fn none() -> Self {
        Self {
            archive: false,
            text: false,
            image: false,
            sfv: false,
            marks: 0,
            file: false,
            dir: false,
            lister: false,
            image_view: false,
        }
    }
}

pub struct Item {
    pub act: Act,
    pub keys: &'static str,
    pub label: &'static str,
    pub group: Group,
}

pub fn catalog() -> &'static [Item] {
    CATALOG
}

pub fn is_text(name: &str) -> bool {
    matches!(
        listing::extension(name).to_ascii_lowercase().as_str(),
        "txt" | "md" | "log" | "rs" | "py" | "js" | "ts" | "go" | "c" | "h"
    )
}

pub fn rank(cue: &Cue) -> Vec<Act> {
    let mut scored: Vec<(i32, usize, Act)> = HINTS
        .iter()
        .enumerate()
        .filter_map(|(index, act)| {
            let score = score(*act, cue);
            (score > 0).then_some((score, index, *act))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().take(6).map(|(_, _, act)| act).collect()
}

pub fn hint_label(act: Act, side: &str) -> String {
    match act {
        Act::Unpack => format!("Alt+F9 Unpack to {side} panel"),
        Act::UnpackHere => "Shift+Alt+F9 Extract here".into(),
        Act::Verify => "Ctrl+Shift+F9 Verify".into(),
        Act::Pack => "Alt+F5 Pack".into(),
        Act::PackHere => "Ctrl+Shift+F5 Pack here".into(),
        Act::View => "F3 View".into(),
        Act::Edit => "F4 Edit".into(),
        Act::Copy => "F5 Copy".into(),
        Act::Move => "F6 Move".into(),
        Act::CopyPaths => "Ctrl+Shift+C Copy path".into(),
        Act::CopyDir => "Alt+Shift+C Copy folder".into(),
        Act::Properties => "Alt+Enter Properties".into(),
        Act::Filter => "Ctrl+S Filter".into(),
        Act::Search => "Ctrl+Shift+S Search".into(),
        other => CATALOG
            .iter()
            .find(|item| item.act == other)
            .map(|item| format!("{} {}", item.keys, item.label))
            .unwrap_or_default(),
    }
}

pub fn applies(act: Act, cue: &Cue) -> bool {
    match act {
        Act::Unpack | Act::UnpackHere => cue.archive,
        Act::Verify => cue.sfv,
        Act::Pack | Act::PackHere | Act::MakeSfv => cue.marks > 0 || cue.file || cue.dir,
        Act::CopyPaths => cue.marks > 0 || cue.file || cue.dir,
        Act::Properties | Act::CopyAs | Act::Rename => cue.file || cue.dir,
        Act::View | Act::Edit => cue.file,
        Act::Queue => false,
        Act::ImageFit | Act::ImageStep => cue.image_view || cue.image,
        Act::ListerText | Act::ListerBinary | Act::ListerHex | Act::ListerAnsi | Act::ListerAscii => cue.lister,
        _ => true,
    }
}

fn score(act: Act, cue: &Cue) -> i32 {
    let multi = cue.marks >= 2;
    let marked = cue.marks >= 1;
    match act {
        Act::Copy if multi => 100,
        Act::Move if multi => 90,
        Act::Pack if multi => 80,
        Act::PackHere if multi => 70,
        Act::Unpack if cue.archive => 60,
        Act::UnpackHere if cue.archive => 58,
        Act::Verify if cue.sfv => 56,
        Act::Pack if marked => 50,
        Act::PackHere if marked => 48,
        Act::View if cue.text || cue.image => 40,
        Act::Edit if cue.text => 36,
        Act::Properties if cue.image => 34,
        Act::CopyPaths if cue.text => 32,
        Act::CopyPaths if cue.dir => 22,
        Act::CopyDir if cue.dir => 20,
        Act::CopyPaths if cue.file => 16,
        Act::Properties if cue.file || cue.dir => 14,
        Act::Filter => 10,
        Act::Search => 8,
        Act::CopyDir => 6,
        _ => 0,
    }
}

const CATALOG: &[Item] = &[
    Item { act: Act::Refresh, keys: "F2", label: "Refresh", group: Group::Files },
    Item { act: Act::View, keys: "F3", label: "View", group: Group::Files },
    Item { act: Act::Edit, keys: "F4", label: "Edit", group: Group::Files },
    Item { act: Act::NewFile, keys: "Shift+F4", label: "New file", group: Group::Files },
    Item { act: Act::Copy, keys: "F5", label: "Copy", group: Group::Files },
    Item { act: Act::Move, keys: "F6", label: "Move", group: Group::Files },
    Item { act: Act::Queue, keys: "F2 in F5/F6", label: "Add to queue", group: Group::Files },
    Item { act: Act::Mkdir, keys: "F7", label: "New folder", group: Group::Files },
    Item { act: Act::Trash, keys: "F8 / Delete", label: "Trash", group: Group::Files },
    Item { act: Act::Delete, keys: "Shift+F8", label: "Delete", group: Group::Files },
    Item { act: Act::CopyAs, keys: "Shift+F5", label: "Copy as", group: Group::Files },
    Item { act: Act::Rename, keys: "Shift+F6", label: "Rename", group: Group::Files },
    Item { act: Act::SelectAll, keys: "Ctrl+A", label: "Select all", group: Group::Files },
    Item { act: Act::Properties, keys: "Alt+Enter", label: "Properties", group: Group::Files },
    Item { act: Act::Settings, keys: "F12", label: "Settings", group: Group::Files },
    Item { act: Act::Pack, keys: "Alt+F5", label: "Pack", group: Group::Archive },
    Item { act: Act::PackHere, keys: "Ctrl+Shift+F5", label: "Pack here", group: Group::Archive },
    Item { act: Act::Unpack, keys: "Alt+F9", label: "Unpack to other panel", group: Group::Archive },
    Item { act: Act::UnpackHere, keys: "Shift+Alt+F9", label: "Extract here", group: Group::Archive },
    Item { act: Act::MakeSfv, keys: "Shift+F9", label: "Create SFV", group: Group::Archive },
    Item { act: Act::Verify, keys: "Ctrl+Shift+F9", label: "Verify SFV", group: Group::Archive },
    Item { act: Act::OtherPane, keys: "Tab", label: "Other pane", group: Group::Navigate },
    Item { act: Act::NextTab, keys: "Ctrl+Tab", label: "Next tab", group: Group::Navigate },
    Item { act: Act::NewTab, keys: "Ctrl+T", label: "New tab", group: Group::Navigate },
    Item { act: Act::CloseTab, keys: "Ctrl+W", label: "Close tab", group: Group::Navigate },
    Item { act: Act::Up, keys: "BackSpace", label: "Up", group: Group::Navigate },
    Item { act: Act::Back, keys: "Alt+Left", label: "Back", group: Group::Navigate },
    Item { act: Act::Forward, keys: "Alt+Right", label: "Forward", group: Group::Navigate },
    Item { act: Act::Mirror, keys: "Ctrl+Left/Right", label: "Mirror folder", group: Group::Navigate },
    Item { act: Act::PlacesLeft, keys: "Alt+F1", label: "Left places", group: Group::Navigate },
    Item { act: Act::PlacesRight, keys: "Alt+F2", label: "Right places", group: Group::Navigate },
    Item { act: Act::Hotlist, keys: "Ctrl+D", label: "Hotlist", group: Group::Navigate },
    Item { act: Act::Open, keys: "Enter", label: "Open", group: Group::Navigate },
    Item { act: Act::Mark, keys: "Insert", label: "Mark", group: Group::Navigate },
    Item { act: Act::MarkStay, keys: "Space", label: "Mark, stay", group: Group::Navigate },
    Item { act: Act::SortName, keys: "Ctrl+F3", label: "Sort by name", group: Group::Navigate },
    Item { act: Act::SortExt, keys: "Ctrl+F4", label: "Sort by extension", group: Group::Navigate },
    Item { act: Act::SortDate, keys: "Ctrl+F5", label: "Sort by date", group: Group::Navigate },
    Item { act: Act::SortSize, keys: "Ctrl+F6", label: "Sort by size", group: Group::Navigate },
    Item { act: Act::SortAttr, keys: "Ctrl+F7", label: "Sort by attributes", group: Group::Navigate },
    Item { act: Act::CopyPaths, keys: "Ctrl+Shift+C", label: "Copy path", group: Group::Clipboard },
    Item { act: Act::CopyDir, keys: "Alt+Shift+C", label: "Copy folder path", group: Group::Clipboard },
    Item { act: Act::Filter, keys: "Ctrl+S", label: "Filter", group: Group::View },
    Item { act: Act::Search, keys: "Ctrl+Shift+S", label: "Search", group: Group::View },
    Item { act: Act::Hidden, keys: "Ctrl+H", label: "Hidden", group: Group::View },
    Item { act: Act::QuickView, keys: "Ctrl+Q", label: "Quick view", group: Group::View },
    Item { act: Act::Terminal, keys: "F9", label: "Terminal in other panel", group: Group::View },
    Item { act: Act::NewShell, keys: "Ctrl+F9", label: "New shell", group: Group::View },
    Item { act: Act::CloseShell, keys: "Ctrl+W", label: "Close shell", group: Group::View },
    Item { act: Act::SwitchShell, keys: "Alt+1…9", label: "Switch shell", group: Group::View },
    Item { act: Act::ExternalView, keys: "Alt+F3", label: "Viewer", group: Group::View },
    Item { act: Act::CloseView, keys: "Escape", label: "Close view", group: Group::View },
    Item { act: Act::ImageFit, keys: "S", label: "Fit or 1:1", group: Group::View },
    Item { act: Act::ImageStep, keys: "Left/Right", label: "Previous or next", group: Group::View },
    Item { act: Act::ListerText, keys: "1", label: "Text", group: Group::View },
    Item { act: Act::ListerBinary, keys: "2", label: "Binary", group: Group::View },
    Item { act: Act::ListerHex, keys: "3", label: "Hex", group: Group::View },
    Item { act: Act::ListerAnsi, keys: "A", label: "Ansi", group: Group::View },
    Item { act: Act::ListerAscii, keys: "S / D", label: "Ascii", group: Group::View },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn cue(build: impl FnOnce(&mut Cue)) -> Cue {
        let mut cue = Cue::none();
        build(&mut cue);
        cue
    }

    #[test]
    fn catalog_lists_each_act_once() {
        for act in Act::ALL {
            assert_eq!(catalog().iter().filter(|item| item.act == act).count(), 1, "{act:?}");
        }
    }

    #[test]
    fn archive_offers_unpack_first() {
        let ranked = rank(&cue(|cue| {
            cue.archive = true;
            cue.file = true;
        }));
        assert_eq!(ranked[0], Act::Unpack);
        assert_eq!(ranked[1], Act::UnpackHere);
        assert!(ranked.contains(&Act::Filter));
        assert!(ranked.len() <= 6);
    }

    #[test]
    fn text_prefers_view_edit_and_path() {
        let ranked = rank(&cue(|cue| {
            cue.text = true;
            cue.file = true;
        }));
        assert_eq!(&ranked[..3], &[Act::View, Act::Edit, Act::CopyPaths]);
        let view = ranked.iter().position(|act| *act == Act::View).unwrap();
        let filter = ranked.iter().position(|act| *act == Act::Filter).unwrap();
        assert!(view < filter);
    }

    #[test]
    fn image_prefers_view_and_properties() {
        let ranked = rank(&cue(|cue| {
            cue.image = true;
            cue.file = true;
        }));
        assert_eq!(ranked[0], Act::View);
        assert_eq!(ranked[1], Act::Properties);
    }

    #[test]
    fn several_marks_outrank_the_archive() {
        let ranked = rank(&cue(|cue| {
            cue.archive = true;
            cue.file = true;
            cue.marks = 2;
        }));
        assert_eq!(&ranked[..4], &[Act::Copy, Act::Move, Act::Pack, Act::PackHere]);
        assert!(ranked.contains(&Act::Unpack));
        assert!(ranked.contains(&Act::UnpackHere));
        assert_eq!(ranked.len(), 6);
    }

    #[test]
    fn sfv_in_the_folder_offers_verify() {
        let ranked = rank(&cue(|cue| {
            cue.sfv = true;
            cue.file = true;
        }));
        assert_eq!(ranked[0], Act::Verify);
    }

    #[test]
    fn plain_file_fills_with_filter_and_search() {
        let ranked = rank(&cue(|cue| cue.file = true));
        assert_eq!(ranked[0], Act::CopyPaths);
        assert!(ranked.contains(&Act::Filter));
        assert!(ranked.contains(&Act::Search));
    }

    #[test]
    fn unpack_is_dimmed_off_an_archive() {
        assert!(!applies(Act::Unpack, &Cue::none()));
        assert!(applies(Act::Filter, &Cue::none()));
    }

    #[test]
    fn text_extensions_match_the_listing() {
        assert!(is_text("notes.txt"));
        assert!(is_text("main.rs"));
        assert!(!is_text("song.mp3"));
        assert!(!is_text("notes.rar"));
    }
}

use crate::listing;

// The status line suggests everyday commands. Special commands stay in the F1 menu.
// Higher is more likely. Several marked files outrank the file type.
const HINTS: &[Act] = &[
    Act::AudioToggle,
    Act::AudioNext,
    Act::AudioPrev,
    Act::AudioMinimize,
    Act::AudioJump,
    Act::AudioExpand,
    Act::AudioClose,
    Act::AudioFocus,
    Act::AudioSend,
    Act::AudioAdd,
    Act::Copy,
    Act::Move,
    Act::Pack,
    Act::PackHere,
    Act::Unpack,
    Act::UnpackHere,
    Act::TestArchive,
    Act::Verify,
    Act::View,
    Act::Edit,
    Act::CopyPaths,
    Act::Properties,
    Act::CopyDir,
    Act::Filter,
    Act::Search,
    Act::Thumbs,
    Act::DiskInfo,
    Act::DiskUsage,
    Act::DiskUsageFollow,
];

/// Commands `rank` keeps, including Quick view in the last slot.
pub const HINT_SLOTS: usize = 9;

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
    NewLink,
    Shred,
    Encode,
    CopyAs,
    Rename,
    SelectAll,
    Properties,
    Settings,
    Pack,
    PackHere,
    Unpack,
    UnpackHere,
    TestArchive,
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
    Path,
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
    Syntax,
    Thumbs,
    Terminal,
    NewShell,
    CloseShell,
    SwitchShell,
    ExternalView,
    CloseView,
    ImageFit,
    ImageStep,
    ImageTurnLeft,
    ImageTurnRight,
    ListerText,
    ListerBinary,
    ListerHex,
    ListerAnsi,
    ListerAscii,
    ListerJson,
    QuickEdit,
    Save,
    Discard,
    LeaveEdit,
    CopyImage,
    AudioToggle,
    AudioSeek,
    AudioVolume,
    AudioMute,
    AudioNext,
    AudioPrev,
    AudioShuffle,
    AudioRepeat,
    AudioMinimize,
    AudioJump,
    AudioExpand,
    AudioClose,
    AudioFocus,
    AudioSend,
    AudioAdd,
    AudioExternal,
    Drive,
    DiskInfo,
    DiskUsage,
    DiskUsageFollow,
    Mount,
    Commands,
}

impl Act {
    pub const ALL: [Act; 96] = [
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
        Act::NewLink,
        Act::Shred,
        Act::Encode,
        Act::CopyAs,
        Act::Rename,
        Act::SelectAll,
        Act::Properties,
        Act::Settings,
        Act::Pack,
        Act::PackHere,
        Act::Unpack,
        Act::UnpackHere,
        Act::TestArchive,
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
        Act::Path,
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
        Act::Syntax,
        Act::Thumbs,
        Act::Terminal,
        Act::NewShell,
        Act::CloseShell,
        Act::SwitchShell,
        Act::ExternalView,
        Act::CloseView,
        Act::ImageFit,
        Act::ImageStep,
        Act::ImageTurnLeft,
        Act::ImageTurnRight,
        Act::ListerText,
        Act::ListerBinary,
        Act::ListerHex,
        Act::ListerAnsi,
        Act::ListerAscii,
        Act::ListerJson,
        Act::QuickEdit,
        Act::Save,
        Act::Discard,
        Act::LeaveEdit,
        Act::CopyImage,
        Act::AudioToggle,
        Act::AudioSeek,
        Act::AudioVolume,
        Act::AudioMute,
        Act::AudioNext,
        Act::AudioPrev,
        Act::AudioShuffle,
        Act::AudioRepeat,
        Act::AudioMinimize,
        Act::AudioJump,
        Act::AudioExpand,
        Act::AudioClose,
        Act::AudioFocus,
        Act::AudioSend,
        Act::AudioAdd,
        Act::AudioExternal,
        Act::Drive,
        Act::DiskInfo,
        Act::DiskUsage,
        Act::DiskUsageFollow,
        Act::Mount,
        Act::Commands,
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
    pub quick_view: bool,
    /// The quick view shows the music player.
    pub audio: bool,
    /// Music plays, in the quick view or in the small player.
    pub player: bool,
    /// The player in play is the small one.
    pub mini: bool,
    /// The cursor is on a music file.
    pub song: bool,
    pub editing: bool,
    // The active panel stands at the root of a drive, as right after picking one.
    pub drive: bool,
    // zip, rar, tar and 7z. Unpack follows `archive`, which includes 7z.
    pub testable: bool,
    /// The active panel shows disk usage bars, or is counting them.
    pub usage: bool,
    /// The active panel counts every folder it opens.
    pub following: bool,
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
            quick_view: false,
            audio: false,
            player: false,
            mini: false,
            song: false,
            editing: false,
            drive: false,
            testable: false,
            usage: false,
            following: false,
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
    // F3 View, F4 Edit, F5 Copy and F6 Move already sit on the key row.
    scored.retain(|(_, _, act)| !matches!(act, Act::View | Act::Edit | Act::Copy | Act::Move));
    // Quick view takes the last slot every time, so it is always in the same place.
    let keep = if cue.editing { HINT_SLOTS } else { HINT_SLOTS - 1 };
    let mut ranked: Vec<Act> = scored.into_iter().take(keep).map(|(_, _, act)| act).collect();
    if !cue.editing {
        ranked.push(Act::QuickView);
    }
    ranked
}

/// PC: Ctl and ⇧. Mac: ⌃ ⌥ ⇧. The CSS font list falls back to Noto Sans CJK for a full-height ⇧.
pub fn format_keys(keys: &str) -> String {
    if cfg!(target_os = "macos") {
        keys.replace("Ctrl", "⌃").replace("Shift", "⇧").replace("Alt", "⌥")
    } else {
        keys.replace("Ctrl", "Ctl").replace("Shift", "⇧")
    }
}

pub fn keys_first(keys: &str) -> bool {
    keys.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|part| part.starts_with('F') && part.as_bytes().get(1).is_some_and(u8::is_ascii_digit))
}

pub fn caption(keys: &str, label: &str) -> String {
    let shown = format_keys(keys);
    if keys_first(keys) {
        format!("{shown} {label}")
    } else {
        format!("{label} {shown}")
    }
}

/// The keys and the label of a command as the status line and the key row spell them.
fn hint_parts(act: Act, side: &str) -> (String, String) {
    let own = |keys: &str, label: &str| (keys.to_string(), label.to_string());
    match act {
        Act::Unpack => own("Alt+F9", &format!("Unpack to {side} panel")),
        Act::UnpackHere => own("Shift+Alt+F9", "Extract here"),
        Act::Verify => own("Ctrl+Shift+F9", "Verify"),
        Act::Pack => own("Alt+F5", "Pack"),
        Act::PackHere => own("Ctrl+Shift+F5", "Pack here"),
        Act::View => own("F3", "View"),
        Act::Edit => own("F4", "Edit"),
        Act::Copy => own("F5", "Copy"),
        Act::Move => own("F6", "Move"),
        Act::CopyPaths => own("Ctrl+Shift+C", "Copy path"),
        Act::Properties => own("Alt+Enter", "Properties"),
        Act::Filter => own("Ctrl+S", "Filter"),
        Act::Search => own("Ctrl+Shift+S", "Search"),
        // The status line is narrow. The list in F1 and the right-click menu spell these out.
        Act::AudioSend => own("Ctrl+P", "Send to player"),
        Act::AudioAdd => own("Ctrl+Shift+P", "Add to playlist"),
        Act::AudioFocus => own("Tab", "Player"),
        // The hint is only on offer while the small player has the keys, where X is the short way.
        Act::AudioClose => own("X", "Close player"),
        other => CATALOG
            .iter()
            .find(|item| item.act == other)
            .map(|item| own(item.keys, item.label))
            .unwrap_or_default(),
    }
}

/// The keys and the label of a command now. The disk usage commands say what pressing them does.
fn hint_parts_for(act: Act, side: &str, cue: &Cue) -> (String, String) {
    let own = |keys: &str, label: &str| (keys.to_string(), label.to_string());
    match act {
        Act::DiskUsage if cue.usage => own("Ctrl+U", "Disk usage off"),
        Act::DiskUsageFollow if cue.following => own("Ctrl+R", "Stop following"),
        Act::DiskUsageFollow => own("Ctrl+U+R", "Follow folders"),
        other => hint_parts(other, side),
    }
}

pub fn hint_label(act: Act, side: &str) -> String {
    let (keys, label) = hint_parts(act, side);
    if label.is_empty() { String::new() } else { caption(&keys, &label) }
}

/// The label of a command now. The disk usage commands say what pressing them does.
pub fn hint_label_for(act: Act, side: &str, cue: &Cue) -> String {
    let (keys, label) = hint_parts_for(act, side, cue);
    if label.is_empty() { String::new() } else { caption(&keys, &label) }
}

/// The commands the status line offers at most. The rest is in F1.
const TIPS: usize = 3;

/// The few commands that matter in the state the cue describes, keys first, for the status line.
pub fn tips(cue: &Cue, side: &str) -> String {
    rank(cue)
        .into_iter()
        .filter(|act| *act != Act::QuickView)
        .take(TIPS)
        .map(|act| {
            let (keys, label) = hint_parts_for(act, side, cue);
            format!("{} {label}", format_keys(&keys))
        })
        .collect::<Vec<_>>()
        .join("  ·  ")
}

/// Other words for what a command does, in English and Swedish, so the F1 search finds it under the word the user
/// happens to think of. Lower case, separated by spaces. The label, the keys and the group are searched as well.
fn synonyms(act: Act) -> &'static str {
    match act {
        Act::Refresh => "reload update rescan uppdatera ladda om läs om",
        Act::View => "show preview look read visa titta förhandsgranska läs",
        Act::Edit => "modify change editor redigera ändra editera",
        Act::NewFile => "create touch blank empty skapa fil ny fil",
        Act::Copy => "duplicate clone kopiera duplicera",
        Act::Move => "relocate transfer cut flytta klipp",
        Act::Queue => "batch later schedule kö köa",
        Act::Mkdir => "folder directory create new dir skapa mapp katalog",
        Act::Trash => "bin recycle remove wastebasket papperskorg radera ta bort släng",
        Act::Delete => "erase remove permanent radera ta bort",
        Act::NewLink => "symlink symbolic shortcut alias ln länk genväg",
        Act::Shred => "wipe secure erase destroy overwrite shredder rensa säker radering förstör",
        Act::Encode => "convert transcode ffmpeg compress video audio konvertera koda omkoda",
        Act::CopyAs => "duplicate save as copy rename kopiera som spara som",
        Act::Rename => "name change namn döp om byt namn",
        Act::SelectAll => "mark everything all select markera alla välj allt",
        Act::Properties => "info details attributes permissions chmod size egenskaper information detaljer rättigheter",
        Act::Settings => "preferences options config configure theme inställningar alternativ konfig",
        Act::Pack | Act::PackHere => "zip compress archive tar gz 7z rar bundle packa arkiv komprimera",
        Act::Unpack | Act::UnpackHere => "unzip extract decompress uncompress open archive packa upp extrahera",
        Act::TestArchive => "check integrity verify validate crc zip kontrollera testa arkiv",
        Act::MakeSfv => "checksum hash md5 sha crc sfv skapa kontrollsumma",
        Act::Verify => "checksum hash md5 sha crc sfv check validate kontrollera verifiera kontrollsumma",
        Act::OtherPane => "switch panel side left right toggle swap byt panel andra panelen växla",
        Act::NextTab => "switch tab cycle nästa flik byt flik",
        Act::NewTab => "open tab ny flik öppna flik",
        Act::CloseTab => "close tab stäng flik",
        Act::Up => "parent folder directory upp överordnad",
        Act::Back => "history previous bakåt tillbaka historik",
        Act::Forward => "history next framåt",
        Act::Mirror => "sync same show folder other panel öppna andra panelen spegla",
        Act::PlacesLeft => "bookmarks sidebar mounts devices places platser vänster",
        Act::PlacesRight => "bookmarks sidebar mounts devices places platser höger",
        Act::Hotlist => "bookmarks favorites favourites directory hot list bokmärken favoriter",
        Act::Path => "address location goto go to type cd adress sökvag gå till",
        Act::Open => "enter run launch execute activate öppna kör",
        Act::Mark | Act::MarkStay => "select tag choose insert markera välj",
        Act::SortName => "order arrange alphabetical sortera namn",
        Act::SortExt => "type extension filetype sortera filtyp ändelse",
        Act::SortDate => "time modified newest oldest datum tid",
        Act::SortSize => "biggest largest smallest storlek",
        Act::SortAttr => "attributes permissions attribut",
        Act::CopyPaths => "clipboard path full name urklipp sökvag kopiera",
        Act::CopyDir => "clipboard current folder directory path urklipp mapp kopiera",
        Act::Filter => "narrow quick find incremental limit filtrera",
        Act::Search => "find locate grep look recursive sök hitta leta",
        Act::Hidden => "dotfiles show invisible dolda gömda dold",
        Act::QuickView => "preview panel peek snabbvisning förhandsvisning",
        Act::Syntax => "highlight colors colour code färger syntaxfärgning",
        Act::Thumbs => "thumbnails pictures images gallery preview miniatyrer bilder",
        Act::Terminal => "shell console command line bash term konsol skal",
        Act::NewShell => "terminal console ny konsol nytt skal",
        Act::CloseShell => "close terminal console stäng konsol skal",
        Act::SwitchShell => "terminal console byt konsol skal",
        Act::ExternalView => "viewer lister open visare",
        Act::CloseView => "leave exit quit back stäng avsluta",
        Act::ImageFit => "zoom scale size actual pixel anpassa zooma",
        Act::ImageStep => "next previous prev gallery slideshow föregående nästa bild",
        Act::ImageTurnLeft => "rotate turn flip counterclockwise vänster rotera vrid vänd",
        Act::ImageTurnRight => "rotate turn flip clockwise höger rotera vrid vänd",
        Act::ListerText => "plain text",
        Act::ListerBinary => "binär bytes",
        Act::ListerHex => "hexadecimal hexdump",
        Act::ListerAnsi => "colors escape terminal",
        Act::ListerAscii => "plain text",
        Act::ListerJson => "format pretty",
        Act::QuickEdit => "edit modify redigera andra",
        Act::Save => "write store spara",
        Act::Discard => "cancel undo revert throw away kasta ångra",
        Act::LeaveEdit => "exit close done lämna avsluta",
        Act::CopyImage => "picture clipboard bild urklipp",
        Act::AudioToggle => "play pause stop music spela paus spela upp musik",
        Act::AudioSeek => "skip forward rewind jump spola hoppa",
        Act::AudioVolume => "sound loud louder quieter ljud volym",
        Act::AudioMute => "silence sound tyst ljud av",
        Act::AudioNext => "skip forward track song nästa låt spår",
        Act::AudioPrev => "back track song föregående låt spår",
        Act::AudioShuffle => "random mix slumpa blanda",
        Act::AudioRepeat => "loop again upprepa slinga",
        Act::AudioMinimize => "small mini shrink minimera liten spelare",
        Act::AudioJump => "player focus go hoppa spelare",
        Act::AudioExpand => "big large maximize full stor spelare",
        Act::AudioClose => "player stop quit stäng spelare",
        Act::AudioFocus => "player switch panel spelare",
        Act::AudioSend => "play replace queue music spela skicka musik",
        Act::AudioAdd => "append playlist queue music lägg till spellista",
        Act::AudioExternal => "player vlc mpv open extern spelare",
        Act::Drive => "disk volume mount usb enhet hårddisk",
        Act::DiskInfo => "free space capacity size storage volume ledigt utrymme diskutrymme kapacitet",
        Act::DiskUsage => "space size big folders biggest du treemap analyzer ncdu bars storage diskanvändning utrymme storlek stora mappar",
        Act::DiskUsageFollow => "recursive descend folders följ mappar",
        Act::Mount => "network share smb nfs sftp ssh ftp remote usb drive nätverk montera ansluta",
        Act::Commands => "help all menu commands keys shortcuts hjälp alla kommandon genvägar",
    }
}

// å, ä and ö count as a and o, so a search typed without them still finds the word.
fn fold(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| match c {
            'å' | 'ä' => 'a',
            'ö' => 'o',
            other => other,
        })
        .collect()
}

/// Whether the F1 search `query` finds `item`. Every word of the query must turn up in the label, the keys, the
/// group or the synonyms.
pub fn matches(query: &str, item: &Item) -> bool {
    let haystack = fold(&format!(
        "{} {} {} {} {}",
        item.label,
        item.keys,
        format_keys(item.keys),
        item.group.name(),
        synonyms(item.act)
    ));
    fold(query).split_whitespace().all(|word| haystack.contains(word))
}

pub fn applies(act: Act, cue: &Cue) -> bool {
    match act {
        Act::Unpack | Act::UnpackHere => cue.archive,
        Act::Verify => cue.sfv,
        Act::Pack | Act::PackHere | Act::MakeSfv | Act::Shred => cue.marks > 0 || cue.file || cue.dir,
        Act::NewLink => cue.file || cue.dir,
        Act::Encode => cue.file,
        Act::TestArchive => cue.testable,
        Act::CopyPaths => cue.marks > 0 || cue.file || cue.dir,
        Act::Properties | Act::CopyAs | Act::Rename => cue.file || cue.dir,
        Act::View | Act::Edit => cue.file,
        Act::Queue => false,
        Act::Filter | Act::Hotlist => !cue.editing,
        Act::Save | Act::Discard | Act::LeaveEdit => cue.editing,
        Act::QuickEdit => cue.quick_view && cue.file && !cue.image && !cue.archive && !cue.editing,
        Act::ImageFit | Act::ImageStep => cue.image_view,
        Act::CopyImage => cue.image_view,
        Act::ImageTurnLeft | Act::ImageTurnRight => cue.quick_view && cue.image && !cue.editing,
        Act::AudioToggle | Act::AudioSeek | Act::AudioVolume | Act::AudioMute => cue.audio,
        Act::AudioNext | Act::AudioPrev | Act::AudioShuffle | Act::AudioRepeat => cue.audio,
        Act::AudioMinimize => cue.player && !cue.mini,
        Act::AudioJump => cue.mini && !cue.audio,
        Act::AudioExpand => cue.mini && cue.audio,
        Act::AudioClose => cue.player,
        Act::AudioFocus => cue.player && !cue.audio,
        Act::AudioSend => cue.song || cue.marks > 0,
        Act::AudioAdd => cue.player && (cue.song || cue.marks > 0),
        Act::AudioExternal => cue.song || cue.player,
        Act::ListerText | Act::ListerBinary | Act::ListerHex => cue.lister,
        Act::ListerAnsi | Act::ListerAscii | Act::ListerJson => cue.lister || cue.quick_view,
        _ => true,
    }
}

fn score(act: Act, cue: &Cue) -> i32 {
    let multi = cue.marks >= 2;
    let marked = cue.marks >= 1;
    match act {
        Act::AudioJump if cue.mini && !cue.audio => 121,
        Act::AudioExpand if cue.mini && cue.audio => 117,
        Act::AudioClose if cue.mini && cue.audio => 116,
        Act::AudioFocus if cue.player && !cue.mini && !cue.audio => 121,
        Act::AudioToggle if cue.audio => 120,
        Act::AudioNext if cue.audio => 119,
        Act::AudioPrev if cue.audio => 118,
        Act::AudioMinimize if cue.audio && !cue.mini => 117,
        Act::AudioSend if cue.song && !cue.audio => 59,
        Act::AudioAdd if cue.song && cue.player && !cue.audio => 58,
        Act::Copy if multi => 100,
        Act::Move if multi => 90,
        Act::Pack if multi => 80,
        Act::PackHere if multi => 70,
        Act::Unpack if cue.archive => 60,
        Act::UnpackHere if cue.archive => 58,
        Act::TestArchive if cue.testable || cue.archive => 54,
        Act::Verify if cue.sfv => 56,
        Act::Pack if marked => 50,
        Act::PackHere if marked => 48,
        Act::View if cue.text || cue.image => 40,
        Act::Edit if cue.text => 36,
        Act::Properties if cue.image => 34,
        Act::Thumbs if cue.image => 28,
        Act::CopyPaths if cue.text => 32,
        Act::CopyPaths if cue.dir => 22,
        Act::CopyDir if cue.dir => 20,
        Act::CopyPaths if cue.file => 16,
        Act::Properties if cue.file || cue.dir => 14,
        Act::DiskInfo if cue.drive => 30,
        // In the mode the next steps lead the row: follow a big folder down, or switch it off.
        Act::DiskUsageFollow if cue.usage => 66,
        Act::DiskUsage if cue.usage => 62,
        Act::DiskUsage if cue.drive || cue.dir => 12,
        Act::Filter => 10,
        Act::Search => 8,
        Act::Thumbs => 7,
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
    Item { act: Act::Delete, keys: "Ctrl+Delete / Shift+F8", label: "Delete", group: Group::Files },
    Item { act: Act::NewLink, keys: "Shift+F7", label: "New link", group: Group::Files },
    Item { act: Act::Shred, keys: "Ctrl+Shift+Delete", label: "Secure delete", group: Group::Files },
    Item { act: Act::Encode, keys: "Ctrl+Shift+F7", label: "Encode", group: Group::Files },
    Item { act: Act::CopyAs, keys: "Shift+F5", label: "Copy as", group: Group::Files },
    Item { act: Act::Rename, keys: "Shift+F6", label: "Rename", group: Group::Files },
    Item { act: Act::SelectAll, keys: "Ctrl+A", label: "Select all", group: Group::Files },
    Item { act: Act::Properties, keys: "Alt+Enter", label: "Properties", group: Group::Files },
    Item { act: Act::Settings, keys: "F12", label: "Settings", group: Group::Files },
    Item { act: Act::Commands, keys: "F1", label: "Commands", group: Group::Files },
    Item { act: Act::Pack, keys: "Alt+F5", label: "Pack", group: Group::Archive },
    Item { act: Act::PackHere, keys: "Ctrl+Shift+F5", label: "Pack here", group: Group::Archive },
    Item { act: Act::Unpack, keys: "Alt+F9", label: "Unpack to other panel", group: Group::Archive },
    Item { act: Act::UnpackHere, keys: "Shift+Alt+F9", label: "Extract here", group: Group::Archive },
    Item { act: Act::TestArchive, keys: "Ctrl+Shift+F2", label: "Test archive", group: Group::Archive },
    Item { act: Act::MakeSfv, keys: "Shift+F9", label: "Create checksum", group: Group::Archive },
    Item { act: Act::Verify, keys: "Ctrl+Shift+F9", label: "Verify checksum", group: Group::Archive },
    Item { act: Act::OtherPane, keys: "Tab", label: "Other pane", group: Group::Navigate },
    Item { act: Act::NextTab, keys: "Ctrl+Tab", label: "Next tab", group: Group::Navigate },
    Item { act: Act::NewTab, keys: "Ctrl+T", label: "New tab", group: Group::Navigate },
    Item { act: Act::CloseTab, keys: "Ctrl+W", label: "Close tab", group: Group::Navigate },
    Item { act: Act::Up, keys: "BackSpace", label: "Up", group: Group::Navigate },
    Item { act: Act::Back, keys: "Alt+Left", label: "Back", group: Group::Navigate },
    Item { act: Act::Forward, keys: "Alt+Right", label: "Forward", group: Group::Navigate },
    Item { act: Act::Mirror, keys: "Ctrl+Left/Right", label: "Open in other panel", group: Group::Navigate },
    Item { act: Act::PlacesLeft, keys: "Alt+F1", label: "Left places", group: Group::Navigate },
    Item { act: Act::PlacesRight, keys: "Alt+F2", label: "Right places", group: Group::Navigate },
    Item { act: Act::Drive, keys: "Ctrl+1…9", label: "Drive", group: Group::Navigate },
    Item { act: Act::DiskInfo, keys: "Ctrl+I", label: "Disk info", group: Group::Navigate },
    Item { act: Act::DiskUsage, keys: "Ctrl+U", label: "Disk usage", group: Group::Navigate },
    Item { act: Act::DiskUsageFollow, keys: "Ctrl+U+R", label: "Disk usage, follow into folders", group: Group::Navigate },
    Item { act: Act::Mount, keys: "Ctrl+Shift+M", label: "Mount", group: Group::Navigate },
    Item { act: Act::Hotlist, keys: "Ctrl+D", label: "Hotlist", group: Group::Navigate },
    Item { act: Act::Path, keys: "Ctrl+L", label: "Path", group: Group::Navigate },
    Item { act: Act::Open, keys: "Enter", label: "Open", group: Group::Navigate },
    Item { act: Act::Mark, keys: "Insert", label: "Mark", group: Group::Navigate },
    Item { act: Act::MarkStay, keys: "Space", label: "Mark, stay", group: Group::Navigate },
    Item { act: Act::SortName, keys: "Ctrl+F3", label: "Sort by name", group: Group::Navigate },
    Item { act: Act::SortExt, keys: "Ctrl+F4", label: "Sort by extension", group: Group::Navigate },
    Item { act: Act::SortDate, keys: "Ctrl+F5", label: "Sort by date", group: Group::Navigate },
    Item { act: Act::SortSize, keys: "Ctrl+F6", label: "Sort by size", group: Group::Navigate },
    Item { act: Act::SortAttr, keys: "Ctrl+F7", label: "Sort by attributes", group: Group::Navigate },
    Item { act: Act::CopyPaths, keys: "Ctrl+Shift+C", label: "Copy path", group: Group::Clipboard },
    Item { act: Act::CopyImage, keys: "Ctrl+C", label: "Copy image", group: Group::Clipboard },
    Item { act: Act::AudioToggle, keys: "Space", label: "Play or pause", group: Group::View },
    Item { act: Act::AudioSeek, keys: "Ctrl+Left/Right", label: "Seek 5 s, Shift 30 s", group: Group::View },
    Item { act: Act::AudioVolume, keys: "+/-", label: "Volume", group: Group::View },
    Item { act: Act::AudioMute, keys: "M", label: "Mute", group: Group::View },
    Item { act: Act::AudioNext, keys: "N", label: "Next track", group: Group::View },
    Item { act: Act::AudioPrev, keys: "P", label: "Previous track", group: Group::View },
    Item { act: Act::AudioShuffle, keys: "S", label: "Shuffle", group: Group::View },
    Item { act: Act::AudioRepeat, keys: "R", label: "Repeat: off, all, one", group: Group::View },
    Item { act: Act::AudioMinimize, keys: "Ctrl+M", label: "Minimize player", group: Group::View },
    Item { act: Act::AudioJump, keys: "Ctrl+M", label: "Jump to player", group: Group::View },
    Item { act: Act::AudioExpand, keys: "Ctrl+M", label: "Big player", group: Group::View },
    Item { act: Act::AudioClose, keys: "Ctrl+Shift+Q", label: "Close player", group: Group::View },
    Item { act: Act::AudioFocus, keys: "Tab", label: "Panel or player", group: Group::View },
    Item { act: Act::AudioSend, keys: "Ctrl+P", label: "Send and replace music in player", group: Group::View },
    Item { act: Act::AudioAdd, keys: "Ctrl+Shift+P", label: "Add to current playlist", group: Group::View },
    Item { act: Act::AudioExternal, keys: "Ctrl+Enter", label: "Open In External Player", group: Group::View },
    Item { act: Act::CopyDir, keys: "Alt+Shift+C", label: "Copy dir path", group: Group::Clipboard },
    Item { act: Act::Filter, keys: "Ctrl+S", label: "Filter", group: Group::View },
    Item { act: Act::Search, keys: "Ctrl+Shift+S", label: "Search", group: Group::View },
    Item { act: Act::Hidden, keys: "Ctrl+H", label: "Hidden", group: Group::View },
    Item { act: Act::QuickView, keys: "Ctrl+Q", label: "Quick view", group: Group::View },
    Item { act: Act::QuickEdit, keys: "Ctrl+E", label: "Quick edit", group: Group::View },
    Item { act: Act::Save, keys: "Ctrl+S", label: "Save", group: Group::View },
    Item { act: Act::Discard, keys: "Ctrl+D", label: "Discard", group: Group::View },
    Item { act: Act::LeaveEdit, keys: "Escape", label: "Leave edit", group: Group::View },
    Item { act: Act::Syntax, keys: "Ctrl+Y", label: "Syntax", group: Group::View },
    Item { act: Act::Thumbs, keys: "Ctrl+Shift+F1", label: "Thumbnails", group: Group::View },
    Item { act: Act::Terminal, keys: "F9", label: "Terminal in other panel", group: Group::View },
    Item { act: Act::NewShell, keys: "Ctrl+F9", label: "New shell", group: Group::View },
    Item { act: Act::CloseShell, keys: "Ctrl+W", label: "Close shell", group: Group::View },
    Item { act: Act::SwitchShell, keys: "Alt+1…9", label: "Switch shell", group: Group::View },
    Item { act: Act::ExternalView, keys: "Alt+F3", label: "Viewer", group: Group::View },
    Item { act: Act::CloseView, keys: "Escape", label: "Close view", group: Group::View },
    Item { act: Act::ImageFit, keys: "S", label: "Fit or 1:1", group: Group::View },
    Item { act: Act::ImageStep, keys: "Left/Right", label: "Previous or next", group: Group::View },
    Item { act: Act::ImageTurnLeft, keys: "L", label: "Rotate image left", group: Group::View },
    Item { act: Act::ImageTurnRight, keys: "R", label: "Rotate image right", group: Group::View },
    Item { act: Act::ListerText, keys: "1", label: "Text", group: Group::View },
    Item { act: Act::ListerBinary, keys: "2", label: "Binary", group: Group::View },
    Item { act: Act::ListerHex, keys: "3", label: "Hex", group: Group::View },
    Item { act: Act::ListerAnsi, keys: "A", label: "Ansi", group: Group::View },
    Item { act: Act::ListerAscii, keys: "S / D", label: "Ascii", group: Group::View },
    Item { act: Act::ListerJson, keys: "Shift+J", label: "JSON", group: Group::View },
];

/// What a right-click on one row can do. Marks are left alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowAct {
    Open,
    QuickView,
    QuickEdit,
    Copy,
    Move,
    Rename,
    Pack,
    Unpack,
    Trash,
    CopyPath,
    Paste,
    Properties,
    PlayerSend,
    PlayerAdd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowItem {
    pub act: RowAct,
    pub keys: &'static str,
    pub label: &'static str,
    pub group: u8,
}

fn row_item(act: RowAct, keys: &'static str, label: &'static str, group: u8) -> RowItem {
    RowItem { act, keys, label, group }
}

/// `parent` is the `..` row. A directory is not an archive and not an image. `music` is a song, or a marked
/// row whose marks may hold songs, which brings the two player items.
pub fn paste_row() -> RowItem {
    row_item(RowAct::Paste, "Ctrl+V", "Paste", 1)
}

pub fn row_menu(parent: bool, dir: bool, archive: bool, image: bool, music: bool) -> Vec<RowItem> {
    if parent {
        return vec![row_item(RowAct::Open, "Enter", "Open", 0), paste_row()];
    }
    let mut rows = vec![row_item(RowAct::Open, "Enter", "Open", 0)];
    if !dir {
        rows.push(row_item(RowAct::QuickView, "Ctrl+Q", "Quick view", 0));
        if !image && !archive {
            rows.push(row_item(RowAct::QuickEdit, "Ctrl+E", "Quick edit", 0));
        }
    }
    if music {
        rows.push(row_item(RowAct::PlayerSend, "Ctrl+P", "Send and replace music in player", 5));
        rows.push(row_item(RowAct::PlayerAdd, "Ctrl+Shift+P", "Add to current playlist", 5));
    }
    rows.push(row_item(RowAct::Copy, "F5", "Copy", 1));
    rows.push(row_item(RowAct::Move, "F6", "Move", 1));
    rows.push(paste_row());
    rows.push(row_item(RowAct::Rename, "Shift+F6", "Rename", 2));
    rows.push(row_item(RowAct::Pack, "Alt+F5", "Pack", 2));
    if archive && !dir {
        rows.push(row_item(RowAct::Unpack, "Alt+F9", "Unpack", 2));
    }
    rows.push(row_item(RowAct::Trash, "Delete", "Delete", 3));
    rows.push(row_item(RowAct::CopyPath, "Ctrl+Shift+C", "Copy path", 4));
    rows.push(row_item(RowAct::Properties, "Alt+Enter", "Properties", 4));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cue(build: impl FnOnce(&mut Cue)) -> Cue {
        let mut cue = Cue::none();
        build(&mut cue);
        cue
    }

    #[test]
    fn disk_usage_mode_leads_the_row_with_its_next_steps() {
        let mut cue = Cue::none();
        cue.dir = true;
        assert!(rank(&cue).contains(&Act::DiskUsage));
        assert!(!rank(&cue).contains(&Act::DiskUsageFollow));
        cue.usage = true;
        let ranked = rank(&cue);
        assert_eq!(&ranked[..2], &[Act::DiskUsageFollow, Act::DiskUsage]);
        assert!(hint_label_for(Act::DiskUsageFollow, "right", &cue).contains("Follow folders"));
        assert!(hint_label_for(Act::DiskUsage, "right", &cue).contains("Disk usage off"));
        cue.following = true;
        assert!(hint_label_for(Act::DiskUsageFollow, "right", &cue).contains("Stop following"));
        // Several marked files still come first.
        cue.marks = 3;
        assert_eq!(rank(&cue)[0], Act::Pack);
    }

    #[test]
    fn tips_put_the_next_steps_first_and_stay_short() {
        let mut cue = Cue::none();
        cue.dir = true;
        let tip = tips(&cue, "right");
        assert!(tip.contains("Copy path") && !tip.contains("Quick view"), "{tip}");
        cue.usage = true;
        let tip = tips(&cue, "right");
        assert!(tip.starts_with("Ctl+U+R Follow folders") || tip.starts_with("⌃U+R Follow folders"), "{tip}");
        assert!(tip.contains("Disk usage off"), "{tip}");
        assert!(tips(&cue, "right").split("  ·  ").count() <= TIPS);
    }

    #[test]
    fn the_command_search_knows_other_words_in_english_and_swedish() {
        let find = |query: &str, act: Act| matches(query, catalog().iter().find(|item| item.act == act).unwrap());
        assert!(find("zip", Act::Pack) && find("zip", Act::Unpack));
        assert!(find("flytta", Act::Move) && find("move", Act::Move));
        assert!(find("unzip", Act::Unpack) && !find("unzip", Act::Pack));
        assert!(find("sok", Act::Search) && find("sök", Act::Search));
        assert!(find("kopiera", Act::Copy));
        // Every word has to match, and an unrelated word finds nothing.
        assert!(find("flytta fil", Act::Move));
        assert!(!find("zip flytta", Act::Pack));
        assert!(!find("flytta", Act::Pack));
    }

    #[test]
    fn catalog_lists_each_act_once() {
        for act in Act::ALL {
            assert_eq!(catalog().iter().filter(|item| item.act == act).count(), 1, "{act:?}");
        }
    }

    #[test]
    fn the_music_player_puts_play_and_pause_first() {
        let ranked = rank(&cue(|cue| {
            cue.audio = true;
            cue.file = true;
            cue.quick_view = true;
        }));
        assert_eq!(ranked[0], Act::AudioToggle);
        assert_eq!(ranked.last(), Some(&Act::QuickView));
        assert!(hint_label(Act::AudioToggle, "right").contains("Play or pause"));
        // Off the player, none of its keys are on offer.
        assert!(!rank(&cue(|cue| cue.file = true)).contains(&Act::AudioToggle));
        for act in [Act::AudioToggle, Act::AudioSeek, Act::AudioVolume, Act::AudioMute] {
            assert!(applies(act, &cue(|cue| cue.audio = true)), "{act:?}");
            assert!(!applies(act, &cue(|cue| cue.file = true)), "{act:?}");
        }
    }

    #[test]
    fn a_player_with_the_keys_offers_next_and_previous_and_one_without_offers_a_way_in() {
        // The big player has the keyboard.
        let big = cue(|cue| {
            cue.audio = true;
            cue.player = true;
            cue.file = true;
            cue.quick_view = true;
        });
        let ranked = rank(&big);
        assert_eq!(&ranked[..4], &[Act::AudioToggle, Act::AudioNext, Act::AudioPrev, Act::AudioMinimize]);
        assert!(!ranked.contains(&Act::AudioJump) && !ranked.contains(&Act::AudioFocus));
        for act in [Act::AudioNext, Act::AudioPrev, Act::AudioShuffle, Act::AudioRepeat, Act::AudioMinimize, Act::AudioExternal] {
            assert!(applies(act, &big), "{act:?}");
        }
        // The big player is up, but the list has the keys: Tab is the way in, and it comes first.
        let list_has_keys = cue(|cue| {
            cue.player = true;
            cue.file = true;
            cue.quick_view = true;
        });
        let ranked = rank(&list_has_keys);
        assert_eq!(ranked[0], Act::AudioFocus);
        assert!(!ranked.contains(&Act::AudioToggle));
        assert!(applies(Act::AudioMinimize, &list_has_keys));
        // The small player: Ctrl+M jumps to it. Next and previous wait until it has the keys.
        let small = cue(|cue| {
            cue.player = true;
            cue.mini = true;
            cue.file = true;
        });
        let ranked = rank(&small);
        assert_eq!(ranked[0], Act::AudioJump);
        assert!(!ranked.contains(&Act::AudioNext) && !ranked.contains(&Act::AudioFocus));
        assert!(applies(Act::AudioExternal, &small));
        assert!(!applies(Act::AudioMinimize, &small));
        assert!(!applies(Act::AudioJump, &list_has_keys));
        // The small player has the keys: Ctrl+M makes it big again, so that is on offer instead of the jump.
        let small_keys = cue(|cue| {
            cue.player = true;
            cue.mini = true;
            cue.audio = true;
            cue.file = true;
        });
        let ranked = rank(&small_keys);
        assert_eq!(&ranked[..4], &[Act::AudioToggle, Act::AudioNext, Act::AudioPrev, Act::AudioExpand]);
        assert!(!applies(Act::AudioJump, &small_keys) && !applies(Act::AudioMinimize, &small_keys));
        assert!(!applies(Act::AudioExpand, &small) && !applies(Act::AudioExpand, &big));
        // Closing is on offer whenever a player is up, with Ctrl+Shift+Q. The hint bar only shows it to the small player with the keys.
        assert!(applies(Act::AudioClose, &small_keys) && rank(&small_keys).contains(&Act::AudioClose));
        assert!(applies(Act::AudioClose, &small) && applies(Act::AudioClose, &big) && applies(Act::AudioClose, &list_has_keys));
        assert!(!rank(&big).contains(&Act::AudioClose) && !rank(&small).contains(&Act::AudioClose));
        assert!(hint_label(Act::AudioClose, "right").contains("Close player"));
        // Nothing plays: none of it is on offer.
        for act in [Act::AudioNext, Act::AudioJump, Act::AudioMinimize, Act::AudioClose, Act::AudioFocus, Act::AudioAdd] {
            assert!(!applies(act, &cue(|cue| cue.file = true)), "{act:?}");
        }
    }

    #[test]
    fn songs_can_be_sent_to_the_player_and_added_once_one_plays() {
        let on_song = cue(|cue| {
            cue.file = true;
            cue.song = true;
        });
        assert!(applies(Act::AudioSend, &on_song));
        // Adding needs a playlist to add to.
        assert!(!applies(Act::AudioAdd, &on_song));
        assert!(rank(&on_song).contains(&Act::AudioSend));
        let playing = cue(|cue| {
            cue.file = true;
            cue.song = true;
            cue.player = true;
        });
        assert!(applies(Act::AudioAdd, &playing));
        assert!(rank(&playing).contains(&Act::AudioAdd));
        // Marks count too, and a text file with nothing marked is not a song.
        assert!(applies(Act::AudioSend, &cue(|cue| cue.marks = 2)));
        assert!(!applies(Act::AudioSend, &cue(|cue| cue.file = true)));
        assert!(hint_label(Act::AudioSend, "right").contains("Send to player"));
        assert!(catalog().iter().any(|item| item.act == Act::AudioSend && item.label == "Send and replace music in player"));
    }

    #[test]
    fn drive_root_offers_disk_info() {
        let ranked = rank(&cue(|cue| {
            cue.drive = true;
            cue.dir = true;
        }));
        assert_eq!(ranked[0], Act::DiskInfo);
        assert!(hint_label(Act::DiskInfo, "right").contains("Disk info"));
        assert!(!rank(&Cue::none()).contains(&Act::DiskInfo));
    }

    #[test]
    fn archive_offers_unpack_first() {
        let ranked = rank(&cue(|cue| {
            cue.archive = true;
            cue.file = true;
        }));
        assert_eq!(ranked[0], Act::Unpack);
        assert_eq!(ranked[1], Act::UnpackHere);
        assert!(ranked.contains(&Act::TestArchive));
        assert_eq!(ranked.len(), HINT_SLOTS);
    }

    #[test]
    fn text_skips_commands_already_on_the_key_row() {
        let ranked = rank(&cue(|cue| {
            cue.text = true;
            cue.file = true;
        }));
        assert_eq!(ranked[0], Act::CopyPaths);
        assert!(ranked.contains(&Act::Properties));
        assert!(ranked.contains(&Act::Filter));
        assert!(!ranked.contains(&Act::View));
        assert!(!ranked.contains(&Act::Edit));
    }

    #[test]
    fn image_offers_properties_not_view() {
        let ranked = rank(&cue(|cue| {
            cue.image = true;
            cue.file = true;
        }));
        assert_eq!(ranked[0], Act::Properties);
        assert!(ranked.contains(&Act::Thumbs));
        assert!(!ranked.contains(&Act::View));
    }

    #[test]
    fn browsing_offers_thumbnails() {
        let ranked = rank(&cue(|cue| cue.file = true));
        assert!(ranked.contains(&Act::Thumbs));
        assert!(hint_label(Act::Thumbs, "right").contains("Thumbnails"));
    }

    #[test]
    fn several_marks_outrank_the_archive() {
        let ranked = rank(&cue(|cue| {
            cue.archive = true;
            cue.file = true;
            cue.marks = 2;
        }));
        assert_eq!(&ranked[..4], &[Act::Pack, Act::PackHere, Act::Unpack, Act::UnpackHere]);
        assert!(ranked.contains(&Act::TestArchive));
        assert_eq!(ranked.len(), HINT_SLOTS);
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
    fn special_commands_stay_off_the_context_row() {
        let file = rank(&cue(|cue| cue.file = true));
        let folder = rank(&cue(|cue| cue.dir = true));
        for act in [Act::NewLink, Act::MakeSfv, Act::Shred, Act::Encode] {
            assert!(!file.contains(&act), "{act:?}");
            assert!(!folder.contains(&act), "{act:?}");
        }
        assert!(!file.contains(&Act::TestArchive));

        assert!(applies(Act::TestArchive, &cue(|cue| cue.testable = true)));
        assert!(!applies(Act::TestArchive, &cue(|cue| cue.file = true)));
    }

    #[test]
    fn plain_file_fills_with_filter_and_search() {
        let ranked = rank(&cue(|cue| cue.file = true));
        assert_eq!(ranked[0], Act::CopyPaths);
        assert!(ranked.contains(&Act::Filter));
        assert!(ranked.contains(&Act::Search));
    }

    #[test]
    fn quick_view_is_always_last() {
        for build in [Cue::none(), cue(|c| c.file = true), cue(|c| c.marks = 3), cue(|c| c.drive = true)] {
            assert_eq!(rank(&build).last(), Some(&Act::QuickView));
        }
        assert!(!rank(&cue(|c| c.editing = true)).contains(&Act::QuickView));
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

    #[test]
    fn caption_puts_keys_after_label_except_fkeys() {
        assert_eq!(caption("F3", "View"), "F3 View");
        assert_eq!(caption("Alt+F5", "Pack"), format!("{} Pack", format_keys("Alt+F5")));
        assert_eq!(caption("Alt+Enter", "Properties"), format!("Properties {}", format_keys("Alt+Enter")));
        assert_eq!(caption("Ctrl+S", "Filter"), format!("Filter {}", format_keys("Ctrl+S")));
    }

    #[test]
    fn format_keys_shortens_modifiers() {
        let shown = format_keys("Ctrl+Shift+C");
        assert!(!shown.contains("Ctrl"));
        assert!(shown.contains('⇧'));
        if cfg!(target_os = "macos") {
            assert!(shown.contains('⌃'));
        } else {
            assert!(shown.contains("Ctl"));
        }
    }

    fn acts(rows: &[RowItem]) -> Vec<RowAct> {
        rows.iter().map(|row| row.act).collect()
    }

    #[test]
    fn parent_row_only_opens() {
        assert_eq!(acts(&row_menu(true, true, false, false, false)), vec![RowAct::Open, RowAct::Paste]);
    }

    #[test]
    fn directory_row_skips_view_and_unpack() {
        let rows = row_menu(false, true, false, false, false);
        assert_eq!(
            acts(&rows),
            vec![
                RowAct::Open,
                RowAct::Copy,
                RowAct::Move,
                RowAct::Paste,
                RowAct::Rename,
                RowAct::Pack,
                RowAct::Trash,
                RowAct::CopyPath,
                RowAct::Properties,
            ]
        );
        assert_eq!(rows[0].group, 0);
        assert_eq!(rows[1].group, 1);
        assert_eq!(rows[4].group, 2);
        assert_eq!(rows[6].group, 3);
        assert_eq!(rows[7].group, 4);
    }

    #[test]
    fn file_row_offers_quick_view_and_edit() {
        let rows = row_menu(false, false, false, false, false);
        assert!(acts(&rows).contains(&RowAct::QuickView));
        assert!(acts(&rows).contains(&RowAct::QuickEdit));
        assert!(!acts(&rows).contains(&RowAct::Unpack));
    }

    #[test]
    fn image_row_skips_quick_edit() {
        let rows = row_menu(false, false, false, true, false);
        assert!(acts(&rows).contains(&RowAct::QuickView));
        assert!(!acts(&rows).contains(&RowAct::QuickEdit));
    }

    #[test]
    fn a_song_row_offers_the_player_items_and_a_plain_row_does_not() {
        let rows = row_menu(false, false, false, false, true);
        let send = rows.iter().position(|row| row.act == RowAct::PlayerSend).expect("send item");
        let add = rows.iter().position(|row| row.act == RowAct::PlayerAdd).expect("add item");
        assert_eq!(add, send + 1);
        assert_eq!(rows[send].label, "Send and replace music in player");
        assert_eq!(rows[add].label, "Add to current playlist");
        assert_eq!(rows[send].keys, "Ctrl+P");
        assert_eq!(rows[add].keys, "Ctrl+Shift+P");
        // They sit in a group of their own, so a separator sets them apart.
        assert!(rows[send].group != rows[send - 1].group && rows[send].group != rows[send + 2].group);
        for plain in [row_menu(false, false, false, false, false), row_menu(true, true, false, false, false), row_menu(false, true, false, false, false)] {
            assert!(!acts(&plain).contains(&RowAct::PlayerSend) && !acts(&plain).contains(&RowAct::PlayerAdd));
        }
    }

    #[test]
    fn archive_row_offers_unpack() {
        let rows = row_menu(false, false, true, false, false);
        assert!(acts(&rows).contains(&RowAct::Unpack));
        assert_eq!(rows.iter().find(|row| row.act == RowAct::Unpack).unwrap().keys, "Alt+F9");
    }
}

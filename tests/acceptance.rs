use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rust_commander::config::{self, Bookmark, Config};
use rust_commander::entry::{Entry, DIR, HIDDEN};
use rust_commander::external;
use rust_commander::imageview;
use rust_commander::lister::{self, Charset, PreviewBody, ViewMode};
use rust_commander::listing::{self, SortKey};
use rust_commander::ops::{self, Choice, Kind};
use rust_commander::search::{self, Event, Options};
use rust_commander::state::{AppState, PaneId};
use rust_commander::theme;

fn main() {
    if let Some(name) = case_arg() {
        let fixture = PathBuf::from(std::env::var("FIXTURE").expect("FIXTURE"));
        if let Err(err) = rust_commander::sandbox::restrict(&fixture) {
            eprintln!("landlock failed: {err}");
            std::process::exit(2);
        }
        if let Err(err) = dispatch(&name, &fixture) {
            eprintln!("{name}: {err}");
            std::process::exit(1);
        }
        return;
    }

    if std::env::var_os("RUST_COMMANDER_REQUIRE_SANDBOX").is_some() {
        if let Ok(real) = std::env::var("RUST_COMMANDER_REAL_HOME") {
            if !real.is_empty() && Path::new(&real).exists() {
                eprintln!("refusing to run: {real} is visible");
                std::process::exit(1);
            }
        }
    }

    let mut failed = 0;
    for (name, test) in MEMORY {
        if let Err(err) = test() {
            eprintln!("FAIL {name}: {err}");
            failed += 1;
        } else {
            println!("ok {name}");
        }
    }
    for name in FS {
        if !spawn(name) {
            failed += 1;
        }
    }
    if failed > 0 {
        eprintln!("{failed} failed");
        std::process::exit(1);
    }
    println!("acceptance ok");
}

fn case_arg() -> Option<String> {
    let mut args = std::env::args().skip(1);
    match (args.next().as_deref(), args.next()) {
        (Some("--case"), Some(name)) => Some(name),
        _ => None,
    }
}

fn spawn(name: &str) -> bool {
    let dir = std::env::temp_dir().join(format!("rust-commander-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    if fs::create_dir_all(&dir).is_err() {
        eprintln!("FAIL {name}: fixture");
        return false;
    }
    let ok = Command::new(std::env::current_exe().unwrap())
        .arg("--case")
        .arg(name)
        .env("FIXTURE", &dir)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let _ = fs::remove_dir_all(&dir);
    if ok {
        println!("ok {name}");
    } else {
        eprintln!("FAIL {name}");
    }
    ok
}

const MEMORY: &[(&str, fn() -> Result<(), String>)] = &[
    ("visible_keeps_raw_order", visible_keeps_raw_order),
    ("hidden_files_follow_toggle", hidden_files_follow_toggle),
    ("sort_keys_order_entries", sort_keys_order_entries),
    ("pane_sort_is_independent", pane_sort_is_independent),
    ("columns_ext_date_attr", columns_ext_date_attr),
    ("file_icons_by_kind", file_icons_by_kind),
    ("column_widths_follow_text", column_widths_follow_text),
    ("filter_keeps_substring", filter_keeps_substring),
    ("insert_mark_is_used", insert_mark_is_used),
    ("space_marks_without_moving_cursor", space_marks_without_moving_cursor),
    ("colors_toml_overrides_palette", colors_toml_overrides_palette),
    ("list_background_color_and_image", list_background_color_and_image),
    ("enter_opens_directory", enter_opens_directory),
    ("up_goes_to_parent", up_goes_to_parent),
    ("refresh_bumps_token", refresh_bumps_token),
    ("empty_mark_uses_cursor", empty_mark_uses_cursor),
    ("history_back", history_back),
    ("new_tab_has_own_listing", new_tab_has_own_listing),
    ("theme_fallback_without_file", theme_fallback_without_file),
    ("places_include_home_and_root", places_include_home_and_root),
    ("mounts_lists_disks", mounts_lists_disks),
    ("external_argv", external_argv),
    ("ctrl_arrow_shows_active_dir_on_other_pane", ctrl_arrow_shows_active_dir_on_other_pane),
    ("terminal_argv_without_starting", terminal_argv_without_starting),
    ("quick_view_ignores_stale_generation", quick_view_ignores_stale_generation),
    ("image_list_skips_other_files", image_list_skips_other_files),
    ("image_step_stops_at_end", image_step_stops_at_end),
    ("image_decode_png_and_scales", image_decode_png_and_scales),
    ("lister_ansi_and_ascii", lister_ansi_and_ascii),
    ("lister_pretty_json", lister_pretty_json),
    ("hotlist_accel_submenu", hotlist_accel_submenu),
];

const FS: &[&str] = &[
    "landlock_blocks_outside_write",
    "list_dir_reads_names",
    "search_nested_names",
    "search_cancelled",
    "f5_copies_tree_to_other_pane",
    "shift_f5_copies_with_new_name",
    "f6_moves_file",
    "shift_f6_renames",
    "f7_makes_directory",
    "shift_f4_creates_file",
    "f8_calls_trash",
    "shift_f8_deletes",
    "conflict_overwrite_rest",
    "cancel_removes_partial",
    "symlink_copied_as_link",
    "copy_into_itself_aborts",
    "lister_text_hex_binary_window",
    "lister_large_file_reads_window_only",
    "bookmark_roundtrip_config",
];

fn dispatch(name: &str, dir: &Path) -> Result<(), String> {
    match name {
        "landlock_blocks_outside_write" => landlock_blocks_outside_write(dir),
        "list_dir_reads_names" => list_dir_reads_names(dir),
        "search_nested_names" => search_nested_names(dir),
        "search_cancelled" => search_cancelled(dir),
        "f5_copies_tree_to_other_pane" => f5_copies_tree_to_other_pane(dir),
        "shift_f5_copies_with_new_name" => shift_f5_copies_with_new_name(dir),
        "f6_moves_file" => f6_moves_file(dir),
        "shift_f6_renames" => shift_f6_renames(dir),
        "f7_makes_directory" => f7_makes_directory(dir),
        "shift_f4_creates_file" => shift_f4_creates_file(dir),
        "f8_calls_trash" => f8_calls_trash(dir),
        "shift_f8_deletes" => shift_f8_deletes(dir),
        "conflict_overwrite_rest" => conflict_overwrite_rest(dir),
        "cancel_removes_partial" => cancel_removes_partial(dir),
        "symlink_copied_as_link" => symlink_copied_as_link(dir),
        "copy_into_itself_aborts" => copy_into_itself_aborts(dir),
        "lister_text_hex_binary_window" => lister_text_hex_binary_window(dir),
        "lister_large_file_reads_window_only" => lister_large_file_reads_window_only(dir),
        "bookmark_roundtrip_config" => bookmark_roundtrip_config(dir),
        other => Err(format!("unknown case {other}")),
    }
}

fn visible_keeps_raw_order() -> Result<(), String> {
    let entries = vec![file("b", 2), file("a", 1)];
    let before: Vec<_> = entries.iter().map(|e| e.name.clone()).collect();
    let _ = listing::visible_indices(&entries, SortKey::Name, true, true, "");
    let after: Vec<_> = entries.iter().map(|e| e.name.clone()).collect();
    eq(before, after, "raw order changed")
}

fn hidden_files_follow_toggle() -> Result<(), String> {
    let mut secret = file(".secret", 1);
    secret.flags |= HIDDEN;
    let entries = vec![file("a", 1), secret];
    let shown = listing::visible_indices(&entries, SortKey::Name, true, false, "");
    if shown.len() != 1 {
        return Err(format!("hidden visible: {}", shown.len()));
    }
    let all = listing::visible_indices(&entries, SortKey::Name, true, true, "");
    eq(all.len(), 2, "dotfile hidden while shown")
}

fn sort_keys_order_entries() -> Result<(), String> {
    let entries = vec![
        Entry { name: "b.txt".into(), size: 2, mtime: 20, mode: 0, flags: 0 },
        Entry { name: "a.md".into(), size: 9, mtime: 10, mode: 0, flags: 0 },
    ];
    let name = listing::visible_indices(&entries, SortKey::Name, true, true, "");
    eq(entries[name[0]].name.as_ref(), "a.md", "name")?;
    let ext = listing::visible_indices(&entries, SortKey::Ext, true, true, "");
    eq(entries[ext[0]].name.as_ref(), "a.md", "ext")?;
    let time = listing::visible_indices(&entries, SortKey::Time, true, true, "");
    eq(entries[time[0]].name.as_ref(), "a.md", "time")?;
    let size = listing::visible_indices(&entries, SortKey::Size, true, true, "");
    eq(entries[size[0]].name.as_ref(), "b.txt", "size")
}

fn columns_ext_date_attr() -> Result<(), String> {
    eq(listing::extension("a.md"), "md", "ext")?;
    eq(listing::extension(".."), "", "dot ext")?;
    eq(listing::format_attr(0o100644), "-rw-r--r--".into(), "file attr")?;
    eq(listing::format_attr(0o040755), "drwxr-xr-x".into(), "dir attr")?;
    let entries = vec![
        Entry { name: "a".into(), size: 1, mtime: 1, mode: 0o100644, flags: 0 },
        Entry { name: "b".into(), size: 1, mtime: 1, mode: 0o100755, flags: 0 },
    ];
    let vis = listing::visible_indices(&entries, SortKey::Attr, true, true, "");
    eq(entries[vis[0]].name.as_ref(), "a", "attr sort")
}

fn file_icons_by_kind() -> Result<(), String> {
    eq(listing::file_icon("docs", true, false), "\u{f07b}", "dir")?;
    eq(listing::file_icon("note.txt", false, false), "\u{f15c}", "txt")?;
    eq(listing::file_icon("song.mp3", false, false), "\u{f001}", "mp3")?;
    eq(listing::file_icon("data.bin", false, false), "\u{f15b}", "other")
}

fn column_widths_follow_text() -> Result<(), String> {
    let short = Entry { name: "a.md".into(), size: 12, mtime: 1_700_000_000, mode: 0o100644, flags: 0 };
    let long = Entry { name: "readme.markdown".into(), size: 12, mtime: 1_700_000_000, mode: 0o100644, flags: 0 };
    let a = listing::column_pixels(listing::column_chars(&[short]), 8);
    let b = listing::column_pixels(listing::column_chars(&[long]), 8);
    if b[0] <= a[0] {
        return Err(format!("name {} <= {}", b[0], a[0]));
    }
    if b[1] <= a[1] {
        return Err(format!("ext {} <= {}", b[1], a[1]));
    }
    if b[3] < 16 * 8 {
        return Err(format!("date {}", b[3]));
    }
    Ok(())
}

fn pane_sort_is_independent() -> Result<(), String> {
    let mut state = AppState::new(pb("/left"), pb("/right"));
    let entries = vec![file("b.txt", 1), file("a.txt", 2)];
    state.tab_mut(PaneId::Left).entries = entries.clone();
    state.tab_mut(PaneId::Right).entries = entries;
    state.pane_mut(PaneId::Left).sort = SortKey::Name;
    state.pane_mut(PaneId::Right).sort = SortKey::Size;
    let left: Vec<_> = state
        .visible(PaneId::Left)
        .into_iter()
        .map(|i| state.tab(PaneId::Left).entries[i].name.to_string())
        .collect();
    let right: Vec<_> = state
        .visible(PaneId::Right)
        .into_iter()
        .map(|i| state.tab(PaneId::Right).entries[i].name.to_string())
        .collect();
    eq(left, vec!["a.txt".to_string(), "b.txt".to_string()], "left name")?;
    eq(right, vec!["b.txt".to_string(), "a.txt".to_string()], "right size")
}

fn filter_keeps_substring() -> Result<(), String> {
    let entries = vec![
        file("notes.txt", 1), file("other.bin", 2), file("edit.txt", 3), file("edit.md", 4),
        file("draft.MD", 5), file("old.md.bak", 6), file("möte.md", 7),
    ];
    let vis = listing::visible_indices(&entries, SortKey::Name, true, true, "note");
    if vis.len() != 1 {
        return Err(format!("filter {}", vis.len()));
    }
    eq(entries[vis[0]].name.as_ref(), "notes.txt", "filter")?;
    for query in ["e", "ed", "edi", "edit", "edit.", "edit.t", "edit.tx", "edit.txt"] {
        let matches = listing::visible_indices(&entries, SortKey::Name, true, true, query);
        if !matches.iter().any(|&index| entries[index].name.as_ref() == "edit.txt") {
            return Err(format!("missing edit.txt while filtering {query}"));
        }
        if query == "edit.txt" {
            eq(matches.len(), 1, "full name with extension")?;
        }
    }
    let markdown = listing::visible_indices(&entries, SortKey::Name, true, true, "*.md");
    let names: Vec<_> = markdown.iter().map(|&index| entries[index].name.as_ref()).collect();
    eq(names, vec!["draft.MD", "edit.md", "möte.md"], "markdown extension mask")?;
    let single = listing::visible_indices(&entries, SortKey::Name, true, true, "m?te.md");
    eq(single.len(), 1, "single Unicode character wildcard")?;
    eq(entries[single[0]].name.as_ref(), "möte.md", "single wildcard result")?;
    Ok(())
}

fn insert_mark_is_used() -> Result<(), String> {
    let mut state = AppState::new(pb("/left"), pb("/right"));
    state.tab_mut(PaneId::Left).entries = vec![Entry::dotdot(), file("a.txt", 1), file("b.txt", 1)];
    state.tab_mut(PaneId::Left).cursor = 1;
    state.toggle_mark();
    eq(state.selected_paths(PaneId::Left), vec![pb("/left/a.txt")], "mark")
}

fn space_marks_without_moving_cursor() -> Result<(), String> {
    let mut state = AppState::new(pb("/left"), pb("/right"));
    state.tab_mut(PaneId::Left).entries = vec![Entry::dotdot(), file("a.txt", 1), file("b.txt", 1)];
    state.tab_mut(PaneId::Left).cursor = 1;
    state.toggle_mark_stay();
    if state.tab(PaneId::Left).cursor != 1 {
        return Err("cursor moved".into());
    }
    eq(state.selected_paths(PaneId::Left), vec![pb("/left/a.txt")], "space")
}

fn colors_toml_overrides_palette() -> Result<(), String> {
    let text = "\
mode = \"dark\"
background = \"#111111\"
foreground = \"#eeeeee\"
accent = \"#222222\"
selection = \"#333333\"
muted = \"#444444\"
bright_foreground = \"#555555\"
";
    let palette = theme::from_colors_toml(text).ok_or("parse")?;
    eq(palette.background.as_str(), "#111111", "colors")
}

fn list_background_color_and_image() -> Result<(), String> {
    let palette = theme::builtin();
    let plain = theme::list_background(&palette, "", "", 100);
    if !plain.is_empty() {
        return Err(format!("plain {plain}"));
    }
    let color = theme::list_background(&palette, "#abcdef", "", 100);
    if !color.contains("#abcdef") || color.contains("url(") || color.contains("alpha(") {
        return Err(format!("color {color}"));
    }
    let glass = theme::list_background(&palette, "#abcdef", "", 40);
    if !glass.contains("alpha(#abcdef, 0.40)") || glass.contains("url(") {
        return Err(format!("glass {glass}"));
    }
    let clear = theme::list_background(&palette, "", "", 0);
    if !clear.contains("alpha(") || clear.contains("url(") {
        return Err(format!("clear {clear}"));
    }
    eq(theme::css_file_url("/My Pictures/a.png").as_str(), "file:///My%20Pictures/a.png", "url")?;
    let image = theme::list_background(&palette, "#abcdef", "/My Pictures/a.png", 70);
    if !image.contains("file:///My%20Pictures/a.png") || !image.contains("0.30") {
        return Err(format!("image {image}"));
    }
    Ok(())
}

fn enter_opens_directory() -> Result<(), String> {
    let mut state = AppState::new(pb("/left"), pb("/right"));
    let mut dir = file("sub", 0);
    dir.flags |= DIR;
    state.tab_mut(PaneId::Left).entries = vec![Entry::dotdot(), dir];
    state.tab_mut(PaneId::Left).cursor = 1;
    let (_, path, _) = state.enter().ok_or("enter returned none")?;
    eq(path, pb("/left/sub"), "enter path")
}

fn up_goes_to_parent() -> Result<(), String> {
    let mut state = AppState::new(pb("/left/sub"), pb("/right"));
    let (_, path, _) = state.up().ok_or("up returned none")?;
    eq(path, pb("/left"), "parent")
}

fn refresh_bumps_token() -> Result<(), String> {
    let mut state = AppState::new(pb("/left"), pb("/right"));
    let before = state.tab(PaneId::Left).token;
    let _ = state.refresh(PaneId::Left);
    if state.tab(PaneId::Left).token <= before {
        return Err("token".into());
    }
    Ok(())
}

fn empty_mark_uses_cursor() -> Result<(), String> {
    let mut state = AppState::new(pb("/left"), pb("/right"));
    state.tab_mut(PaneId::Left).entries = vec![Entry::dotdot(), file("a.txt", 1)];
    state.tab_mut(PaneId::Left).cursor = 1;
    let paths = state.selected_paths(PaneId::Left);
    eq(paths, vec![pb("/left/a.txt")], "cursor selection")
}

fn history_back() -> Result<(), String> {
    let mut state = AppState::new(pb("/left"), pb("/right"));
    state.navigate(PaneId::Left, pb("/left/child"), true);
    let (path, _) = state.go_back(PaneId::Left).ok_or("no history")?;
    eq(path, pb("/left"), "history")
}

fn new_tab_has_own_listing() -> Result<(), String> {
    let mut state = AppState::new(pb("/left"), pb("/right"));
    state.tab_mut(PaneId::Left).entries = vec![file("only-old", 1)];
    state.new_tab(PaneId::Left);
    if !state.tab(PaneId::Left).entries.is_empty() {
        return Err("new tab copied listing".into());
    }
    eq(
        state.pane(PaneId::Left).tabs[0].entries[0].name.as_ref(),
        "only-old",
        "old tab",
    )
}

fn theme_fallback_without_file() -> Result<(), String> {
    let palette = theme::palette_from_optional(None);
    eq(palette.background.as_str(), "#1a1b26", "fallback")
}

fn places_include_home_and_root() -> Result<(), String> {
    let home = pb("/home/example");
    let places = rust_commander::places::places(&home, &[]);
    if !places.iter().any(|(_, path)| path == &home) || !places.iter().any(|(_, path)| path == Path::new("/")) {
        return Err("places".into());
    }
    Ok(())
}

fn mounts_lists_disks() -> Result<(), String> {
    let text = "\
/dev/nvme0n1p2 / ext4 rw 0 0
tmpfs /run tmpfs rw 0 0
/dev/sda1 /media/marcus/USB vfat rw 0 0
/dev/nvme0n1p1 /boot/efi vfat rw 0 0
/dev/sdb1 /run/media/marcus/Stick exfat rw 0 0
systemd-1 /proc/sys/fs/binfmt_misc autofs rw 0 0
systemd-1 /mnt/wd4tb autofs rw 0 0
systemd-1 /mnt/seagate12tb autofs rw 0 0
//smb.lan/seagate12tb /mnt/seagate12tb cifs rw 0 0
";
    let mounts = rust_commander::places::parse_mounts(text);
    let paths: Vec<_> = mounts.iter().map(|m| m.path.clone()).collect();
    eq(paths.first().cloned().unwrap_or_default(), pb("/"), "root first")?;
    if paths.iter().any(|p| p.starts_with("/run") && !p.starts_with("/run/media")) {
        return Err("tmp run listed".into());
    }
    if !paths.iter().any(|p| p == Path::new("/media/marcus/USB")) || !paths.iter().any(|p| p == Path::new("/run/media/marcus/Stick")) {
        return Err("usb missing".into());
    }
    let usb = mounts.iter().find(|m| m.path == pb("/media/marcus/USB")).ok_or("usb")?;
    if !usb.removable || usb.label != "USB" {
        return Err("usb label".into());
    }
    if paths.iter().any(|p| p.starts_with("/boot")) {
        return Err("efi listed".into());
    }
    let share = mounts.iter().find(|m| m.path == pb("/mnt/seagate12tb")).ok_or("share")?;
    if !share.network || share.removable || share.label != "seagate12tb" {
        return Err("share".into());
    }
    let idle = mounts.iter().find(|m| m.path == pb("/mnt/wd4tb")).ok_or("wd")?;
    if idle.network || idle.label != "wd4tb" {
        return Err("idle automount".into());
    }
    if paths.iter().any(|p| p.starts_with("/proc")) {
        return Err("binfmt listed".into());
    }
    let refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
    let usb = rust_commander::places::current_mount(&refs, Path::new("/media/marcus/USB/dcim")).ok_or("usb slot")?;
    if paths[usb] != pb("/media/marcus/USB") {
        return Err("usb prefix".into());
    }
    let share_i = rust_commander::places::current_mount(&refs, Path::new("/mnt/seagate12tb/foto")).ok_or("share slot")?;
    if paths[share_i] != pb("/mnt/seagate12tb") {
        return Err("share prefix".into());
    }
    let root = rust_commander::places::current_mount(&refs, Path::new("/home/marcus")).ok_or("root slot")?;
    if paths[root] != pb("/") {
        return Err("root prefix".into());
    }
    Ok(())
}

fn ctrl_arrow_shows_active_dir_on_other_pane() -> Result<(), String> {
    let mut state = AppState::new(pb("/left"), pb("/right"));
    state.tab_mut(PaneId::Left).cursor = 2;
    let (dest, path, _) = state.mirror_to_other();
    if dest != PaneId::Right || state.tab(PaneId::Left).cursor != 2 {
        return Err("source moved".into());
    }
    eq(path, pb("/left"), "mirrored")?;
    eq(state.tab(PaneId::Right).path.clone(), pb("/left"), "other pane")
}

fn terminal_argv_without_starting() -> Result<(), String> {
    let dir = pb("/work");
    let foot = external::terminal_start_with(Some("foot"), |_| false, &dir).ok_or("foot")?;
    eq(foot, vec!["foot".to_string()], "env")?;
    let gnome = external::terminal_start_with(None, |name| name == "gnome-terminal", &dir).ok_or("gnome")?;
    eq(
        gnome,
        vec!["gnome-terminal".to_string(), "--working-directory".into(), "/work".into()],
        "gnome",
    )?;
    if external::terminal_start_with(None, |_| false, &dir).is_some() {
        return Err("missing terminal started".into());
    }
    Ok(())
}

fn external_argv() -> Result<(), String> {
    let editor = external::editor_argv("", Some("nvim"), Path::new("/tmp/a"));
    eq(editor, vec!["nvim".to_string(), "/tmp/a".to_string()], "editor")?;
    let viewer = external::viewer_argv("less -N", Path::new("/tmp/a"));
    eq(viewer[0].as_str(), "less", "viewer")?;
    eq(viewer[1].as_str(), "-N", "viewer arg")?;
    let has = |name: &str| matches!(name, "gnome-text-editor" | "code" | "gnome-terminal" | "nvim");
    let auto = external::editor_argv_with("", None, has, Path::new("/tmp/a"));
    eq(
        auto,
        vec!["gnome-text-editor".into(), "--standalone".into(), "/tmp/a".into()],
        "auto editor",
    )?;
    let code = external::editor_argv_with("code", Some("nvim"), has, Path::new("/tmp/a"));
    eq(code, vec!["code".into(), "/tmp/a".into()], "code")?;
    let nvim = external::editor_argv_with("nvim", None, has, Path::new("/tmp/a"));
    eq(
        nvim,
        vec!["gnome-terminal".into(), "--".into(), "nvim".into(), "/tmp/a".into()],
        "nvim",
    )?;
    let custom = external::editor_argv_with("/opt/ed --wait", None, |_| false, Path::new("/tmp/a"));
    eq(custom, vec!["/opt/ed".into(), "--wait".into(), "/tmp/a".into()], "custom")?;
    if !external::editor_argv_with("", None, |_| false, Path::new("/tmp/a")).is_empty() {
        return Err("xdg-open used".into());
    }
    eq(external::editor_id_for_path(Path::new("/usr/bin/code")), "code".into(), "code id")?;
    let choices = external::editor_choices_with("", None, has);
    let labeled = external::editor_choices_with("", Some("nvim"), |_| false);
    if !labeled[0].label.contains("nvim") {
        return Err("editor env label".into());
    }
    if !choices.iter().any(|choice| choice.id == "code" && choice.label.contains("Visual Studio Code")) {
        return Err("code missing from choices".into());
    }
    eq(choices.last().map(|choice| choice.id.as_str()), Some(external::EDITOR_BROWSE), "other")?;
    let vlc = "/usr/bin/flatpak run --file-forwarding org.videolan.VLC --started-from-file @@u %U @@";
    let argv = external::desktop_argv(vlc, Path::new("/tmp/a b.mkv"));
    if argv.iter().any(|arg| arg == "--started-from-file") {
        return Err("started-from-file kept".into());
    }
    eq(argv.last().map(String::as_str), Some("@@"), "flatpak end")?;
    if !argv.iter().any(|arg| arg == "file:///tmp/a%20b.mkv") {
        return Err(format!("uri {argv:?}"));
    }
    Ok(())
}

fn lister_ansi_and_ascii() -> Result<(), String> {
    let ansi = lister::render(ViewMode::Text, Charset::Ansi, &[0x84], 0);
    if ansi != "\u{201e}" {
        return Err(format!("ansi {ansi}"));
    }
    let dos = lister::render(ViewMode::Text, Charset::Ascii, &[0x84, b'\n', 0xC4], 0);
    if dos != "ä\n\u{2500}" {
        return Err(format!("dos {dos}"));
    }
    let hex = lister::render(ViewMode::Hex, Charset::Ascii, &[0xC4], 0);
    if !hex.contains('\u{2500}') {
        return Err("hex dos".into());
    }
    let text = lister::format_text(b"a\0b\xff");
    if text.contains('\0') || text != "a.b\u{FFFD}" {
        return Err(format!("nul {text}"));
    }
    let frame = lister::render(ViewMode::Text, Charset::Ascii, b"\xc9\xcd\xbb\r\n\xdd", 0);
    if frame != "╔═╗\n▌" {
        return Err(format!("nfo {frame}"));
    }
    if lister::choose_charset("movie.nfo", b"\xef\xbb\xbf<?xml", Charset::Utf8) != Charset::Utf8 {
        return Err("utf8 nfo".into());
    }
    if lister::choose_charset("scene.nfo", b"\xc9\xcd", Charset::Utf8) != Charset::Ascii {
        return Err("cp437 nfo".into());
    }
    if lister::choose_charset("scene.nfo", b"\xc9\xcd", Charset::Ansi) != Charset::Ansi {
        return Err("ansi stays".into());
    }
    Ok(())
}

fn lister_pretty_json() -> Result<(), String> {
    let pretty = lister::render(ViewMode::Text, Charset::Json, br#"{"a":1,"b":[true,{"c":"}]","n":-1.5e+10}]}"#, 0);
    let expect = "{\n  \"a\": 1,\n  \"b\": [\n    true,\n    {\n      \"c\": \"}]\",\n      \"n\": -1.5e+10\n    }\n  ]\n}";
    if pretty != expect {
        return Err(format!("json {pretty}"));
    }
    let bom = lister::render(ViewMode::Text, Charset::Json, b"\xef\xbb\xbf{}", 0);
    if bom != "{}" {
        return Err(format!("bom {bom}"));
    }
    let bad = lister::render(ViewMode::Text, Charset::Json, b"{", 0);
    if !bad.starts_with("Not valid JSON") {
        return Err(format!("bad {bad}"));
    }
    if lister::choose_charset("a.json", b"{}", Charset::Json) != Charset::Json {
        return Err("json stays".into());
    }
    Ok(())
}

fn quick_view_ignores_stale_generation() -> Result<(), String> {
    let mut slot = lister::PreviewSlot { generation: 2, ..lister::PreviewSlot::default() };
    if lister::apply_preview(&mut slot, 1, PreviewBody::Text("stale".into())) {
        return Err("stale applied".into());
    }
    eq(slot.text(), "", "stale text")
}

fn image_list_skips_other_files() -> Result<(), String> {
    let mut folder = file("photo.jpg", 1);
    folder.flags |= DIR;
    let entries = vec![Entry::dotdot(), file("a.txt", 1), file("b.JPG", 1), folder, file("c.png", 1)];
    let order: Vec<usize> = (0..entries.len()).collect();
    let paths = imageview::image_paths(Path::new("/pics"), &entries, &order);
    eq(paths, vec![pb("/pics/b.JPG"), pb("/pics/c.png")], "images")
}

fn image_step_stops_at_end() -> Result<(), String> {
    eq(imageview::step_index(3, 0, -1), None, "step back")?;
    eq(imageview::step_index(3, 2, 1), None, "step forward")?;
    eq(imageview::step_index(3, 1, 1), Some(2), "step mid")
}

fn image_decode_png_and_scales() -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("rust-commander-img-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(s)?;
    let png = dir.join("dot.png");
    fs::write(&png, TINY_PNG).map_err(s)?;
    let small = imageview::decode(&png, imageview::MAX_EDGE).map_err(|err| format!("png {err}"))?;
    if small.width != 1 || small.height != 1 {
        let _ = fs::remove_dir_all(&dir);
        return Err(format!("png {}x{}", small.width, small.height));
    }
    let wide = dir.join("wide.bmp");
    fs::write(&wide, bmp24(3000, 2)).map_err(s)?;
    let scaled = imageview::decode(&wide, 2048).map_err(|err| format!("bmp {err}"))?;
    let _ = fs::remove_dir_all(&dir);
    if scaled.width != 2048 || scaled.height != 1 {
        return Err(format!("scaled {}x{}", scaled.width, scaled.height));
    }
    Ok(())
}

const TINY_PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 218, 99, 252, 207, 192, 80, 15, 0, 4, 133, 1, 128, 132, 169, 140, 33, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

fn bmp24(w: u32, h: u32) -> Vec<u8> {
    let stride = (w as usize * 3 + 3) & !3;
    let size = (54 + stride * h as usize) as u32;
    let mut out = vec![0u8; size as usize];
    out[0] = b'B';
    out[1] = b'M';
    out[2..6].copy_from_slice(&size.to_le_bytes());
    out[10..14].copy_from_slice(&54u32.to_le_bytes());
    out[14..18].copy_from_slice(&40u32.to_le_bytes());
    out[18..22].copy_from_slice(&w.to_le_bytes());
    out[22..26].copy_from_slice(&h.to_le_bytes());
    out[26..28].copy_from_slice(&1u16.to_le_bytes());
    out[28..30].copy_from_slice(&24u16.to_le_bytes());
    out
}

fn landlock_blocks_outside_write(dir: &Path) -> Result<(), String> {
    let outside = dir.parent().unwrap().join(format!("rust-commander-escape-{}", std::process::id()));
    match File::create(&outside) {
        Ok(_) => {
            let _ = fs::remove_file(&outside);
            Err("wrote outside the fixture".into())
        }
        Err(_) => Ok(()),
    }
}

fn list_dir_reads_names(dir: &Path) -> Result<(), String> {
    fs::write(dir.join("a.txt"), "hi").map_err(s)?;
    fs::create_dir(dir.join("sub")).map_err(s)?;
    let entries = listing::read_dir(dir).map_err(s)?;
    let before: Vec<_> = entries.iter().map(|e| e.name.clone()).collect();
    let _ = listing::visible_indices(&entries, SortKey::Name, true, true, "");
    let after: Vec<_> = entries.iter().map(|e| e.name.clone()).collect();
    if before != after {
        return Err("listing mutated".into());
    }
    if !entries.iter().any(|e| e.name.as_ref() == "a.txt") {
        return Err("missing a.txt".into());
    }
    Ok(())
}

fn search_nested_names(dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir.join("nested")).map_err(s)?;
    fs::create_dir_all(dir.join(".private")).map_err(s)?;
    fs::write(dir.join("nested/report.txt"), "one").map_err(s)?;
    fs::write(dir.join(".private/Report.txt"), "two").map_err(s)?;
    std::os::unix::fs::symlink(dir.join("nested"), dir.join("shortcut")).map_err(s)?;
    let options = |query: &str| Options {
        root: dir.to_path_buf(), query: query.into(), recursive: true,
        hidden: false, files_only: false, case_sensitive: false,
    };
    let collect = |options| {
        let mut found = Vec::new();
        let mut finished = None;
        search::run(options, || false, |event| match event {
            Event::Matches(paths) => found.extend(paths),
            Event::Finished { errors, cancelled } => finished = Some((errors, cancelled)),
        });
        (found, finished)
    };
    let (found, finished) = collect(options("report"));
    eq(found, vec![dir.join("nested/report.txt")], "recursive visible names")?;
    eq(finished, Some((0, false)), "search completed")?;
    let (found, _) = collect(Options { recursive: false, ..options("report") });
    eq(found.len(), 0, "nonrecursive search")?;
    let (found, _) = collect(Options { hidden: true, ..options("report") });
    eq(found.len(), 2, "hidden result or symlink traversal")?;
    let (found, _) = collect(Options { case_sensitive: true, ..options("Report") });
    eq(found.len(), 0, "case-sensitive search")?;
    let (found, _) = collect(options("nested"));
    eq(found, vec![dir.join("nested")], "directory match")?;
    let (found, _) = collect(Options { files_only: true, ..options("nested") });
    eq(found.len(), 0, "files only")
}

fn search_cancelled(dir: &Path) -> Result<(), String> {
    for index in 0..90 {
        fs::write(dir.join(format!("result-{index}")), "").map_err(s)?;
    }
    let stopped = std::cell::Cell::new(false);
    let mut batches = 0;
    let mut finished = None;
    search::run(Options {
        root: dir.to_path_buf(), query: "result".into(), recursive: true,
        hidden: false, files_only: true, case_sensitive: false,
    }, || stopped.get(), |event| match event {
        Event::Matches(_) => {
            batches += 1;
            stopped.set(true);
        }
        Event::Finished { errors, cancelled } => finished = Some((errors, cancelled)),
    });
    eq(batches, 1, "stopped after the first batch")?;
    eq(finished, Some((0, true)), "cancelled status")
}

fn f5_copies_tree_to_other_pane(dir: &Path) -> Result<(), String> {
    let src = dir.join("src");
    let dst = dir.join("dst");
    fs::create_dir_all(src.join("sub")).map_err(s)?;
    fs::create_dir_all(&dst).map_err(s)?;
    fs::write(src.join("a.txt"), "A").map_err(s)?;
    fs::write(src.join("sub/b.txt"), "B").map_err(s)?;
    copy(&[(src.clone(), dst.join("src"))])?;
    eq(fs::read(dst.join("src/sub/b.txt")).map_err(s)?, b"B".to_vec(), "tree")
}

fn shift_f5_copies_with_new_name(dir: &Path) -> Result<(), String> {
    fs::write(dir.join("a.txt"), "A").map_err(s)?;
    copy(&[(dir.join("a.txt"), dir.join("b.txt"))])?;
    eq(fs::read(dir.join("b.txt")).map_err(s)?, b"A".to_vec(), "copy as")
}

fn f6_moves_file(dir: &Path) -> Result<(), String> {
    let src = dir.join("src");
    let dst = dir.join("dst");
    fs::create_dir_all(&src).map_err(s)?;
    fs::create_dir_all(&dst).map_err(s)?;
    fs::write(src.join("f"), "x").map_err(s)?;
    transfer(&[(src.join("f"), dst.join("f"))], Kind::Move)?;
    if src.join("f").exists() || fs::read(dst.join("f")).map_err(s)? != b"x" {
        return Err("move".into());
    }
    Ok(())
}

fn shift_f6_renames(dir: &Path) -> Result<(), String> {
    fs::write(dir.join("old"), "z").map_err(s)?;
    ops::rename_in_place(&dir.join("old"), "new").map_err(s)?;
    if dir.join("old").exists() || !dir.join("new").is_file() {
        return Err("rename".into());
    }
    Ok(())
}

fn f7_makes_directory(dir: &Path) -> Result<(), String> {
    ops::make_dir(dir, "folder").map_err(s)?;
    if dir.join("folder").is_dir() { Ok(()) } else { Err("mkdir".into()) }
}

fn shift_f4_creates_file(dir: &Path) -> Result<(), String> {
    ops::make_file(dir, "note.txt").map_err(s)?;
    if fs::read(dir.join("note.txt")).map_err(s)? != b"" {
        return Err("not empty".into());
    }
    if ops::make_file(dir, "note.txt").is_ok() {
        return Err("overwrite".into());
    }
    if ops::make_file(dir, "../escape").is_ok() || ops::make_file(dir, "").is_ok() {
        return Err("bad name".into());
    }
    Ok(())
}

fn f8_calls_trash(dir: &Path) -> Result<(), String> {
    let file = dir.join("gone.txt");
    fs::write(&file, "t").map_err(s)?;
    let seen = AtomicUsize::new(0);
    ops::delete_paths(&[file.clone()], false, &mut |path| {
        if path == file {
            seen.fetch_add(1, Ordering::Relaxed);
        }
        fs::remove_file(path)
    })
    .map_err(s)?;
    if seen.load(Ordering::Relaxed) != 1 || file.exists() {
        return Err("trash".into());
    }
    Ok(())
}

fn shift_f8_deletes(dir: &Path) -> Result<(), String> {
    fs::create_dir(dir.join("d")).map_err(s)?;
    fs::write(dir.join("d/a"), "a").map_err(s)?;
    ops::delete_paths(&[dir.join("d")], true, &mut |_| Err(io::Error::other("trash called"))).map_err(s)?;
    if dir.join("d").exists() { Err("still there".into()) } else { Ok(()) }
}

fn conflict_overwrite_rest(dir: &Path) -> Result<(), String> {
    let src = dir.join("src");
    let dst = dir.join("dst");
    fs::create_dir_all(&src).map_err(s)?;
    fs::create_dir_all(&dst).map_err(s)?;
    fs::write(src.join("a"), "new-a").map_err(s)?;
    fs::write(src.join("b"), "new-b").map_err(s)?;
    fs::write(dst.join("a"), "old-a").map_err(s)?;
    fs::write(dst.join("b"), "old-b").map_err(s)?;
    let asks = AtomicUsize::new(0);
    ops::transfer(
        &[(src.join("a"), dst.join("a")), (src.join("b"), dst.join("b"))],
        Kind::Copy,
        &AtomicBool::new(false),
        &mut |_| {},
        &mut |_, _| {
            asks.fetch_add(1, Ordering::Relaxed);
            Choice::OverwriteAll
        },
    )
    .map_err(s)?;
    if asks.load(Ordering::Relaxed) != 1 {
        return Err(format!("asks {}", asks.load(Ordering::Relaxed)));
    }
    eq(fs::read(dst.join("b")).map_err(s)?, b"new-b".to_vec(), "rest")
}

fn cancel_removes_partial(dir: &Path) -> Result<(), String> {
    let src = dir.join("big");
    let dest = dir.join("big-out");
    let mut file = File::create(&src).map_err(s)?;
    let byte = [1u8; 64 * 1024];
    let chunks = (ops::CHUNK * 2 / byte.len()) + 2;
    for _ in 0..chunks {
        file.write_all(&byte).map_err(s)?;
    }
    drop(file);
    let cancel = AtomicBool::new(false);
    let mut seen = 0u32;
    let _ = ops::transfer(
        &[(src, dest.clone())],
        Kind::Copy,
        &cancel,
        &mut |_| {
            seen += 1;
            if seen == 1 {
                cancel.store(true, Ordering::Relaxed);
            }
        },
        &mut |_, _| Choice::Overwrite,
    );
    let partial = dest.with_file_name(".rust-commander-partial-big-out");
    if dest.exists() || partial.exists() {
        return Err("partial left behind".into());
    }
    Ok(())
}

fn symlink_copied_as_link(dir: &Path) -> Result<(), String> {
    let src = dir.join("src");
    let dst = dir.join("dst");
    fs::create_dir_all(&src).map_err(s)?;
    fs::create_dir_all(&dst).map_err(s)?;
    std::os::unix::fs::symlink("missing-target", src.join("link")).map_err(s)?;
    copy(&[(src.join("link"), dst.join("link"))])?;
    let meta = fs::symlink_metadata(dst.join("link")).map_err(s)?;
    if !meta.file_type().is_symlink() {
        return Err("followed symlink".into());
    }
    eq(fs::read_link(dst.join("link")).map_err(s)?, pb("missing-target"), "link text")
}

fn copy_into_itself_aborts(dir: &Path) -> Result<(), String> {
    let src = dir.join("tree");
    fs::create_dir_all(src.join("nested")).map_err(s)?;
    fs::write(src.join("a"), "a").map_err(s)?;
    let err = ops::transfer(
        &[(src.clone(), src.join("nested/tree"))],
        Kind::Copy,
        &AtomicBool::new(false),
        &mut |_| {},
        &mut |_, _| Choice::Overwrite,
    );
    if err.is_ok() {
        return Err("inside copy allowed".into());
    }
    if !src.join("a").is_file() {
        return Err("source damaged".into());
    }
    Ok(())
}

fn lister_text_hex_binary_window(dir: &Path) -> Result<(), String> {
    let path = dir.join("note.txt");
    fs::write(&path, "hello\nrust-commander").map_err(s)?;
    let mut reader = lister::Reader::open(&path).map_err(s)?;
    let bytes = reader.read_window().map_err(s)?;
    let text = lister::render(ViewMode::Text, Charset::Utf8, &bytes, 0);
    if !text.contains("hello") {
        return Err("text".into());
    }
    let hex = lister::render(ViewMode::Hex, Charset::Utf8, &bytes, 0);
    if !hex.contains("68") {
        return Err("hex".into());
    }
    let binary = lister::render(ViewMode::Binary, Charset::Utf8, &bytes, 0);
    if !binary.contains("hello") {
        return Err("binary".into());
    }
    Ok(())
}

fn lister_large_file_reads_window_only(dir: &Path) -> Result<(), String> {
    let path = dir.join("sparse");
    let file = File::create(&path).map_err(s)?;
    file.set_len(32 * 1024 * 1024).map_err(s)?;
    drop(file);
    let mut reader = lister::Reader::open(&path).map_err(s)?;
    let bytes = reader.read_window().map_err(s)?;
    if bytes.len() > lister::WINDOW || reader.bytes_read > lister::WINDOW as u64 {
        return Err(format!("read {} bytes", reader.bytes_read));
    }
    if bytes.len() != lister::WINDOW {
        return Err(format!("short window {}", bytes.len()));
    }
    Ok(())
}

fn hotlist_accel_submenu() -> Result<(), String> {
    let (label, accel) = rust_commander::hotlist::split_accel("Dropbox &d");
    eq(label, "Dropbox d".into(), "label")?;
    eq(accel, Some('d'), "accel")?;
    let (label, accel) = rust_commander::hotlist::split_accel("A && B");
    eq(label, "A & B".into(), "amp")?;
    eq(accel, None, "no accel")?;
    let marks = vec![
        Bookmark { name: "Dropbox &d".into(), path: pb("/drop"), target: PathBuf::new(), kind: String::new() },
        Bookmark { name: "Work".into(), path: PathBuf::new(), target: PathBuf::new(), kind: "menu".into() },
        Bookmark { name: "Projects &p".into(), path: pb("/proj"), target: PathBuf::new(), kind: String::new() },
        Bookmark { name: "--".into(), path: PathBuf::new(), target: PathBuf::new(), kind: "end".into() },
        Bookmark { name: "-".into(), path: PathBuf::new(), target: PathBuf::new(), kind: "sep".into() },
    ];
    let root = rust_commander::hotlist::rows(&marks, None);
    if root.len() != 3 {
        return Err(format!("root {}", root.len()));
    }
    let kids = rust_commander::hotlist::rows(&marks, Some(1));
    if kids.len() != 1 {
        return Err(format!("submenu {}", kids.len()));
    }
    if rust_commander::hotlist::dir_index(&marks, Path::new("/drop")) != Some(0) {
        return Err("index".into());
    }
    Ok(())
}

fn bookmark_roundtrip_config(dir: &Path) -> Result<(), String> {
    let path = dir.join("config.toml");
    let mut cfg = Config::default();
    cfg.bookmarks.push(Bookmark {
        name: "Work".into(),
        path: pb("/work"),
        target: pb("/other"),
        kind: String::new(),
    });
    cfg.auto_columns = false;
    cfg.column_widths = vec![10, 20, 30, 40, 50];
    config::save_to(&path, &cfg).map_err(s)?;
    let loaded = config::load_from(&path);
    if loaded.bookmarks.len() != 1 || loaded.bookmarks[0].name != "Work" || loaded.bookmarks[0].target != pb("/other") {
        return Err("bookmark".into());
    }
    if loaded.auto_columns || loaded.column_widths != vec![10, 20, 30, 40, 50] {
        return Err("columns".into());
    }
    if !loaded.mode_buttons {
        return Err("mode buttons".into());
    }
    fs::write(&path, "show_hidden = true\n").map_err(s)?;
    let loaded = config::load_from(&path);
    if !loaded.mode_buttons {
        return Err("mode buttons default".into());
    }
    if loaded.file_icons {
        return Err("file icons default".into());
    }
    Ok(())
}

fn copy(items: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    transfer(items, Kind::Copy)
}

fn transfer(items: &[(PathBuf, PathBuf)], kind: Kind) -> Result<(), String> {
    ops::transfer(items, kind, &AtomicBool::new(false), &mut |_| {}, &mut |_, _| Choice::Overwrite).map_err(s)?;
    Ok(())
}

fn file(name: &str, size: u64) -> Entry {
    Entry { name: name.into(), size, mtime: 1, mode: 0, flags: 0 }
}

fn pb(path: &str) -> PathBuf {
    PathBuf::from(path)
}

fn s(err: io::Error) -> String {
    err.to_string()
}

fn eq<T: PartialEq + std::fmt::Debug>(got: T, want: T, what: &str) -> Result<(), String> {
    if got == want { Ok(()) } else { Err(format!("{what}: {got:?} != {want:?}")) }
}

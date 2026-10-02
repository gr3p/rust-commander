use std::ffi::OsString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

// Not stored. The settings row uses it for "Other…".
pub const EDITOR_BROWSE: &str = "\u{0}";

struct Preset {
    id: &'static str,
    name: &'static str,
    bins: &'static [&'static str],
    args: &'static [&'static str],
    terminal: bool,
}

const PRESETS: &[Preset] = &[
    // --standalone skips the saved session, which can reopen admin:// and ask for a password.
    Preset { id: "gnome-text-editor", name: "Text Editor", bins: &["gnome-text-editor"], args: &["--standalone"], terminal: false },
    Preset { id: "code", name: "Visual Studio Code", bins: &["code"], args: &[], terminal: false },
    Preset { id: "cursor", name: "Cursor", bins: &["cursor"], args: &[], terminal: false },
    Preset { id: "code-insiders", name: "VS Code Insiders", bins: &["code-insiders"], args: &[], terminal: false },
    Preset { id: "codium", name: "VSCodium", bins: &["codium"], args: &[], terminal: false },
    Preset { id: "zed", name: "Zed", bins: &["zed", "zeditor"], args: &[], terminal: false },
    Preset { id: "gedit", name: "gedit", bins: &["gedit"], args: &[], terminal: false },
    Preset { id: "kate", name: "Kate", bins: &["kate"], args: &[], terminal: false },
    Preset { id: "mousepad", name: "Mousepad", bins: &["mousepad"], args: &[], terminal: false },
    Preset { id: "xed", name: "Xed", bins: &["xed"], args: &[], terminal: false },
    Preset { id: "emacs", name: "Emacs", bins: &["emacs"], args: &[], terminal: false },
    Preset { id: "subl", name: "Sublime Text", bins: &["subl"], args: &[], terminal: false },
    Preset { id: "nano", name: "Nano", bins: &["nano"], args: &[], terminal: true },
    Preset { id: "vim", name: "Vim", bins: &["vim"], args: &[], terminal: true },
    Preset { id: "nvim", name: "Neovim", bins: &["nvim"], args: &[], terminal: true },
    Preset { id: "hx", name: "Helix", bins: &["hx"], args: &[], terminal: true },
];

pub struct EditorChoice {
    pub id: String,
    pub label: String,
}

// Draw in the terminal they run in. Spawned with no stdio they would run with no window.
const TERMINAL_EDITORS: &[&str] = &["vi", "micro", "helix", "kak", "joe", "ne", "mcedit", "ed", "mg"];

// xdg-terminal-exec runs the terminal the desktop picked (Omarchy sets it up).
const TERMINALS: &[&str] = &[
    "xdg-terminal-exec",
    "x-terminal-emulator",
    "kitty",
    "foot",
    "alacritty",
    "ghostty",
    "wezterm",
    "gnome-terminal",
    "konsole",
    "xfce4-terminal",
    "xterm",
];

pub fn viewer_argv(configured: &str, path: &Path) -> Vec<OsString> {
    argv(configured, None, "xdg-open", path)
}

// Desktop Exec field codes. Flatpak VLC drops a file handed to a running player, so that flag is skipped.
pub fn desktop_argv(exec: &str, path: &Path) -> Vec<OsString> {
    let uri = file_uri(path);
    let mut out = Vec::new();
    for arg in split_exec(exec) {
        if arg == "--started-from-file" {
            continue;
        }
        match arg.as_str() {
            "%f" | "%F" => out.push(path.into()),
            "%u" | "%U" => out.push(uri.clone().into()),
            "%i" | "%c" | "%k" => {}
            other => {
                let other = other.replace("%%", "%");
                if !other.is_empty() {
                    out.push(other.into());
                }
            }
        }
    }
    out
}

fn split_exec(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote = false;
    let mut escape = false;
    for ch in line.chars() {
        if escape {
            cur.push(ch);
            escape = false;
            continue;
        }
        match ch {
            '\\' if quote => escape = true,
            '"' => quote = !quote,
            c if c.is_whitespace() && !quote => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

// Percent-encode each byte. '/' stays, so the result is file:///tmp/a.
fn file_uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// $VISUAL is the full-screen editor, so it wins over $EDITOR.
pub fn editor_env() -> Option<String> {
    ["VISUAL", "EDITOR"].iter().find_map(|key| std::env::var(key).ok().filter(|line| !line.trim().is_empty()))
}

pub fn editor_argv(configured: &str, editor_env: Option<&str>, path: &Path) -> Vec<OsString> {
    let terminal = std::env::var("TERMINAL").ok();
    editor_argv_in(configured, editor_env, terminal.as_deref(), program_on_path, path)
}

pub fn editor_argv_with(
    configured: &str,
    editor_env: Option<&str>,
    exists: impl Fn(&str) -> bool,
    path: &Path,
) -> Vec<OsString> {
    editor_argv_in(configured, editor_env, None, exists, path)
}

// `terminal_env` is $TERMINAL. It wins over the terminals found on PATH.
pub fn editor_argv_in(
    configured: &str,
    editor_env: Option<&str>,
    terminal_env: Option<&str>,
    exists: impl Fn(&str) -> bool,
    path: &Path,
) -> Vec<OsString> {
    let configured = configured.trim();
    if !configured.is_empty() && configured != EDITOR_BROWSE {
        if let Some(preset) = PRESETS.iter().find(|preset| preset.id == configured) {
            return preset_cmd(preset, terminal_env, &exists, path).unwrap_or_default();
        }
        return custom_cmd(configured, terminal_env, &exists, path);
    }
    if let Some(env) = editor_env.map(str::trim).filter(|line| !line.is_empty()) {
        return custom_cmd(env, terminal_env, &exists, path);
    }
    auto_preset(&exists)
        .and_then(|preset| preset_cmd(preset, terminal_env, &exists, path))
        .unwrap_or_default()
}

pub fn editor_choices(configured: &str) -> Vec<EditorChoice> {
    let env = editor_env();
    editor_choices_with(configured, env.as_deref(), program_on_path)
}

pub fn editor_choices_with(configured: &str, editor_env: Option<&str>, exists: impl Fn(&str) -> bool) -> Vec<EditorChoice> {
    let mut out = Vec::new();
    let label = if let Some(env) = editor_env.map(str::trim).filter(|line| !line.is_empty()) {
        format!("Automatic  ({env})")
    } else {
        match auto_preset(&exists) {
            Some(preset) => format!("Automatic  ({})", preset.name),
            None => "Automatic".into(),
        }
    };
    out.push(EditorChoice { id: String::new(), label });
    for preset in PRESETS {
        let Some(bin) = preset.bins.iter().copied().find(|name| exists(name)) else { continue };
        let shown = program_path(bin).map(|p| p.display().to_string()).unwrap_or_else(|| bin.to_string());
        out.push(EditorChoice {
            id: preset.id.to_string(),
            label: format!("{}  {shown}", preset.name),
        });
    }
    let configured = configured.trim();
    if !configured.is_empty() && configured != EDITOR_BROWSE && !out.iter().any(|choice| choice.id == configured) {
        out.push(EditorChoice { id: configured.to_string(), label: configured.to_string() });
    }
    out.push(EditorChoice { id: EDITOR_BROWSE.into(), label: "Other…".into() });
    out
}

pub fn editor_id_for_path(path: &Path) -> String {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return path.display().to_string();
    };
    if let Some(preset) = PRESETS.iter().find(|preset| preset.id == name || preset.bins.iter().any(|bin| *bin == name)) {
        return preset.id.to_string();
    }
    path.display().to_string()
}

fn auto_preset(exists: &impl Fn(&str) -> bool) -> Option<&'static Preset> {
    PRESETS.iter().find(|preset| preset.bins.iter().any(|name| exists(name)))
}

fn preset_cmd(preset: &Preset, terminal_env: Option<&str>, exists: &impl Fn(&str) -> bool, path: &Path) -> Option<Vec<OsString>> {
    let bin = preset.bins.iter().copied().find(|name| exists(name))?;
    let mut parts = vec![OsString::from(bin)];
    parts.extend(preset.args.iter().copied().map(OsString::from));
    parts.push(path.into());
    if preset.terminal {
        Some(in_terminal(parts, terminal_env, exists))
    } else {
        Some(parts)
    }
}

fn custom_cmd(line: &str, terminal_env: Option<&str>, exists: &impl Fn(&str) -> bool, path: &Path) -> Vec<OsString> {
    let parts = with_file(command_words(line), path);
    if needs_terminal(&parts) {
        in_terminal(parts, terminal_env, exists)
    } else {
        parts
    }
}

fn needs_terminal(parts: &[OsString]) -> bool {
    let Some(bin) = parts.first().and_then(|bin| Path::new(bin).file_name()).and_then(|name| name.to_str()) else {
        return false;
    };
    if TERMINAL_EDITORS.contains(&bin) || PRESETS.iter().any(|preset| preset.terminal && preset.bins.contains(&bin)) {
        return true;
    }
    matches!(bin, "emacs" | "emacsclient")
        && parts[1..].iter().any(|arg| matches!(arg.to_str(), Some("-nw" | "-t" | "--tty" | "--no-window-system")))
}

fn in_terminal(cmd: Vec<OsString>, terminal_env: Option<&str>, exists: &impl Fn(&str) -> bool) -> Vec<OsString> {
    let chosen = terminal_env
        .map(words)
        .filter(|parts| !parts.is_empty())
        .or_else(|| TERMINALS.iter().copied().find(|name| exists(name)).map(|name| vec![OsString::from(name)]));
    let Some(mut parts) = chosen else {
        return cmd;
    };
    let bin = Path::new(&parts[0]).file_name().and_then(|name| name.to_str()).unwrap_or("").to_string();
    // $TERMINAL may already end with its exec flag.
    let flagged = parts.len() > 1 && parts.last().is_some_and(|last| matches!(last.to_str(), Some("-e" | "-x" | "--")));
    if !flagged {
        match bin.as_str() {
            "gnome-terminal" => parts.push("--".into()),
            "wezterm" => parts.extend(["start".into(), "--".into()]),
            // -e takes one string there, so the file would be lost.
            "xfce4-terminal" | "terminator" => parts.push("-x".into()),
            "kitty" | "foot" | "xdg-terminal-exec" => {}
            _ => parts.push("-e".into()),
        }
    }
    parts.extend(cmd);
    parts
}

fn words(line: &str) -> Vec<OsString> {
    line.split_whitespace().map(OsString::from).collect()
}

// A program picked with "Other…" is stored as its path. A space in a folder name must not split it.
fn command_words(line: &str) -> Vec<OsString> {
    let whole = Path::new(line);
    if whole.is_absolute() && whole.is_file() {
        vec![line.into()]
    } else {
        words(line)
    }
}

fn with_file(mut parts: Vec<OsString>, path: &Path) -> Vec<OsString> {
    if parts.is_empty() {
        return Vec::new();
    }
    parts.push(path.into());
    parts
}

fn argv(configured: &str, env: Option<&str>, fallback: &str, path: &Path) -> Vec<OsString> {
    let chosen = if !configured.trim().is_empty() {
        configured
    } else if let Some(env) = env.filter(|s| !s.trim().is_empty()) {
        env
    } else {
        fallback
    };
    let mut parts = words(chosen);
    if parts.is_empty() {
        parts.push(fallback.into());
    }
    parts.push(path.into());
    parts
}

pub fn terminal_start_with(
    terminal_env: Option<&str>,
    exists: impl Fn(&str) -> bool,
    dir: &Path,
) -> Option<Vec<String>> {
    let chosen = terminal_env
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| TERMINALS.iter().copied().find(|name| exists(name)).map(str::to_string))?;
    let mut parts: Vec<String> = chosen.split_whitespace().map(str::to_string).collect();
    let bin = parts.first()?.clone();
    if bin == "gnome-terminal" || bin.ends_with("/gnome-terminal") {
        parts.push("--working-directory".into());
        parts.push(dir.display().to_string());
    }
    Some(parts)
}

fn program_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(name)).find(|file| file.is_file())
}

fn program_on_path(name: &str) -> bool {
    program_path(name).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(parts: &[OsString]) -> Vec<&str> {
        parts.iter().filter_map(|part| part.to_str()).collect()
    }

    #[test]
    fn terminal_editor_from_env_gets_a_terminal() {
        let file = Path::new("/tmp/a");
        let none = |_: &str| false;
        let xdg = |name: &str| name == "xdg-terminal-exec";
        assert_eq!(strs(&editor_argv_in("", Some("nvim"), None, xdg, file)), ["xdg-terminal-exec", "nvim", "/tmp/a"]);
        assert_eq!(strs(&editor_argv_in("", Some("vim -p"), Some("alacritty"), none, file)), ["alacritty", "-e", "vim", "-p", "/tmp/a"]);
        assert_eq!(strs(&editor_argv_in("", Some("/usr/bin/hx"), Some("wezterm"), none, file)), ["wezterm", "start", "--", "/usr/bin/hx", "/tmp/a"]);
        assert_eq!(strs(&editor_argv_in("", Some("nano"), Some("wezterm start --"), none, file)), ["wezterm", "start", "--", "nano", "/tmp/a"]);
        assert_eq!(strs(&editor_argv_in("", Some("emacs -nw"), Some("ghostty"), none, file)), ["ghostty", "-e", "emacs", "-nw", "/tmp/a"]);
        let xfce = |name: &str| matches!(name, "vim" | "xfce4-terminal");
        assert_eq!(strs(&editor_argv_in("vim", None, None, xfce, file)), ["xfce4-terminal", "-x", "vim", "/tmp/a"]);
        assert_eq!(strs(&editor_argv_in("", Some("emacs"), Some("foot"), none, file)), ["emacs", "/tmp/a"]);
        assert_eq!(strs(&editor_argv_in("", Some("code --wait"), Some("foot"), none, file)), ["code", "--wait", "/tmp/a"]);
    }

    #[test]
    fn picked_program_with_a_space_stays_whole() {
        let dir = std::env::temp_dir().join(format!("rust-commander-external-{}", std::process::id()));
        let bin = dir.join("Sublime Text/sublime_text");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, b"").unwrap();
        let line = bin.to_str().unwrap();
        assert_eq!(strs(&editor_argv_with(line, None, |_| false, Path::new("/tmp/a"))), [line, "/tmp/a"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

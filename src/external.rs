use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

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

const TERMINALS: &[&str] = &[
    "x-terminal-emulator",
    "kitty",
    "foot",
    "alacritty",
    "gnome-terminal",
    "konsole",
    "xfce4-terminal",
    "xterm",
];

pub fn viewer_argv(configured: &str, path: &Path) -> Vec<String> {
    argv(configured, None, "xdg-open", path)
}

// Desktop Exec field codes. Flatpak VLC drops a file handed to a running player, so that flag is skipped.
pub fn desktop_argv(exec: &str, path: &Path) -> Vec<String> {
    let uri = file_uri(path);
    let path_text = path.display().to_string();
    let mut out = Vec::new();
    for arg in split_exec(exec) {
        if arg == "--started-from-file" {
            continue;
        }
        match arg.as_str() {
            "%f" | "%F" => out.push(path_text.clone()),
            "%u" | "%U" => out.push(uri.clone()),
            "%i" | "%c" | "%k" => {}
            other => {
                let other = other.replace("%%", "%");
                if !other.is_empty() {
                    out.push(other);
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
    for b in path.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn editor_argv(configured: &str, editor_env: Option<&str>, path: &Path) -> Vec<String> {
    editor_argv_with(configured, editor_env, program_on_path, path)
}

pub fn editor_argv_with(
    configured: &str,
    editor_env: Option<&str>,
    exists: impl Fn(&str) -> bool,
    path: &Path,
) -> Vec<String> {
    let configured = configured.trim();
    if !configured.is_empty() && configured != EDITOR_BROWSE {
        if let Some(preset) = PRESETS.iter().find(|preset| preset.id == configured) {
            return preset_cmd(preset, &exists, path).unwrap_or_default();
        }
        return with_file(words(configured), path);
    }
    if let Some(env) = editor_env.map(str::trim).filter(|line| !line.is_empty()) {
        return with_file(words(env), path);
    }
    auto_preset(&exists)
        .and_then(|preset| preset_cmd(preset, &exists, path))
        .unwrap_or_default()
}

pub fn editor_choices(configured: &str) -> Vec<EditorChoice> {
    let env = std::env::var("EDITOR").ok();
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

fn preset_cmd(preset: &Preset, exists: &impl Fn(&str) -> bool, path: &Path) -> Option<Vec<String>> {
    let bin = preset.bins.iter().copied().find(|name| exists(name))?;
    let mut parts = vec![bin.to_string()];
    parts.extend(preset.args.iter().copied().map(str::to_string));
    parts.push(path.display().to_string());
    if preset.terminal {
        Some(in_terminal(parts, exists))
    } else {
        Some(parts)
    }
}

fn in_terminal(cmd: Vec<String>, exists: &impl Fn(&str) -> bool) -> Vec<String> {
    let Some(term) = TERMINALS.iter().copied().find(|name| exists(name)) else {
        return cmd;
    };
    let mut parts = vec![term.to_string()];
    match term {
        "gnome-terminal" => parts.push("--".into()),
        "kitty" | "foot" => {}
        _ => parts.push("-e".into()),
    }
    parts.extend(cmd);
    parts
}

fn words(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_string).collect()
}

fn with_file(mut parts: Vec<String>, path: &Path) -> Vec<String> {
    if parts.is_empty() {
        return Vec::new();
    }
    parts.push(path.display().to_string());
    parts
}

fn argv(configured: &str, env: Option<&str>, fallback: &str, path: &Path) -> Vec<String> {
    let chosen = if !configured.trim().is_empty() {
        configured
    } else if let Some(env) = env.filter(|s| !s.trim().is_empty()) {
        env
    } else {
        fallback
    };
    let mut parts: Vec<String> = chosen.split_whitespace().map(str::to_string).collect();
    if parts.is_empty() {
        parts.push(fallback.to_string());
    }
    parts.push(path.display().to_string());
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

pub fn terminal_start(dir: &Path) -> Option<Vec<String>> {
    let env = std::env::var("TERMINAL").ok();
    terminal_start_with(env.as_deref(), program_on_path, dir)
}

pub fn run_shell(dir: &Path, line: &str) -> io::Result<String> {
    let out = Command::new("sh").arg("-c").arg(line).current_dir(dir).output()?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    let err = String::from_utf8_lossy(&out.stderr);
    if !err.is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&err);
    }
    if !out.status.success() {
        let code = out.status.code().map(|n| n.to_string()).unwrap_or_else(|| "signal".into());
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&format!("exit {code}"));
    }
    Ok(text)
}

fn program_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(name)).find(|file| file.is_file())
}

fn program_on_path(name: &str) -> bool {
    program_path(name).is_some()
}

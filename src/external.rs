use std::io;
use std::path::Path;
use std::process::Command;

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

pub fn editor_argv(configured: &str, editor_env: Option<&str>, path: &Path) -> Vec<String> {
    argv(configured, editor_env, "xdg-open", path)
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

fn program_on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(name).is_file())
}

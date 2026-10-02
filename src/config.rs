use std::borrow::Cow;
use std::ffi::{CStr, OsStr};
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, Once};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::listing::SortKey;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Bookmark {
    pub name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub target: PathBuf,
    #[serde(default)]
    pub kind: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default)]
    pub show_hidden: bool,
    #[serde(default = "default_sort")]
    pub sort: String,
    #[serde(default)]
    pub viewer: String,
    #[serde(default)]
    pub editor: String,
    #[serde(default)]
    pub bookmarks: Vec<Bookmark>,
    #[serde(default = "default_true")]
    pub auto_columns: bool,
    #[serde(default)]
    pub column_widths: Vec<i32>,
    #[serde(default)]
    pub background: String,
    #[serde(default)]
    pub background_color: String,
    #[serde(default = "default_opaque")]
    pub background_alpha: u8,
    #[serde(default = "default_true")]
    pub background_stretch: bool,
    #[serde(default)]
    pub seven_zip: String,
    #[serde(default)]
    pub rar: String,
    #[serde(default = "default_true")]
    pub mode_buttons: bool,
    #[serde(default)]
    pub file_icons: bool,
    #[serde(default = "default_icon_pack")]
    pub icon_pack: String,
    #[serde(default = "default_true")]
    pub flash_feedback: bool,
    #[serde(default = "default_true")]
    pub syntax_highlight: bool,
}

fn default_theme() -> String {
    "tokyo-night".into()
}

fn default_icon_pack() -> String {
    crate::icons::AWESOME.into()
}

fn default_true() -> bool {
    true
}

fn default_sort() -> String {
    "name".into()
}

fn default_opaque() -> u8 {
    100
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            show_hidden: false,
            sort: default_sort(),
            viewer: String::new(),
            editor: String::new(),
            bookmarks: Vec::new(),
            auto_columns: true,
            column_widths: Vec::new(),
            background: String::new(),
            background_color: String::new(),
            background_alpha: default_opaque(),
            background_stretch: true,
            seven_zip: String::new(),
            rar: String::new(),
            mode_buttons: true,
            file_icons: false,
            icon_pack: default_icon_pack(),
            flash_feedback: true,
            syntax_highlight: true,
        }
    }
}

impl Config {
    pub fn sort_key(&self) -> SortKey {
        SortKey::from_config(&self.sort)
    }
}

pub fn config_dir() -> PathBuf {
    let dir = xdg_dir("XDG_CONFIG_HOME", ".config").unwrap_or_else(|| {
        static WARN: Once = Once::new();
        WARN.call_once(|| crate::log::error("no home directory; config is kept in ./.config"));
        PathBuf::from(".config")
    });
    dir.join("rust-commander")
}

// The XDG spec says a relative value is ignored.
pub fn xdg_dir(var: &str, under_home: &str) -> Option<PathBuf> {
    match std::env::var_os(var).map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => Some(dir),
        _ => Some(home_dir()?.join(under_home)),
    }
}

// $HOME, else the passwd entry, so a service started with no HOME does not write into its cwd.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).filter(|home| home.is_absolute()).or_else(passwd_home)
}

fn passwd_home() -> Option<PathBuf> {
    let mut buf = vec![0u8; 1024];
    loop {
        let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found = std::ptr::null_mut();
        let rc = unsafe { libc::getpwuid_r(libc::getuid(), &mut pw, buf.as_mut_ptr().cast(), buf.len(), &mut found) };
        if rc == libc::ERANGE && buf.len() < 1 << 20 {
            buf.resize(buf.len() * 2, 0);
            continue;
        }
        if rc != 0 || found.is_null() || pw.pw_dir.is_null() {
            return None;
        }
        let dir = unsafe { CStr::from_ptr(pw.pw_dir) };
        return Some(PathBuf::from(OsStr::from_bytes(dir.to_bytes()))).filter(|dir| dir.is_absolute());
    }
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn active_theme_path() -> PathBuf {
    config_dir().join("active-theme")
}

pub fn load() -> Config {
    let (cfg, usable) = read_config(&config_path());
    KEEP_FILE.store(!usable, Ordering::Relaxed);
    *base() = Some(cfg.clone());
    cfg
}

// What this process last read or wrote. Several windows can run, each with its own copy.
static BASE: Mutex<Option<Config>> = Mutex::new(None);

// config.toml is there but could not be used. Saving now would replace it with defaults.
static KEEP_FILE: AtomicBool = AtomicBool::new(false);

fn base() -> MutexGuard<'static, Option<Config>> {
    BASE.lock().unwrap_or_else(|err| err.into_inner())
}

// Strict, so a file that is half edited by hand is not merged in.
fn read_strict(path: &Path) -> Option<Config> {
    toml::from_str(&fs::read_to_string(path).ok()?).ok()
}

// Keys this window changed win. The rest keep what another window saved meanwhile.
fn merge(base: &Config, ours: &Config, disk: &Config) -> Config {
    let tables = (toml::Table::try_from(base), toml::Table::try_from(ours), toml::Table::try_from(disk));
    let (Ok(base), Ok(changed), Ok(mut out)) = tables else {
        return ours.clone();
    };
    for (key, value) in changed {
        if base.get(&key) != Some(&value) {
            out.insert(key, value);
        }
    }
    out.try_into().unwrap_or_else(|_| ours.clone())
}

/// Takes the bookmarks from disk, so an edit here does not drop one another window added.
pub fn reload_bookmarks(cfg: &mut Config) {
    let Some(disk) = read_strict(&config_path()) else { return };
    let mut base = base();
    // Unsaved edits here stay.
    if let Some(old) = base.as_mut().filter(|old| old.bookmarks == cfg.bookmarks) {
        old.bookmarks = disk.bookmarks.clone();
        cfg.bookmarks = disk.bookmarks;
    }
}

pub fn load_from(path: &Path) -> Config {
    read_config(path).0
}

// false: the file is there but gave nothing back, so it must not be saved over.
fn read_config(path: &Path) -> (Config, bool) {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return (Config::default(), true),
        Err(err) => {
            crate::log::error(&format!("config {}: {err}; it is not saved over until it can be read", path.display()));
            backup(path);
            return (Config::default(), false);
        }
    };
    let err = match toml::from_str(&text) {
        Ok(cfg) => return (cfg, true),
        Err(err) => err,
    };
    // One bad value must not wipe the bookmarks on the next save. Keep a copy and drop only the bad keys.
    crate::log::error(&format!("config {}: {err}", path.display()));
    backup(path);
    let Ok(mut table) = toml::from_str::<toml::Table>(&text) else {
        return (Config::default(), false);
    };
    if let Some(toml::Value::Array(marks)) = table.get_mut("bookmarks") {
        marks.retain(|mark| mark.clone().try_into::<Bookmark>().is_ok());
    }
    table.retain(|key, value| {
        let mut one = toml::Table::new();
        one.insert(key.to_string(), value.clone());
        one.try_into::<Config>().is_ok()
    });
    match table.try_into() {
        Ok(cfg) => (cfg, true),
        Err(_) => (Config::default(), false),
    }
}

// config.toml.bad-<unix seconds>. An earlier copy is never replaced.
fn backup(path: &Path) {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
    let kept = (0..100).find_map(|n| {
        let mut name = path.as_os_str().to_os_string();
        name.push(if n == 0 { format!(".bad-{secs}") } else { format!(".bad-{secs}.{n}") });
        let dest = PathBuf::from(name);
        match copy_new(path, &dest) {
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => None,
            done => Some(done.map(|()| dest)),
        }
    });
    let msg = match kept {
        Some(Ok(dest)) => format!("config {}: copy kept as {}", path.display(), dest.display()),
        Some(Err(err)) => format!("config {}: no copy kept: {err}", path.display()),
        None => format!("config {}: no copy kept: no free name", path.display()),
    };
    crate::log::error(&msg);
}

fn copy_new(from: &Path, to: &Path) -> io::Result<()> {
    let mut src = fs::File::open(from)?;
    let mut out = fs::OpenOptions::new().write(true).create_new(true).open(to)?;
    io::copy(&mut src, &mut out).map(drop)
}

pub fn save(cfg: &Config) -> io::Result<()> {
    let saved = save_merged(cfg);
    if let Err(err) = &saved {
        crate::log::error(&format!("config save: {err}"));
    }
    saved
}

fn save_merged(cfg: &Config) -> io::Result<()> {
    let path = config_path();
    if KEEP_FILE.load(Ordering::Relaxed) {
        return Err(io::Error::other(format!("{} could not be read at start, so it is not saved over", path.display())));
    }
    // Strict, so a file that is half edited by hand is not merged in. One that cannot be read is not replaced.
    let disk = match fs::read_to_string(&path) {
        Ok(text) => toml::from_str::<Config>(&text).ok(),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Err(err),
    };
    let cfg = printable(cfg);
    let mut base = base();
    let merged = match (base.as_ref(), disk) {
        (Some(old), Some(disk)) => merge(old, &cfg, &disk),
        _ => Config::clone(&cfg),
    };
    save_to(&path, &merged)?;
    *base = Some(cfg.into_owned());
    Ok(())
}

pub fn save_to(path: &Path, cfg: &Config) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(&*printable(cfg)).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    crate::ops::write_atomic(path, text.as_bytes())
}

// TOML has no way to write a path that is not UTF-8, and one such bookmark would fail every save.
fn printable(cfg: &Config) -> Cow<'_, Config> {
    let utf8 = |mark: &Bookmark| mark.path.to_str().is_some() && mark.target.to_str().is_some();
    if cfg.bookmarks.iter().all(utf8) {
        return Cow::Borrowed(cfg);
    }
    let mut out = cfg.clone();
    out.bookmarks.retain(|mark| {
        if !utf8(mark) {
            crate::log::error(&format!("bookmark {:?} not saved: its path is not UTF-8", mark.path));
        }
        utf8(mark)
    });
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_flash_defaults_to_enabled() {
        assert!(Config::default().flash_feedback);
        let old_config: Config = toml::from_str("").unwrap();
        assert!(old_config.flash_feedback);
        let disabled: Config = toml::from_str("flash_feedback = false").unwrap();
        assert!(!disabled.flash_feedback);
    }

    #[test]
    fn background_stretch_defaults_to_enabled() {
        assert!(Config::default().background_stretch);
        let old_config: Config = toml::from_str("").unwrap();
        assert!(old_config.background_stretch);
        let off: Config = toml::from_str("background_stretch = false").unwrap();
        assert!(!off.background_stretch);
    }

    #[test]
    fn save_keeps_what_another_window_changed() {
        let base = Config::default();
        let mark = Bookmark { name: "w".into(), path: "/w".into(), target: PathBuf::new(), kind: String::new() };
        let ours = Config { sort: "time".into(), ..base.clone() };
        let disk = Config { bookmarks: vec![mark.clone()], theme: "other".into(), ..base.clone() };
        let merged = merge(&base, &ours, &disk);
        assert_eq!(merged.sort, "time");
        assert_eq!(merged.theme, "other");
        assert_eq!(merged.bookmarks, vec![mark]);
        let ours = Config { theme: "mine".into(), ..base.clone() };
        assert_eq!(merge(&base, &ours, &disk).theme, "mine");
    }
}

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

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
    #[serde(default)]
    pub seven_zip: String,
    #[serde(default)]
    pub rar: String,
    #[serde(default = "default_true")]
    pub mode_buttons: bool,
    #[serde(default)]
    pub file_icons: bool,
    #[serde(default = "default_true")]
    pub flash_feedback: bool,
}

fn default_theme() -> String {
    "tokyo-night".into()
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
            seven_zip: String::new(),
            rar: String::new(),
            mode_buttons: true,
            file_icons: false,
            flash_feedback: true,
        }
    }
}

impl Config {
    pub fn sort_key(&self) -> SortKey {
        SortKey::from_config(&self.sort)
    }
}

pub fn config_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".config/rust-commander")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn active_theme_path() -> PathBuf {
    config_dir().join("active-theme")
}

pub fn load() -> Config {
    let cfg = load_from(&config_path());
    *base() = Some(cfg.clone());
    cfg
}

// What this process last read or wrote. Several windows can run, each with its own copy.
static BASE: Mutex<Option<Config>> = Mutex::new(None);

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
    let Ok(text) = fs::read_to_string(path) else {
        return Config::default();
    };
    let err = match toml::from_str(&text) {
        Ok(cfg) => return cfg,
        Err(err) => err,
    };
    // One bad value must not wipe the bookmarks on the next save. Keep a copy and drop only the bad keys.
    let _ = fs::copy(path, path.with_extension("toml.bad"));
    crate::log::error(&format!("config {}: {err}", path.display()));
    let Ok(mut table) = toml::from_str::<toml::Table>(&text) else {
        return Config::default();
    };
    table.retain(|key, value| {
        let mut one = toml::Table::new();
        one.insert(key.to_string(), value.clone());
        one.try_into::<Config>().is_ok()
    });
    table.try_into().unwrap_or_default()
}

pub fn save(cfg: &Config) -> io::Result<()> {
    let path = config_path();
    let mut base = base();
    let merged = match (base.as_ref(), read_strict(&path)) {
        (Some(old), Some(disk)) => merge(old, cfg, &disk),
        _ => cfg.clone(),
    };
    save_to(&path, &merged)?;
    *base = Some(cfg.clone());
    Ok(())
}

pub fn save_to(path: &Path, cfg: &Config) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(cfg).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    crate::ops::write_atomic(path, text.as_bytes())
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

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::listing::SortKey;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Bookmark {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_true")]
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

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            show_hidden: true,
            sort: default_sort(),
            viewer: String::new(),
            editor: String::new(),
            bookmarks: Vec::new(),
            auto_columns: true,
            column_widths: Vec::new(),
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
    home.join(".config/comarchy")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn active_theme_path() -> PathBuf {
    config_dir().join("active-theme")
}

pub fn load() -> Config {
    load_from(&config_path())
}

pub fn load_from(path: &Path) -> Config {
    let Ok(text) = fs::read_to_string(path) else {
        return Config::default();
    };
    toml::from_str(&text).unwrap_or_default()
}

pub fn save(cfg: &Config) -> io::Result<()> {
    save_to(&config_path(), cfg)
}

pub fn save_to(path: &Path, cfg: &Config) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(cfg).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(path, text)
}

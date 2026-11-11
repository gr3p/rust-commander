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
    #[serde(default = "default_true")]
    pub background_stretch: bool,
    #[serde(default)]
    pub seven_zip: String,
    #[serde(default)]
    pub rar: String,
    #[serde(default = "default_true")]
    pub mode_buttons: bool,
    #[serde(default = "default_true")]
    pub command_bar: bool,
    #[serde(default = "default_true")]
    pub file_icons: bool,
    #[serde(default = "default_icon_pack")]
    pub icon_pack: String,
    #[serde(default = "default_true")]
    pub flash_feedback: bool,
    #[serde(default = "default_true")]
    pub syntax_highlight: bool,
    #[serde(default)]
    pub cursor_color: String,
    #[serde(default)]
    pub cursor_text_color: String,
    #[serde(default)]
    pub mark_color: String,
    #[serde(default)]
    pub blink_color: String,
    /// How fast a blink pulses. 50 is the original speed.
    #[serde(default = "default_blink_speed")]
    pub blink_speed: u8,
    /// How many times the ring flashes. 3 is the original.
    #[serde(default = "default_blink_count")]
    pub blink_count: u8,
    #[serde(default = "default_cursor_alpha")]
    pub cursor_alpha: u8,
    #[serde(default)]
    pub cursor_frame: bool,
    /// The music player's visualizer: bars, wave, radial or ribbon.
    #[serde(default = "default_visual")]
    pub player_visual: String,
    #[serde(default = "default_true")]
    pub player_visual_on: bool,
    /// How hard the sound moves the picture. 50 is the original strength.
    #[serde(default = "default_visual_mid")]
    pub player_visual_strength: u8,
    /// How slowly the picture falls back. 50 is the original fall.
    #[serde(default = "default_visual_mid")]
    pub player_visual_smooth: u8,
    /// How visible the picture is behind the text. 50 is the original density.
    #[serde(default = "default_visual_mid")]
    pub player_visual_density: u8,
    /// What the player does after the last track: off, all or one.
    #[serde(default = "default_repeat")]
    pub player_repeat: String,
    #[serde(default)]
    pub player_shuffle: bool,
}

fn default_theme() -> String {
    crate::theme::OMARCHY.into()
}

fn default_icon_pack() -> String {
    crate::theme::OMARCHY.into()
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

fn default_cursor_alpha() -> u8 {
    70
}

fn default_blink_speed() -> u8 {
    50
}

fn default_blink_count() -> u8 {
    3
}

fn default_visual() -> String {
    crate::audio::VisualStyle::default().name().into()
}

fn default_visual_mid() -> u8 {
    50
}

fn default_repeat() -> String {
    crate::audio::Repeat::default().name().into()
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
            command_bar: true,
            file_icons: true,
            icon_pack: default_icon_pack(),
            flash_feedback: true,
            syntax_highlight: true,
            cursor_color: String::new(),
            cursor_text_color: String::new(),
            mark_color: String::new(),
            blink_color: String::new(),
            blink_speed: default_blink_speed(),
            blink_count: default_blink_count(),
            cursor_alpha: default_cursor_alpha(),
            cursor_frame: false,
            player_visual: default_visual(),
            player_visual_on: true,
            player_visual_strength: default_visual_mid(),
            player_visual_smooth: default_visual_mid(),
            player_visual_density: default_visual_mid(),
            player_repeat: default_repeat(),
            player_shuffle: false,
        }
    }
}

impl Config {
    // Colors set by the user on top of the theme. An empty value follows the theme.
    pub fn color_overrides(&self) -> Vec<&'static str> {
        [
            ("background color", &self.background_color),
            ("cursor color", &self.cursor_color),
            ("cursor text color", &self.cursor_text_color),
            ("mark color", &self.mark_color),
            ("blink color", &self.blink_color),
        ]
        .into_iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(name, _)| name)
        .collect()
    }

    pub fn sort_key(&self) -> SortKey {
        SortKey::from_config(&self.sort)
    }

    pub fn visual_style(&self) -> crate::audio::VisualStyle {
        crate::audio::VisualStyle::from_name(&self.player_visual)
    }

    pub fn repeat(&self) -> crate::audio::Repeat {
        crate::audio::Repeat::from_name(&self.player_repeat)
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
    fn blink_speed_defaults_to_today() {
        assert_eq!(Config::default().blink_speed, 50);
        let old: Config = toml::from_str("").unwrap();
        assert_eq!(old.blink_speed, 50);
        let set: Config = toml::from_str("blink_speed = 80").unwrap();
        assert_eq!(set.blink_speed, 80);
        assert_eq!(Config::default().blink_count, 3);
        assert_eq!(old.blink_count, 3);
        let count: Config = toml::from_str("blink_count = 5").unwrap();
        assert_eq!(count.blink_count, 5);
    }

    #[test]
    fn confirmation_flash_defaults_to_enabled() {
        assert!(Config::default().flash_feedback);
        let old_config: Config = toml::from_str("").unwrap();
        assert!(old_config.flash_feedback);
        let disabled: Config = toml::from_str("flash_feedback = false").unwrap();
        assert!(!disabled.flash_feedback);
    }

    #[test]
    fn cursor_frame_defaults_off() {
        assert!(!Config::default().cursor_frame);
        let old_config: Config = toml::from_str("").unwrap();
        assert!(!old_config.cursor_frame);
        let on: Config = toml::from_str("cursor_frame = true").unwrap();
        assert!(on.cursor_frame);
    }

    #[test]
    fn the_player_remembers_its_modes_and_defaults_for_old_configs() {
        let old: Config = toml::from_str("").unwrap();
        assert!(old.player_visual_on);
        assert_eq!(old.player_visual_strength, 50);
        assert_eq!(old.player_visual_smooth, 50);
        assert_eq!(old.player_visual_density, 50);
        assert_eq!(old.visual_style(), crate::audio::VisualStyle::Bars);
        assert_eq!(old.repeat(), crate::audio::Repeat::Off);
        assert!(!old.player_shuffle);
        let set: Config =
            toml::from_str("player_visual = \"radial\"\nplayer_visual_on = false\nplayer_repeat = \"all\"\nplayer_shuffle = true").unwrap();
        assert_eq!(set.visual_style(), crate::audio::VisualStyle::Radial);
        assert!(!set.player_visual_on);
        assert_eq!(set.repeat(), crate::audio::Repeat::All);
        assert!(set.player_shuffle);
        // A name this version does not know must not break loading.
        let odd: Config = toml::from_str("player_visual = \"lava\"\nplayer_repeat = \"sometimes\"").unwrap();
        assert_eq!(odd.visual_style(), crate::audio::VisualStyle::Bars);
        assert_eq!(odd.repeat(), crate::audio::Repeat::Off);
    }

    #[test]
    fn a_saved_player_mode_survives_another_window_saving() {
        let base = Config::default();
        let ours = Config { player_repeat: "one".into(), player_shuffle: true, ..base.clone() };
        let disk = Config { theme: "other".into(), ..base.clone() };
        let merged = merge(&base, &ours, &disk);
        assert_eq!(merged.repeat(), crate::audio::Repeat::One);
        assert!(merged.player_shuffle);
        assert_eq!(merged.theme, "other");
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

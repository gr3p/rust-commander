use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    pub background: String,
    pub foreground: String,
    pub accent: String,
    pub selection: String,
    pub muted: String,
    pub bright: String,
}

pub fn builtin() -> Palette {
    Palette {
        background: "#1a1b26".into(),
        foreground: "#a9b1d6".into(),
        accent: "#7aa2f7".into(),
        selection: "#292e42".into(),
        muted: "#414868".into(),
        bright: "#c0caf5".into(),
    }
}

pub fn palette_from_optional(text: Option<&str>) -> Palette {
    text.and_then(from_colors_toml).unwrap_or_else(builtin)
}

pub fn from_colors_toml(text: &str) -> Option<Palette> {
    let table: toml::Table = toml::from_str(text).ok()?;
    Some(Palette {
        background: color(&table, "background")?,
        foreground: color(&table, "foreground")?,
        accent: color(&table, "accent").or_else(|| color(&table, "blue"))?,
        selection: color(&table, "selection")?,
        muted: color(&table, "muted")?,
        bright: color(&table, "bright_foreground").unwrap_or_else(|| "#c0caf5".into()),
    })
}

fn color(table: &toml::Table, key: &str) -> Option<String> {
    let s = table.get(key)?.as_str()?.trim().to_string();
    if s.starts_with('#') && (s.len() == 7 || s.len() == 4) {
        Some(s)
    } else {
        None
    }
}

pub fn active_name(config_theme: &str, active_file: &Path) -> String {
    if let Ok(text) = fs::read_to_string(active_file) {
        let name = text.trim();
        if !name.is_empty() {
            return name.to_string();
        }
    }
    if config_theme.is_empty() {
        "tokyo-night".into()
    } else {
        config_theme.to_string()
    }
}

pub fn omarchy_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/usr/share/omarchy/themes")];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".config/omarchy/themes"));
    }
    dirs
}

pub fn load_named(name: &str) -> Palette {
    for dir in omarchy_dirs() {
        let path = dir.join(name).join("colors.toml");
        if let Ok(text) = fs::read_to_string(path) {
            if let Some(palette) = from_colors_toml(&text) {
                return palette;
            }
        }
    }
    builtin()
}

pub fn stylesheet(p: &Palette) -> String {
    format!(
        r#"
window,
window.csd,
decoration,
headerbar,
.titlebar,
frame,
frame > border,
.pane,
.pane-active,
entry,
button,
listview,
listview > row,
scrolledwindow {{
  border-radius: 0;
}}
window {{
  background: {bg};
  color: {fg};
}}
.pane {{
  border: 1px solid {muted};
}}
.pane-active {{
  border: 1px solid {accent};
}}
.marked {{
  color: {accent};
}}
listview > row:selected,
listview row:selected {{
  background: {sel};
  color: {bright};
}}
.fbar, .status {{
  background: {bg};
  color: {fg};
}}
entry {{
  background: {sel};
  color: {fg};
  caret-color: {bright};
}}
entry.path {{
  background: {bg};
  border: none;
  border-bottom: 1px solid {muted};
}}
columnview > header > button {{
  background: transparent;
  color: {fg};
  border-radius: 0;
  box-shadow: none;
  padding: 2px 8px;
  min-height: 0;
}}
textview, textview text {{
  background: {bg};
  color: {fg};
}}
button.fkey {{
  background: {sel};
  color: {fg};
}}
"#,
        bg = p.background,
        fg = p.foreground,
        accent = p.accent,
        muted = p.muted,
        sel = p.selection,
        bright = p.bright,
    )
}

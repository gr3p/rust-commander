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

pub fn css_file_url(path: &str) -> String {
    let mut out = String::from("file://");
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn hex_color(color: &str) -> Option<&str> {
    let color = color.trim();
    if color.len() == 7 && color.starts_with('#') && color.bytes().skip(1).all(|b| b.is_ascii_hexdigit()) {
        Some(color)
    } else {
        None
    }
}

// Image strength 0 is the solid color. 100 is the picture alone.
pub fn list_background(p: &Palette, color: &str, image: &str, strength: u8) -> String {
    let chosen = hex_color(color);
    let strength = strength.min(100);
    let image = image.trim();
    let show_image = strength > 0 && !image.is_empty();
    if !show_image && chosen.is_none() {
        return String::new();
    }
    let fill = chosen.unwrap_or(p.background.as_str());
    let selected = format!(
        "columnview > listview > row:selected,\ncolumnview > listview > row:selected > cell {{\n  background: {sel};\n  color: {bright};\n}}\n",
        sel = p.selection,
        bright = p.bright,
    );
    if !show_image {
        return format!(
            "columnview,\ncolumnview > listview,\ncolumnview > listview > row,\ncolumnview > listview > row > cell {{\n  background: {fill};\n}}\n{selected}"
        );
    }
    let url = css_file_url(image);
    let veil = f64::from(100 - strength) / 100.0;
    let (picture, size, at, repeat) = if veil <= 0.0 {
        (format!("url(\"{url}\")"), "cover", "center", "no-repeat")
    } else {
        (
            format!("linear-gradient(alpha({fill}, {veil:.2}), alpha({fill}, {veil:.2})), url(\"{url}\")"),
            "cover, cover",
            "center, center",
            "no-repeat, no-repeat",
        )
    };
    format!(
        "columnview {{\n  background: {fill};\n}}\ncolumnview > listview {{\n  background-color: {fill};\n  background-image: {picture};\n  background-size: {size};\n  background-position: {at};\n  background-repeat: {repeat};\n}}\ncolumnview > listview > row,\ncolumnview > listview > row > cell {{\n  background: transparent;\n}}\n{selected}"
    )
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
  color: #9ece6a;
}}
label.glyph.marked {{
  color: {accent};
}}
label.glyph {{
  font-size: 8px;
  min-width: 0;
  padding: 0;
  margin: 0;
  min-height: 0;
}}
label.glyph.cursor {{
  color: #ffffff;
  font-size: 13px;
}}
columnview > listview > row > cell:first-child {{
  padding: 0;
}}
columnview > header > button:first-child {{
  min-width: 0;
  min-height: 0;
  padding: 0;
  margin: 0;
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
label.path {{
  padding: 2px 8px;
  border-bottom: 1px solid {muted};
}}
columnview > listview > row > cell {{
  padding: 0 6px;
  min-height: 0;
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
button.drive {{
  background: transparent;
  padding: 10px 4px;
  min-width: 0;
  min-height: 0;
}}
button.drive label {{
  font-size: 11px;
}}
button.drive:hover {{
  background: {sel};
}}
button.fkey {{
  background: {sel};
  color: {fg};
  min-width: 0;
  padding: 4px 2px;
}}
button.fkey label {{
  min-width: 0;
}}
.settings-row {{
  padding: 10px 12px;
}}
.settings-row:hover {{
  background: {sel};
}}
label.settings-on {{
  color: {accent};
}}
label.cmd-key {{
  font-family: monospace;
  color: {accent};
}}
label.cmd-head {{
  font-weight: 700;
  padding-top: 14px;
}}
dropdown.editor-pick > button {{
  background: {sel};
  color: {fg};
  padding: 2px 8px;
  min-height: 0;
}}
dropdown.editor-pick popover,
dropdown.editor-pick popover listview,
dropdown.editor-pick popover listview > row {{
  background: {bg};
  color: {fg};
}}
dropdown.editor-pick popover listview > row:selected {{
  background: {sel};
  color: {bright};
}}
button.hot {{
  background: transparent;
  color: {fg};
  border: none;
  border-radius: 0;
  box-shadow: none;
  padding: 4px 12px;
  min-height: 0;
  min-width: 0;
}}
button.hot:hover, button.hot:focus {{
  background: {sel};
}}
button.hot-on {{
  background: {sel};
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

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

// A theme bundled with the binary, so several themes work with no Omarchy install.
// Adding a theme later is one entry here; the rest of the app (picker, CSS, terminal) follows.
struct BuiltinTheme {
    name: &'static str,
    // background, foreground, accent, selection, muted, bright
    colors: [&'static str; 6],
    ansi: [&'static str; 16],
}

const BUILTIN_THEMES: &[BuiltinTheme] = &[
    BuiltinTheme {
        name: "tokyo-night",
        colors: ["#1a1b26", "#a9b1d6", "#7aa2f7", "#292e42", "#414868", "#c0caf5"],
        ansi: TOKYO_ANSI,
    },
    BuiltinTheme {
        name: "catppuccin-mocha",
        colors: ["#1e1e2e", "#cdd6f4", "#89b4fa", "#313244", "#45475a", "#b4befe"],
        ansi: [
            "#45475a", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#bac2de",
            "#585b70", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#a6adc8",
        ],
    },
    BuiltinTheme {
        name: "gruvbox-dark",
        colors: ["#282828", "#ebdbb2", "#83a598", "#3c3836", "#504945", "#fbf1c7"],
        ansi: [
            "#282828", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#a89984",
            "#928374", "#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b", "#8ec07c", "#ebdbb2",
        ],
    },
    BuiltinTheme {
        name: "nord",
        colors: ["#2e3440", "#d8dee9", "#88c0d0", "#3b4252", "#434c5e", "#eceff4"],
        ansi: [
            "#3b4252", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0", "#e5e9f0",
            "#4c566a", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#8fbcbb", "#eceff4",
        ],
    },
];

fn builtin_theme(name: &str) -> Option<&'static BuiltinTheme> {
    BUILTIN_THEMES.iter().find(|t| t.name == name)
}

fn palette_of(t: &BuiltinTheme) -> Palette {
    let [background, foreground, accent, selection, muted, bright] = t.colors;
    Palette {
        background: background.into(),
        foreground: foreground.into(),
        accent: accent.into(),
        selection: selection.into(),
        muted: muted.into(),
        bright: bright.into(),
    }
}

pub fn builtin() -> Palette {
    palette_of(&BUILTIN_THEMES[0])
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
        BUILTIN_THEMES[0].name.into()
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

// Theme names found on disk under the Omarchy theme dirs (each needs a colors.toml).
fn omarchy_theme_names() -> Vec<String> {
    let mut names = Vec::new();
    for dir in omarchy_dirs() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.join("colors.toml").is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    names.push(name.to_string());
                }
            }
        }
    }
    names
}

// Every theme this build can show: built in first, then anything found on disk, no duplicates.
pub fn available_themes() -> Vec<String> {
    let mut names: Vec<String> = BUILTIN_THEMES.iter().map(|t| t.name.to_string()).collect();
    let mut disk = omarchy_theme_names();
    disk.sort();
    disk.dedup();
    for name in disk {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

// A theme file on disk wins over the built-in one with the same name, so Omarchy's own
// tokyo-night or nord (and any edits to them) are what the app shows.
pub fn load_named(name: &str) -> Palette {
    read_theme(name)
        .and_then(|text| from_colors_toml(&text))
        .or_else(|| builtin_theme(name).map(palette_of))
        .unwrap_or_else(builtin)
}

pub struct TerminalTheme {
    pub background: String,
    pub foreground: String,
    pub cursor: String,
    pub selection: String,
    pub ansi: [String; 16],
}

// Tokyo Night ANSI. Used when the theme file has no color0–color15.
const TOKYO_ANSI: [&str; 16] = [
    "#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6",
    "#414868", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#c0caf5",
];

const ANSI_KEYS: [[&str; 2]; 16] = [
    ["color0", "black"],
    ["color1", "red"],
    ["color2", "green"],
    ["color3", "yellow"],
    ["color4", "blue"],
    ["color5", "magenta"],
    ["color6", "cyan"],
    ["color7", "white"],
    ["color8", "bright_black"],
    ["color9", "bright_red"],
    ["color10", "bright_green"],
    ["color11", "bright_yellow"],
    ["color12", "bright_blue"],
    ["color13", "bright_magenta"],
    ["color14", "bright_cyan"],
    ["color15", "bright_white"],
];

pub fn load_terminal(name: &str, bg_override: &str) -> TerminalTheme {
    let palette = load_named(name);
    // Same order as the palette: the file on disk, then the built-in theme, then Tokyo Night.
    let mut ansi = builtin_theme(name).map_or(TOKYO_ANSI, |t| t.ansi).map(String::from);
    let table = read_theme(name).and_then(|text| toml::from_str::<toml::Table>(&text).ok());
    if let Some(table) = &table {
        for (i, keys) in ANSI_KEYS.iter().enumerate() {
            if let Some(hex) = keys.iter().find_map(|key| color(table, key)) {
                ansi[i] = hex;
            }
        }
    }
    let background = hex_color(bg_override).unwrap_or(palette.background.as_str()).to_string();
    TerminalTheme {
        background,
        foreground: palette.foreground,
        cursor: palette.bright,
        selection: palette.selection,
        ansi,
    }
}

fn read_theme(name: &str) -> Option<String> {
    omarchy_dirs().into_iter().find_map(|dir| fs::read_to_string(dir.join(name).join("colors.toml")).ok())
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

// With a picture, 0 is the solid color and 100 is the picture alone.
// With no picture, the value is opacity: 100 is the color, lower shows the desktop through.
pub fn list_background(p: &Palette, color: &str, image: &str, strength: u8, stretch: bool) -> String {
    let chosen = hex_color(color);
    let strength = strength.min(100);
    let image = image.trim();
    let selected = selected_rows(p);
    if image.is_empty() {
        return plain_background(p, chosen, strength, &selected);
    }
    let show_image = strength > 0;
    if !show_image && chosen.is_none() {
        return String::new();
    }
    let fill = chosen.unwrap_or(p.background.as_str());
    if !show_image {
        return solid_lists(fill, &selected);
    }
    let url = css_file_url(image);
    let veil = f64::from(100 - strength) / 100.0;
    let fit = if stretch { "100% 100%" } else { "cover" };
    let (picture, size, at, repeat) = if veil <= 0.0 {
        (format!("url(\"{url}\")"), fit.to_string(), "center", "no-repeat")
    } else {
        (
            format!("linear-gradient(alpha({fill}, {veil:.2}), alpha({fill}, {veil:.2})), url(\"{url}\")"),
            format!("{fit}, {fit}"),
            "center, center",
            "no-repeat, no-repeat",
        )
    };
    format!(
        "paned.panes {{\n  background-color: {fill};\n  background-image: {picture};\n  background-size: {size};\n  background-position: {at};\n  background-repeat: {repeat};\n}}\npaned.panes > separator,\npaned.panes frame.pane,\npaned.panes frame.pane > border,\npaned.panes frame.pane box,\npaned.panes scrolledwindow,\npaned.panes columnview,\npaned.panes columnview > header > button,\npaned.panes columnview > listview,\npaned.panes columnview > listview > row,\npaned.panes columnview > listview > row > cell {{\n  background-color: transparent;\n  background-image: none;\n}}\n{selected}"
    )
}

fn selected_rows(p: &Palette) -> String {
    format!(
        "frame.pane-active columnview > listview > row:selected,\nframe.pane-active columnview > listview > row:selected > cell {{\n  background: {sel};\n  color: {bright};\n}}\n",
        sel = p.selection,
        bright = p.bright,
    )
}

// The filled bar belongs to the pane that has focus. The other pane keeps its place without that bar.
pub fn cursor_bar(fallback: &str, color: &str, alpha: u8, text: &str) -> String {
    let color = hex_color(color).unwrap_or(fallback);
    let opacity = f64::from(alpha.min(100)) / 100.0;
    format!(
        "frame.pane columnview > listview > row:selected,\nframe.pane columnview > listview > row:selected > cell {{\n  background-color: transparent;\n  background-image: none;\n  color: {text};\n}}\nframe.pane-active columnview > listview > row:selected,\nframe.pane-active columnview > listview > row:selected > cell {{\n  background-color: alpha({color}, {opacity:.2});\n  background-image: none;\n}}\n"
    )
}

fn solid_lists(fill: &str, selected: &str) -> String {
    format!(
        "columnview,\ncolumnview > listview,\ncolumnview > listview > row,\ncolumnview > listview > row > cell {{\n  background: {fill};\n}}\n{selected}"
    )
}

fn plain_background(p: &Palette, chosen: Option<&str>, strength: u8, selected: &str) -> String {
    if strength >= 100 {
        return match chosen {
            Some(fill) => solid_lists(fill, selected),
            None => String::new(),
        };
    }
    let fill = chosen.unwrap_or(p.background.as_str());
    let opacity = f64::from(strength) / 100.0;
    format!(
        "window.rust-commander {{\n  background-color: alpha({fill}, {opacity:.2});\n}}\nwindow.rust-commander headerbar,\nwindow.rust-commander .titlebar,\nwindow.rust-commander frame,\nwindow.rust-commander frame > border,\nwindow.rust-commander scrolledwindow,\nwindow.rust-commander columnview,\nwindow.rust-commander columnview > listview,\nwindow.rust-commander columnview > listview > row,\nwindow.rust-commander columnview > listview > row > cell,\nwindow.rust-commander textview,\nwindow.rust-commander textview text {{\n  background-color: transparent;\n  background-image: none;\n}}\n{selected}"
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
.pane-blink {{
  border-color: {bright};
  box-shadow: inset 0 0 0 1px {bright};
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
.cursor-mark {{
  color: #ffffff;
}}
.file-icon {{
  min-height: 0;
  min-width: 0;
  padding: 0;
  margin: 0 2px 0 -2px;
}}
.file-icon.dir {{
  color: {accent};
}}
label.icon-glyph {{
  /* PUA glyphs: Nerd Font if present, otherwise Font Awesome. */
  font-family: "JetBrainsMono Nerd Font", "Symbols Nerd Font Mono", FontAwesome, sans-serif;
  font-size: 14px;
  min-height: 0;
  min-width: 0;
  padding: 0;
  margin: 0 2px 0 0;
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
listview.search-results,
listview.search-results > row {{
  background: transparent;
  color: {fg};
}}
listview.search-results > row:hover {{
  background: alpha({sel}, 0.5);
}}
listview.search-results > row:selected,
listview.search-results > row:selected:hover {{
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
  color: {fg};
  padding: 2px 8px;
  border-bottom: 1px solid {muted};
  box-shadow: none;
  min-height: 0;
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
progressbar.job text {{
  font-size: 2.2em;
  font-weight: bold;
  color: {bright};
}}
button.drive {{
  background: transparent;
  border: 1px solid transparent;
  padding: 10px 4px;
  min-width: 0;
  min-height: 0;
}}
button.drive label {{
  font-size: 13px;
}}
button.drive label.icon-glyph {{
  font-size: 16px;
  margin: 0;
}}
button.drive label.drive-num {{
  color: {accent};
  font-family: monospace;
}}
button.drive:hover {{
  background: {sel};
}}
button.drive.current {{
  border-color: {accent};
}}
button.flash {{
  background: {accent};
  color: {bg};
}}
button.flash label {{
  color: {bg};
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
  font-family: "JetBrainsMono Nerd Font", "Noto Sans CJK SC", monospace;
  color: {accent};
}}
label.cmd-head {{
  font-size: 16px;
  font-weight: 700;
  color: {accent};
  padding-top: 18px;
  padding-bottom: 4px;
}}
button.command {{
  background: transparent;
  box-shadow: none;
  border: none;
  padding: 1px 4px;
  min-height: 0;
}}
button.command-on label {{
  font-size: 15px;
}}
button.command:hover {{
  background: {sel};
}}
button.command-idle label {{
  color: {fg};
}}
button.command-idle label.cmd-key {{
  color: {accent};
}}
button.command-idle:hover {{
  background: transparent;
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
button.hot:hover {{
  background: {sel};
}}
button.hot:focus,
button.hot:focus-visible {{
  background: {sel};
  border: 1px solid {accent};
  outline: none;
}}
window.hotlist {{
  outline: none;
  background: {bg};
}}
.hot-frame {{
  border: 1px solid {accent};
  background: {bg};
}}
button.hot-on {{
  background: {sel};
}}
button.flash:hover,
button.flash:focus,
button.flash:focus-visible,
button.flash:active,
button.fkey.flash,
button.hot.flash,
.settings-row.flash,
listview.search-results > row.flash,
listview.search-results > row.flash:selected,
box.flash,
label.flash,
colorbutton.flash > button {{
  background: {accent};
  color: {bg};
}}
.flash label {{
  color: {bg};
}}
@define-color accent_color {accent};
@define-color accent_bg_color {accent};
@define-color accent_fg_color {bg};
@define-color theme_selected_bg_color {accent};
@define-color theme_selected_fg_color {bg};
checkbutton check:checked,
checkbutton check:indeterminate,
checkbutton radio:checked,
check:checked,
radio:checked {{
  background: {accent};
  border-color: {accent};
  color: {bg};
}}
switch:checked {{
  background: {accent};
}}
*:focus-visible,
entry:focus-within {{
  outline-color: {accent};
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

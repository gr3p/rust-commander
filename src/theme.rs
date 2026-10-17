use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::RwLock;

// Theme name meaning "whatever Omarchy has applied right now".
pub const OMARCHY: &str = "omarchy";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    pub background: String,
    pub foreground: String,
    pub accent: String,
    pub selection: String,
    pub muted: String,
    pub bright: String,
    pub red: String,
    pub green: String,
    pub blue: String,
    pub dim: String,
    pub light: bool,
    // Selected and hovered rows; Omarchy's shell.toml can change them via `styled`.
    pub row_selected: String,
    pub row_hover: String,
    pub border: String,
    // The theme's own cursor bar and the text on it. Config overrides win over these.
    pub cursor: String,
    pub cursor_text: String,
    // Text color of files marked with Space or Insert.
    pub mark: String,
}

static CURRENT: RwLock<Option<Palette>> = RwLock::new(None);

// The palette last applied to the window, for colors set outside CSS (text tags, markup).
pub fn current() -> Palette {
    CURRENT.read().ok().and_then(|p| p.clone()).unwrap_or_else(builtin)
}

pub fn set_current(p: &Palette) {
    if let Ok(mut slot) = CURRENT.write() {
        *slot = Some(p.clone());
    }
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
        red: t.ansi[1].into(),
        green: t.ansi[2].into(),
        blue: t.ansi[4].into(),
        dim: muted.into(),
        light: false,
        row_selected: selection.into(),
        row_hover: selection.into(),
        border: accent.into(),
        cursor: accent.into(),
        cursor_text: bright.into(),
        mark: t.ansi[2].into(),
    }
}

pub fn builtin() -> Palette {
    palette_of(&BUILTIN_THEMES[0])
}

pub fn palette_from_optional(text: Option<&str>) -> Palette {
    text.and_then(from_colors_toml).unwrap_or_else(builtin)
}

pub fn from_colors_toml(text: &str) -> Option<Palette> {
    palette_of_colors(&resolve_colors(text)?)
}

fn palette_of_colors(c: &Colors) -> Option<Palette> {
    let background = col(c, "background")?;
    let foreground = col(c, "foreground")?;
    let accent = col(c, "accent").or_else(|| col(c, "blue"))?;
    let selection = col(c, "selection")?;
    let muted = col(c, "muted")?;
    let fallback = builtin();
    let bright = col(c, "bright_foreground").unwrap_or_else(|| foreground.clone());
    let green = col(c, "green").unwrap_or(fallback.green);
    Some(Palette {
        mark: green.clone(),
        cursor: accent.clone(),
        cursor_text: bright.clone(),
        bright,
        red: col(c, "red").unwrap_or(fallback.red),
        green,
        blue: col(c, "blue").unwrap_or_else(|| accent.clone()),
        dim: col(c, "dark_foreground").unwrap_or_else(|| muted.clone()),
        light: is_light(c, &background),
        row_selected: selection.clone(),
        row_hover: selection.clone(),
        border: accent.clone(),
        background,
        foreground,
        accent,
        selection,
        muted,
    })
}

type Colors = HashMap<String, String>;

impl Palette {
    // Applies Omarchy's shell.toml state tokens on top of the colors.
    pub fn styled(mut self, s: &ShellStyle) -> Self {
        let pick = |role: &str| match role {
            "accent" => self.accent.clone(),
            "background" => self.background.clone(),
            r => hex_color(r).map_or_else(|| self.foreground.clone(), str::to_string),
        };
        if let Some(alpha) = s.selected_alpha {
            self.row_selected = mix(&self.background, &pick(&s.selected_color), alpha);
        }
        if let Some(alpha) = s.hover_alpha {
            self.row_hover = mix(&self.background, &pick(&s.hover_color), alpha);
        }
        if let Some(border) = &s.active_border {
            self.border = border.clone();
        }
        self
    }
}

// Reads colors.toml the way `omarchy-theme-color` does: legacy names, ANSI names and derived shades.
fn resolve_colors(text: &str) -> Option<Colors> {
    let table: toml::Table = toml::from_str(text).ok()?;
    let mut c = Colors::new();
    for (key, value) in &table {
        let Some(raw) = value.as_str() else { continue };
        if key == "mode" || key == "theme_type" {
            c.insert(key.clone(), raw.trim().to_lowercase());
        } else if let Some(hex) = full_hex(raw) {
            c.insert(key.clone(), hex);
        }
    }
    let alias = |c: &mut Colors, key: &str, from: &[&str]| {
        if !c.contains_key(key) {
            if let Some(v) = from.iter().find_map(|f| c.get(*f).cloned()) {
                c.insert(key.to_string(), v);
            }
        }
    };
    for (key, old) in [
        ("background", "bg"),
        ("dark_background", "dark_bg"),
        ("darker_background", "darker_bg"),
        ("lighter_background", "lighter_bg"),
        ("foreground", "fg"),
        ("dark_foreground", "dark_fg"),
        ("light_foreground", "light_fg"),
        ("bright_foreground", "bright_fg"),
    ] {
        alias(&mut c, key, &[old]);
    }
    alias(&mut c, "background", &["color0"]);
    alias(&mut c, "foreground", &["color7"]);
    for (i, key) in ["red", "green", "yellow", "blue", "magenta", "cyan"].into_iter().enumerate() {
        alias(&mut c, key, &[&format!("color{}", i + 1)]);
        alias(&mut c, &format!("bright_{key}"), &[&format!("color{}", i + 9)]);
    }
    alias(&mut c, "magenta", &["purple"]);
    alias(&mut c, "bright_magenta", &["bright_purple"]);
    alias(&mut c, "dark_foreground", &["color8", "foreground"]);
    alias(&mut c, "muted", &["color8", "dark_foreground"]);
    alias(&mut c, "bright_foreground", &["color15", "foreground"]);
    alias(&mut c, "selection", &["selection_background", "color8", "color0", "background"]);
    for key in ["red", "green", "yellow", "blue", "magenta", "cyan"] {
        if let Some(base) = c.get(key).cloned() {
            c.entry(format!("bright_{key}")).or_insert_with(|| mix(&base, "#ffffff", 0.2));
        }
    }
    Some(c)
}

fn is_light(c: &Colors, background: &str) -> bool {
    match c.get("mode").or_else(|| c.get("theme_type")).map(String::as_str) {
        Some("light") => return true,
        Some("dark") => return false,
        _ => {}
    }
    rgb(background).is_some_and(|(r, g, b)| u32::from(r) + u32::from(g) + u32::from(b) > 382)
}

fn col(c: &Colors, key: &str) -> Option<String> {
    c.get(key).cloned()
}

// Accepts #rgb or #rrggbb and returns #rrggbb.
fn full_hex(raw: &str) -> Option<String> {
    let s = raw.trim();
    let digits = s.strip_prefix('#')?;
    if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match digits.len() {
        6 => Some(s.to_string()),
        3 => Some(format!("#{}", digits.chars().flat_map(|c| [c, c]).collect::<String>())),
        _ => None,
    }
}

fn rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let hex = full_hex(hex)?;
    let n = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some((n(1)?, n(3)?, n(5)?))
}

// `amount` of `to` over `from`, as #rrggbb.
fn mix(from: &str, to: &str, amount: f64) -> String {
    let (Some(a), Some(b)) = (rgb(from), rgb(to)) else { return from.to_string() };
    let amount = amount.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (f64::from(x) * (1.0 - amount) + f64::from(y) * amount).round() as u8;
    format!("#{:02x}{:02x}{:02x}", m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

pub fn active_name(config_theme: &str, active_file: &Path) -> String {
    if let Ok(text) = fs::read_to_string(active_file) {
        let name = text.trim();
        if !name.is_empty() {
            return name.to_string();
        }
    }
    if config_theme.is_empty() {
        OMARCHY.into()
    } else {
        config_theme.to_string()
    }
}

// Omarchy's applied theme: colors.toml, shell.toml and icons.theme, plus theme.name beside it.
pub fn omarchy_current() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".local/state/omarchy/current"))
}

fn omarchy_current_file(name: &str) -> Option<PathBuf> {
    Some(omarchy_current()?.join("theme").join(name))
}

fn omarchy_present() -> bool {
    omarchy_current_file("colors.toml").is_some_and(|p| p.is_file())
}

// Slug of the theme Omarchy has applied, e.g. "nord".
pub fn omarchy_theme_name() -> Option<String> {
    let text = fs::read_to_string(omarchy_current()?.join("theme.name")).ok()?;
    Some(text.trim().to_string()).filter(|s| !s.is_empty())
}

// The icon theme Omarchy's theme asks for, e.g. "Yaru-magenta".
pub fn omarchy_icon_theme() -> Option<String> {
    let text = fs::read_to_string(omarchy_current_file("icons.theme")?).ok()?;
    Some(text.trim().to_string()).filter(|s| !s.is_empty())
}

pub fn label(name: &str) -> String {
    if name != OMARCHY {
        return name.to_string();
    }
    match omarchy_theme_name() {
        Some(applied) => format!("omarchy ({applied})"),
        None => "omarchy (follow system)".into(),
    }
}

// The user's own themes come first: Omarchy lets a same-named user theme replace the stock one.
pub fn omarchy_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".config/omarchy/themes"));
    }
    dirs.push(PathBuf::from("/usr/share/omarchy/themes"));
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

// Every theme this build can show: the system's own first, then built in, then anything on disk.
pub fn available_themes() -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    if omarchy_present() {
        names.push(OMARCHY.to_string());
    }
    names.extend(BUILTIN_THEMES.iter().map(|t| t.name.to_string()));
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
    let palette = read_theme(name)
        .and_then(|text| from_colors_toml(&text))
        .or_else(|| builtin_theme(name).map(palette_of))
        .unwrap_or_else(builtin);
    if name == OMARCHY { palette.styled(&ShellStyle::load()) } else { palette }
}

pub struct TerminalTheme {
    pub background: String,
    pub foreground: String,
    pub cursor: String,
    pub selection: String,
    pub ansi: [String; 16],
}

// Tokyo Night ANSI. Used when the theme file has no matching color.
const TOKYO_ANSI: [&str; 16] = [
    "#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6",
    "#414868", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#c0caf5",
];

// Same mapping as Omarchy's alacritty.toml.tpl: black is the background, white the foreground.
const ANSI_KEYS: [&str; 16] = [
    "background",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "foreground",
    "muted",
    "bright_red",
    "bright_green",
    "bright_yellow",
    "bright_blue",
    "bright_magenta",
    "bright_cyan",
    "bright_foreground",
];

pub fn load_terminal(name: &str, bg_override: &str) -> TerminalTheme {
    let palette = load_named(name);
    // Same order as the palette: the file on disk, then the built-in theme, then Tokyo Night.
    let mut ansi = builtin_theme(name).map_or(TOKYO_ANSI, |t| t.ansi).map(String::from);
    if let Some(colors) = read_theme(name).and_then(|text| resolve_colors(&text)) {
        for (slot, key) in ansi.iter_mut().zip(ANSI_KEYS) {
            if let Some(hex) = col(&colors, key) {
                *slot = hex;
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
    if name == OMARCHY {
        return fs::read_to_string(omarchy_current_file("colors.toml")?).ok();
    }
    omarchy_dirs().into_iter().find_map(|dir| fs::read_to_string(dir.join(name).join("colors.toml")).ok())
}

// Sizes and state tokens from the applied theme's shell.toml, plus Hyprland's corner rounding.
#[derive(Clone, Debug, PartialEq)]
pub struct ShellStyle {
    pub selected_alpha: Option<f64>,
    pub selected_color: String,
    pub hover_alpha: Option<f64>,
    pub hover_color: String,
    pub active_border: Option<String>,
    pub font_px: u32,
    pub space_scale: f64,
    pub radius: u32,
}

impl Default for ShellStyle {
    fn default() -> Self {
        Self {
            selected_alpha: None,
            selected_color: "foreground".into(),
            hover_alpha: None,
            hover_color: "foreground".into(),
            active_border: None,
            font_px: 13,
            space_scale: 1.0,
            radius: 0,
        }
    }
}

impl ShellStyle {
    pub fn load() -> Self {
        let text = omarchy_current_file("shell.toml").and_then(|p| fs::read_to_string(p).ok());
        Self::from_text(text.as_deref().unwrap_or(""), rounding())
    }

    pub fn from_text(text: &str, radius: u32) -> Self {
        let v = shell_values(text);
        let num = |key: &str| v.get(key).and_then(|s| s.parse::<f64>().ok());
        let base = num("font.base-size").filter(|n| *n >= 1.0).unwrap_or(12.0);
        let with_font = v.get("spacing.scale-with-font").is_none_or(|s| s != "false");
        let scale = num("spacing.scale").filter(|n| *n > 0.0).unwrap_or(1.0);
        let role = |key: &str| v.get(key).cloned().unwrap_or_else(|| "foreground".into());
        Self {
            selected_alpha: num("controls.selected-fill-alpha").map(|a| a.clamp(0.0, 1.0)),
            selected_color: role("controls.selected-color"),
            hover_alpha: num("controls.hover-cursor-fill-alpha").map(|a| a.clamp(0.0, 1.0)),
            hover_color: role("controls.hover-cursor-color"),
            active_border: v.get("hyprland.active-border").and_then(|b| first_color(b)),
            // 13px is the body size at Omarchy's default 12px base.
            font_px: (base * 13.0 / 12.0).round() as u32,
            space_scale: scale * if with_font { base / 12.0 } else { 1.0 },
            radius,
        }
    }

    // A padding or margin in px, scaled like Omarchy's `Style.space`.
    pub fn space(&self, px: u32) -> u32 {
        ((f64::from(px) * self.space_scale).round() as u32).max(1)
    }
}

// Flat "section.key" -> value; tolerates the bare words and lists Omarchy's own parser accepts.
fn shell_values(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut section = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.split(']').next()) {
            section = name.trim().to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim();
        let value = match value.chars().next() {
            Some(q @ ('"' | '\'')) => value[1..].split(q).next().unwrap_or(""),
            _ => value.split('#').next().unwrap_or("").trim(),
        };
        if !section.is_empty() {
            out.insert(format!("{section}.{}", key.trim()), value.to_string());
        }
    }
    out
}

// First solid color of a value that may be a Hyprland gradient like "rgba(33ccffee) rgba(00ff99ee) 45deg".
fn first_color(value: &str) -> Option<String> {
    value.split_whitespace().find_map(|token| {
        if let Some(hex) = full_hex(token) {
            return Some(hex);
        }
        let inner = token.strip_prefix("rgba(").or_else(|| token.strip_prefix("rgb("))?.strip_suffix(')')?;
        let digits = inner.get(..6).filter(|d| d.bytes().all(|b| b.is_ascii_hexdigit()))?;
        Some(format!("#{digits}"))
    })
}

static ROUNDING: AtomicI32 = AtomicI32::new(-1);

// Hyprland's window rounding is remembered until `refresh_system_style`, so restyling stays cheap.
fn rounding() -> u32 {
    let cached = ROUNDING.load(Ordering::Relaxed);
    if cached >= 0 {
        return cached as u32;
    }
    let value = query_rounding().unwrap_or(0);
    ROUNDING.store(value as i32, Ordering::Relaxed);
    value
}

pub fn refresh_system_style() {
    ROUNDING.store(-1, Ordering::Relaxed);
}

fn query_rounding() -> Option<u32> {
    let out = Command::new("hyprctl").args(["getoption", "decoration:rounding", "-j"]).output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let rest = text.split("\"int\":").nth(1)?.trim_start();
    rest.chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()
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
        "paned.panes {{\n  background-color: {fill};\n  background-image: {picture};\n  background-size: {size};\n  background-position: {at};\n  background-repeat: {repeat};\n}}\npaned.panes > separator,\npaned.panes frame.pane,\npaned.panes frame.pane > border,\npaned.panes frame.pane box,\npaned.panes scrolledwindow,\npaned.panes columnview,\npaned.panes columnview > header > button,\npaned.panes columnview > listview,\npaned.panes columnview > listview > row,\npaned.panes columnview > listview > row > cell,\npaned.panes gridview,\npaned.panes gridview > child {{\n  background-color: transparent;\n  background-image: none;\n}}\n{selected}"
    )
}

fn selected_rows(p: &Palette) -> String {
    format!(
        "frame.pane-active columnview > listview > row:selected,\nframe.pane-active columnview > listview > row:selected > cell,\nframe.pane-active gridview > child:selected {{\n  background: {sel};\n  color: {bright};\n}}\n",
        sel = p.row_selected,
        bright = p.bright,
    )
}

// The filled bar belongs to the pane that has focus. The other pane keeps its place without that bar.
// `color` and `text` are the user's overrides; empty means the theme's own. Text only changes on the bar.
pub fn cursor_bar(p: &Palette, color: &str, alpha: u8, text: &str) -> String {
    let color = hex_color(color).unwrap_or(&p.cursor);
    let text = hex_color(text).unwrap_or(&p.cursor_text);
    let opacity = f64::from(alpha.min(100)) / 100.0;
    let idle = &p.foreground;
    format!(
        "frame.pane columnview > listview > row:selected,\nframe.pane columnview > listview > row:selected > cell,\nframe.pane gridview > child:selected {{\n  background-color: transparent;\n  background-image: none;\n  color: {idle};\n}}\nframe.pane-active columnview > listview > row:selected,\nframe.pane-active columnview > listview > row:selected > cell,\nframe.pane-active gridview > child:selected {{\n  background-color: alpha({color}, {opacity:.2});\n  background-image: none;\n  color: {text};\n}}\n"
    )
}

// Text of marked files. `color` is the user's override; empty means the theme's own.
pub fn marked_text(p: &Palette, color: &str) -> String {
    let color = hex_color(color).unwrap_or(&p.mark);
    format!(".marked {{\n  color: {color};\n}}\n")
}

fn solid_lists(fill: &str, selected: &str) -> String {
    format!(
        "columnview,\ncolumnview > listview,\ncolumnview > listview > row,\ncolumnview > listview > row > cell,\ngridview,\ngridview > child {{\n  background: {fill};\n}}\n{selected}"
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
        "window.rust-commander {{\n  background-color: alpha({fill}, {opacity:.2});\n}}\nwindow.rust-commander headerbar,\nwindow.rust-commander .titlebar,\nwindow.rust-commander frame,\nwindow.rust-commander frame > border,\nwindow.rust-commander scrolledwindow,\nwindow.rust-commander columnview,\nwindow.rust-commander columnview > listview,\nwindow.rust-commander columnview > listview > row,\nwindow.rust-commander columnview > listview > row > cell,\nwindow.rust-commander gridview,\nwindow.rust-commander gridview > child,\nwindow.rust-commander textview,\nwindow.rust-commander textview text {{\n  background-color: transparent;\n  background-image: none;\n}}\n{selected}"
    )
}

pub fn stylesheet(p: &Palette, s: &ShellStyle) -> String {
    // The accent is the light theme blue. A flash of that reads as a pale wash, so the blink is the same hue, pulled toward black.
    let flash = mix(&p.accent, "#000000", 0.42);
    let flash_ink = if rgb(&flash).is_some_and(|(r, g, b)| u32::from(r) + u32::from(g) + u32::from(b) > 382) {
        &p.background
    } else {
        &p.foreground
    };
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
button {{
  border-radius: {radius}px;
}}
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
  border: 1px solid {border};
}}
.pane-blink {{
  border-color: {bright};
  box-shadow: inset 0 0 0 1px {bright};
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
.cursor-mark, .thumb-spin {{
  color: {bright};
}}
.file-icon {{
  min-height: 0;
  min-width: 0;
  padding: 0;
  margin: 0 {icon_gap}px 0 -2px;
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
  margin: 0 {icon_gap}px 0 0;
}}
.thumb-cell .file-icon,
.thumb-cell label.icon-glyph {{
  margin: 0;
}}
.thumb-cell label.icon-glyph {{
  font-size: 40px;
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
  background: {rsel};
  color: {bright};
}}
listview.search-results,
listview.search-results > row {{
  background: transparent;
  color: {fg};
}}
listview.search-results > row:hover {{
  background: alpha({rhov}, 0.5);
}}
listview.search-results > row:selected,
listview.search-results > row:selected:hover {{
  background: {rsel};
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
entry.filter {{
  padding: 2px 8px;
  border-bottom: 1px solid {muted};
  box-shadow: none;
  min-height: 0;
}}
columnview > listview > row > cell {{
  padding: {cell_pad};
  min-height: 0;
}}
columnview > header > button {{
  background: transparent;
  color: {fg};
  border-radius: 0;
  box-shadow: none;
  padding: {head_pad};
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
  padding: 3px 4px;
  min-width: 0;
  min-height: 0;
}}
button.drive label {{
  font-size: 13px;
}}
button.drive label.icon-glyph {{
  font-size: 16px;
  margin: 0 {icon_gap}px 0 0;
}}
button.drive label.drive-num {{
  color: {accent};
  font-family: monospace;
}}
button.drive:hover {{
  background: {rhov};
}}
button.drive.current {{
  border-color: {accent};
}}
button.drive.idle {{
  opacity: 0.6;
}}
button.flash {{
  background: {flash};
  color: {flash_ink};
}}
button.flash label {{
  color: {flash_ink};
}}
button.fkey {{
  background: {sel};
  color: {fg};
  min-width: 0;
  padding: {fkey_pad};
}}
button.fkey label {{
  min-width: 0;
}}
.settings-row {{
  padding: {row_pad};
}}
.settings-row:hover {{
  background: {rhov};
}}
label.settings-on {{
  color: {accent};
}}
label.dialog-title {{
  font-weight: 700;
  color: {accent};
}}
label.about-title {{
  font-family: "Noto Sans", Cantarell, sans-serif;
  font-size: 30px;
  font-weight: 800;
  letter-spacing: 0.4px;
  color: {fg};
}}
label.about-version {{
  color: {muted};
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
  background: {rhov};
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
  background: {rsel};
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
  background: {rhov};
}}
button.hot:focus,
button.hot:focus-visible {{
  background: {rhov};
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
  background: {rsel};
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
tab.flash,
tab.flash:hover,
.flash,
colorbutton.flash > button {{
  background: {flash};
  color: {flash_ink};
}}
.flash label {{
  color: {flash_ink};
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
        border = p.border,
        rsel = p.row_selected,
        rhov = p.row_hover,
        radius = s.radius,
        icon_gap = s.space(6),
        cell_pad = format!("0 {}px", s.space(6)),
        head_pad = format!("{}px {}px", s.space(2), s.space(8)),
        fkey_pad = format!("{}px {}px", s.space(4), s.space(2)),
        row_pad = format!("{}px {}px", s.space(10), s.space(12)),
        flash = flash,
        flash_ink = flash_ink,
    )
}

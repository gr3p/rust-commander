use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

use relm4::gtk;
use relm4::gtk::prelude::*;

use relm4::gtk::pango::prelude::FontExt;
use rust_commander::icons::{AWESOME, SYSTEM, THEME_REQUIRED};

pub fn active<'a>(pack: &'a str) -> &'a str {
    if pack == AWESOME && !glyphs_available() { SYSTEM } else { pack }
}

pub fn choices() -> Vec<(String, String)> {
    let mut out = Vec::new();
    if glyphs_available() {
        out.push((AWESOME.to_string(), "Font Awesome".to_string()));
    }
    out.push((SYSTEM.to_string(), "System".to_string()));
    for name in gtk_themes() {
        out.push((name.clone(), name.clone()));
    }
    out
}

pub fn is_glyph(token: &str) -> bool {
    let mut chars = token.chars();
    matches!(chars.next(), Some(c) if !c.is_ascii() && chars.next().is_none())
}

pub fn icon_widget(token: &str, px: i32) -> gtk::Widget {
    if is_glyph(token) {
        let label = gtk::Label::new(Some(token));
        label.add_css_class("icon-glyph");
        label.set_valign(gtk::Align::Center);
        return label.upcast();
    }
    let name = icon_name(token);
    let themed = token.starts_with('@');
    let known = themed || gtk::gdk::Display::default()
        .map(|display| gtk::IconTheme::for_display(&display))
        .is_some_and(|theme| theme.has_icon(name));
    if !known {
        return gtk::Label::new(Some(fallback(name))).upcast();
    }
    let image = gtk::Image::new();
    image.set_pixel_size(px);
    apply_image(&image, token);
    image.upcast()
}

pub fn apply_image(image: &gtk::Image, token: &str) {
    if let Some((theme, name)) = themed(token) {
        let px = image.pixel_size().max(16);
        image.set_paintable(Some(&lookup(theme, name, px)));
        return;
    }
    image.set_icon_name(Some(token));
}

fn themed(token: &str) -> Option<(&str, &str)> {
    token.strip_prefix('@')?.split_once('/')
}

fn icon_name(token: &str) -> &str {
    themed(token).map(|(_, name)| name).unwrap_or(token)
}

fn fallback(name: &str) -> &'static str {
    match name {
        "network-server-symbolic" => "⇄",
        "drive-removable-media-symbolic" => "⏏",
        _ => "▮",
    }
}

fn glyphs_available() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| {
        let probe = gtk::Label::new(None);
        let ctx = probe.pango_context();
        for family in ["JetBrainsMono Nerd Font", "Symbols Nerd Font Mono", "FontAwesome"] {
            let desc = gtk::pango::FontDescription::from_string(&format!("{family} 16"));
            if ctx.load_font(&desc).is_some_and(|font| font.has_char('\u{f0a0}') && font.has_char('\u{f07b}')) {
                return true;
            }
        }
        false
    })
}

fn gtk_themes() -> &'static [String] {
    static CACHE: OnceLock<Vec<String>> = OnceLock::new();
    CACHE.get_or_init(scan_themes)
}

fn scan_themes() -> Vec<String> {
    let Some(display) = gtk::gdk::Display::default() else { return Vec::new() };
    let sys = gtk::IconTheme::for_display(&display);
    let mut names = Vec::new();
    for dir in sys.search_path() {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            if entry.path().join("index.theme").is_file() {
                if let Ok(name) = entry.file_name().into_string() {
                    names.push(name);
                }
            }
        }
    }
    names.sort();
    names.dedup();
    let mut out = Vec::new();
    let mut seen: Vec<Vec<String>> = Vec::new();
    for name in names {
        if name == SYSTEM || name == AWESOME {
            continue;
        }
        let theme = theme_for(&name);
        if !THEME_REQUIRED.iter().all(|icon| theme.has_icon(icon)) {
            continue;
        }
        let files: Vec<String> = THEME_REQUIRED
            .iter()
            .map(|icon| icon_file(&theme, icon))
            .collect();
        if seen.iter().any(|prev| prev == &files) {
            continue;
        }
        seen.push(files);
        out.push(name);
    }
    out
}

fn icon_file(theme: &gtk::IconTheme, name: &str) -> String {
    theme
        .lookup_icon(name, &[], 16, 1, gtk::TextDirection::None, gtk::IconLookupFlags::empty())
        .file()
        .and_then(|file| file.path())
        .map(|path| path.display().to_string())
        .unwrap_or_default()
}

fn lookup(theme_name: &str, icon: &str, px: i32) -> gtk::IconPaintable {
    thread_local! {
        static CACHE: RefCell<HashMap<String, gtk::IconTheme>> = RefCell::new(HashMap::new());
    }
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let theme = cache.entry(theme_name.to_string()).or_insert_with(|| theme_for(theme_name));
        theme.lookup_icon(icon, &[], px, 1, gtk::TextDirection::None, gtk::IconLookupFlags::empty())
    })
}

fn theme_for(name: &str) -> gtk::IconTheme {
    let theme = gtk::IconTheme::new();
    if let Some(display) = gtk::gdk::Display::default() {
        let sys = gtk::IconTheme::for_display(&display);
        let paths = sys.search_path();
        let refs: Vec<&std::path::Path> = paths.iter().map(|path| path.as_path()).collect();
        theme.set_search_path(&refs);
    }
    theme.set_theme_name(Some(name));
    theme
}

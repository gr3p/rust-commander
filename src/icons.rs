use crate::listing::{self, extension};

pub const SYSTEM: &str = "system";
pub const AWESOME: &str = "awesome";

// Font Awesome 4 private-use codepoints. Same slots exist in a Nerd Font.
const GLYPH_FOLDER: &str = "\u{f07b}";
const GLYPH_LINK: &str = "\u{f0c1}";
const GLYPH_TEXT: &str = "\u{f0f6}";
const GLYPH_AUDIO: &str = "\u{f1c7}";
const GLYPH_IMAGE: &str = "\u{f1c5}";
const GLYPH_VIDEO: &str = "\u{f1c8}";
const GLYPH_ARCHIVE: &str = "\u{f1c6}";
const GLYPH_PDF: &str = "\u{f1c1}";
const GLYPH_CODE: &str = "\u{f1c9}";
const GLYPH_FILE: &str = "\u{f016}";
const GLYPH_DRIVE: &str = "\u{f0a0}";
const GLYPH_REMOVABLE: &str = "\u{f287}";
const GLYPH_NETWORK: &str = "\u{f233}";

enum Kind {
    Folder,
    Link,
    Text,
    Audio,
    Image,
    Video,
    Archive,
    Pdf,
    Code,
    File,
    Drive,
    Removable,
    Network,
}

fn kind(name: &str, is_dir: bool, is_link: bool) -> Kind {
    if is_link {
        return Kind::Link;
    }
    if is_dir {
        return Kind::Folder;
    }
    if listing::is_audio(name) {
        return Kind::Audio;
    }
    if listing::is_video(name) {
        return Kind::Video;
    }
    match extension(name).to_ascii_lowercase().as_str() {
        "txt" | "md" | "log" => Kind::Text,
        "jpg" | "jpeg" | "jfif" | "png" | "gif" | "webp" => Kind::Image,
        "zip" | "7z" | "rar" | "tar" | "gz" | "tgz" | "bz2" | "xz" => Kind::Archive,
        "pdf" => Kind::Pdf,
        "rs" | "py" | "js" | "ts" | "go" | "c" | "h" | "cpp" | "java" => Kind::Code,
        _ => Kind::File,
    }
}

fn glyph(kind: Kind) -> &'static str {
    match kind {
        Kind::Folder => GLYPH_FOLDER,
        Kind::Link => GLYPH_LINK,
        Kind::Text => GLYPH_TEXT,
        Kind::Audio => GLYPH_AUDIO,
        Kind::Image => GLYPH_IMAGE,
        Kind::Video => GLYPH_VIDEO,
        Kind::Archive => GLYPH_ARCHIVE,
        Kind::Pdf => GLYPH_PDF,
        Kind::Code => GLYPH_CODE,
        Kind::File => GLYPH_FILE,
        Kind::Drive => GLYPH_DRIVE,
        Kind::Removable => GLYPH_REMOVABLE,
        Kind::Network => GLYPH_NETWORK,
    }
}

// Full-color names. A GTK theme pack is usable when it has this set.
fn theme_icon(kind_name: &str, is_dir: bool, is_link: bool) -> &'static str {
    match kind(kind_name, is_dir, is_link) {
        Kind::Folder => "folder",
        Kind::Link => "emblem-symbolic-link",
        Kind::Text | Kind::File => "text-x-generic",
        Kind::Audio => "audio-x-generic",
        Kind::Image => "image-x-generic",
        Kind::Video => "video-x-generic",
        Kind::Archive => "package-x-generic",
        Kind::Pdf => "application-pdf",
        Kind::Code => "text-x-script",
        Kind::Drive => "drive-harddisk",
        Kind::Removable => "drive-removable-media",
        Kind::Network => "network-server",
    }
}

pub const THEME_REQUIRED: &[&str] = &[
    "folder",
    "text-x-generic",
    "image-x-generic",
    "audio-x-generic",
    "video-x-generic",
    "package-x-generic",
    "drive-harddisk",
    "drive-removable-media",
    "network-server",
];

fn drive_kind(removable: bool, network: bool) -> Kind {
    if network {
        Kind::Network
    } else if removable {
        Kind::Removable
    } else {
        Kind::Drive
    }
}

fn system_drive(removable: bool, network: bool) -> &'static str {
    match drive_kind(removable, network) {
        Kind::Network => "network-server-symbolic",
        Kind::Removable => "drive-removable-media-symbolic",
        _ => "drive-harddisk-symbolic",
    }
}

// Empty, one Font Awesome character, a GTK name, or `@Theme/icon-name`.
pub fn file_token(pack: &str, name: &str, is_dir: bool, is_link: bool) -> String {
    if name == ".." {
        return String::new();
    }
    if pack == AWESOME {
        return glyph(kind(name, is_dir, is_link)).to_string();
    }
    if pack.is_empty() || pack == SYSTEM {
        return listing::file_icon(name, is_dir, is_link).to_string();
    }
    format!("@{pack}/{}", theme_icon(name, is_dir, is_link))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Art {
    Audio,
    Video,
    Pdf,
}

pub fn art_for(name: &str) -> Option<Art> {
    if listing::is_audio(name) {
        Some(Art::Audio)
    } else if listing::is_video(name) {
        Some(Art::Video)
    } else if listing::is_pdf(name) {
        Some(Art::Pdf)
    } else {
        None
    }
}

// Quick view art. Outline in the theme text color, mark in the accent.
pub fn art_svg(art: Art, palette: &crate::theme::Palette) -> String {
    let (fg, accent) = (&palette.foreground, &palette.accent);
    let body = match art {
        Art::Audio => format!(
            r#"<path d="M26 46V14l24-5v31" fill="none" stroke="{fg}" stroke-width="4" stroke-linejoin="round"/><ellipse cx="20" cy="46" rx="7" ry="5.5" fill="{accent}"/><ellipse cx="44" cy="40" rx="7" ry="5.5" fill="{accent}"/>"#
        ),
        Art::Video => format!(
            r#"<rect x="6" y="10" width="52" height="44" rx="5" fill="none" stroke="{fg}" stroke-width="3"/><path d="M14 10v44M50 10v44M6 19h8M6 28h8M6 36h8M6 45h8M50 19h8M50 28h8M50 36h8M50 45h8" stroke="{fg}" stroke-width="2"/><path d="M27 23l13 9-13 9z" fill="{accent}"/>"#
        ),
        // No text feature in resvg: page with fold and an accent seal.
        Art::Pdf => format!(
            r#"<path d="M18 6h20l12 12v40H18z" fill="none" stroke="{fg}" stroke-width="3" stroke-linejoin="round"/><path d="M38 6v12h12" fill="none" stroke="{fg}" stroke-width="3" stroke-linejoin="round"/><path d="M26 28h16M26 35h16M26 42h10" stroke="{fg}" stroke-width="2" stroke-linecap="round"/><rect x="22" y="48" width="20" height="8" rx="2" fill="{accent}"/>"#
        ),
    };
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">{body}</svg>"#)
}

pub fn drive_token(pack: &str, removable: bool, network: bool) -> String {
    if pack == AWESOME {
        return glyph(drive_kind(removable, network)).to_string();
    }
    if pack.is_empty() || pack == SYSTEM {
        return system_drive(removable, network).to_string();
    }
    let icon = match drive_kind(removable, network) {
        Kind::Network => "network-server",
        Kind::Removable => "drive-removable-media",
        _ => "drive-harddisk",
    };
    format!("@{pack}/{icon}")
}

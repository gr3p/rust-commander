use std::io::BufReader;
use std::path::{Path, PathBuf};

use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat, ImageReader, Limits};

use crate::entry::Entry;
use crate::listing;

pub const MAX_EDGE: u32 = 2048;
const MAX_BYTES: u64 = 48 * 1024 * 1024;

pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

pub fn is_image(name: &str) -> bool {
    matches!(
        listing::extension(name).to_ascii_lowercase().as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "ico"
    )
}

pub fn image_paths(dir: &Path, entries: &[Entry], order: &[usize]) -> Vec<PathBuf> {
    order
        .iter()
        .filter_map(|&i| {
            let entry = entries.get(i)?;
            if entry.is_dir() || !is_image(&entry.name) {
                return None;
            }
            Some(listing::entry_path(dir, entry))
        })
        .collect()
}

pub fn step_index(len: usize, index: usize, delta: isize) -> Option<usize> {
    let next = index as isize + delta;
    if len == 0 || next < 0 || next >= len as isize {
        None
    } else {
        Some(next as usize)
    }
}

// A FIFO named *.png would block its decode thread for good, so only a regular file is opened.
pub fn decode(path: &Path, max_edge: u32) -> Result<Rgba, String> {
    let file = crate::lister::open_regular(path).map_err(|err| err.to_string())?;
    let len = file.metadata().map_err(|err| err.to_string())?.len();
    if len > MAX_BYTES {
        return Err("Image too large".into());
    }
    let mut reader = ImageReader::new(BufReader::new(file));
    // As ImageReader::open: the extension first, the content wins when it is recognized.
    if let Ok(format) = ImageFormat::from_path(path) {
        reader.set_format(format);
    }
    let mut reader = reader.with_guessed_format().map_err(|err| err.to_string())?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let img = reader.decode().map_err(|err| err.to_string())?;
    let img = downscale(img, max_edge);
    let rgba = img.to_rgba8();
    Ok(Rgba {
        width: rgba.width(),
        height: rgba.height(),
        pixels: rgba.into_raw(),
    })
}

fn downscale(img: DynamicImage, max_edge: u32) -> DynamicImage {
    let (w, h) = fit_size(img.width(), img.height(), max_edge);
    if w == img.width() && h == img.height() {
        img
    } else {
        img.resize_exact(w, h, FilterType::Triangle)
    }
}

fn fit_size(w: u32, h: u32, max_edge: u32) -> (u32, u32) {
    let long = w.max(h);
    if long <= max_edge || w == 0 || h == 0 {
        return (w, h);
    }
    let nw = (w as u64 * max_edge as u64 / long as u64).max(1) as u32;
    let nh = (h as u64 * max_edge as u64 / long as u64).max(1) as u32;
    (nw, nh)
}

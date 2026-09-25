use std::path::Path;

use crate::config::Bookmark;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Dir,
    Sep,
    Menu,
    End,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Dir { index: usize, accel: Option<char> },
    Sep,
    Menu { index: usize, accel: Option<char> },
}

fn kind_name(mark: &Bookmark) -> Kind {
    if mark.kind == "end" || mark.name == "--" {
        Kind::End
    } else if mark.kind == "menu" {
        Kind::Menu
    } else if mark.kind == "sep" || mark.name == "-" {
        Kind::Sep
    } else {
        Kind::Dir
    }
}

pub fn is_dir(mark: &Bookmark) -> bool {
    kind_name(mark) == Kind::Dir
}

// '&' marks the accelerator letter. '&&' is a literal ampersand.
pub fn split_accel(name: &str) -> (String, Option<char>) {
    let mut label = String::new();
    let mut accel = None;
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&' {
            if chars.peek() == Some(&'&') {
                chars.next();
                label.push('&');
            } else if let Some(next) = chars.next() {
                if accel.is_none() && !next.is_whitespace() {
                    accel = Some(next.to_ascii_lowercase());
                }
                label.push(next);
            }
        } else {
            label.push(c);
        }
    }
    (label, accel)
}

pub fn accel_markup(name: &str) -> String {
    let mut out = String::new();
    let mut used = false;
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&' {
            if chars.peek() == Some(&'&') {
                chars.next();
                out.push_str("&amp;");
            } else if let Some(next) = chars.next() {
                let text = escape(next);
                if !used && !next.is_whitespace() {
                    used = true;
                    out.push_str(&format!("<u>{text}</u>"));
                } else {
                    out.push_str(&text);
                }
            }
        } else {
            out.push_str(&escape(c));
        }
    }
    out
}

fn escape(c: char) -> String {
    match c {
        '&' => "&amp;".into(),
        '<' => "&lt;".into(),
        '>' => "&gt;".into(),
        c => c.to_string(),
    }
}

pub fn rows(marks: &[Bookmark], menu: Option<usize>) -> Vec<Row> {
    let start = menu.map(|i| i + 1).unwrap_or(0);
    let mut depth = 0;
    let mut out = Vec::new();
    for (index, mark) in marks.iter().enumerate().skip(start) {
        match kind_name(mark) {
            Kind::End => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            Kind::Menu => {
                if depth == 0 {
                    let (_, accel) = split_accel(&mark.name);
                    out.push(Row::Menu { index, accel });
                }
                depth += 1;
            }
            Kind::Sep if depth == 0 => out.push(Row::Sep),
            Kind::Dir if depth == 0 => {
                let (_, accel) = split_accel(&mark.name);
                out.push(Row::Dir { index, accel });
            }
            _ => {}
        }
    }
    out
}

pub fn dir_index(marks: &[Bookmark], path: &Path) -> Option<usize> {
    marks.iter().position(|mark| is_dir(mark) && mark.path == path)
}

pub fn edit_label(mark: &Bookmark) -> String {
    match kind_name(mark) {
        Kind::Menu => format!("▸ {}", mark.name),
        Kind::End => "--".into(),
        Kind::Sep => "—".into(),
        Kind::Dir => format!("{}   {}", mark.name, mark.path.display()),
    }
}

pub fn delete_at(marks: &mut Vec<Bookmark>, index: usize) {
    if index >= marks.len() {
        return;
    }
    if kind_name(&marks[index]) != Kind::Menu {
        marks.remove(index);
        return;
    }
    let mut depth = 0;
    let mut end = marks.len() - 1;
    for (i, mark) in marks.iter().enumerate().skip(index) {
        match kind_name(mark) {
            Kind::Menu => depth += 1,
            Kind::End => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    marks.drain(index..=end);
}

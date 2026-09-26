use std::fs;
use std::path::PathBuf;

#[derive(Debug)]
pub struct Options {
    pub root: PathBuf,
    pub query: String,
    pub recursive: bool,
    pub hidden: bool,
    pub files_only: bool,
    pub case_sensitive: bool,
}

#[derive(Debug)]
pub enum Event {
    Matches(Vec<PathBuf>),
    Finished { errors: usize, cancelled: bool },
}

pub fn run(options: Options, cancelled: impl Fn() -> bool, mut emit: impl FnMut(Event)) {
    let needle = if options.case_sensitive { options.query.clone() } else { options.query.to_lowercase() };
    let mut stack = vec![options.root];
    let mut errors = 0;
    let mut matches = Vec::with_capacity(64);
    while let Some(dir) = stack.pop() {
        if cancelled() {
            break;
        }
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => {
                errors += 1;
                continue;
            }
        };
        for entry in entries {
            if cancelled() {
                break;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    errors += 1;
                    continue;
                }
            };
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !options.hidden && name.starts_with('.') {
                continue;
            }
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(_) => {
                    errors += 1;
                    continue;
                }
            };
            let path = entry.path();
            if kind.is_dir() && options.recursive {
                stack.push(path.clone());
            }
            let matches_name = if options.case_sensitive {
                name.contains(&needle)
            } else {
                name.to_lowercase().contains(&needle)
            };
            if (options.files_only && kind.is_dir()) || !matches_name {
                continue;
            }
            matches.push(path);
            if matches.len() == 64 {
                emit(Event::Matches(std::mem::take(&mut matches)));
            }
        }
        if !matches.is_empty() {
            emit(Event::Matches(std::mem::take(&mut matches)));
        }
    }
    emit(Event::Finished { errors, cancelled: cancelled() });
}
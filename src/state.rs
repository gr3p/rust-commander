use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

// Unique across all tabs, so a late listing can never land in a tab that moved after a close.
static TOKENS: AtomicU64 = AtomicU64::new(1);

fn fresh_token() -> u64 {
    TOKENS.fetch_add(1, Ordering::Relaxed)
}

use crate::entry::Entry;
use crate::listing::{self, SortKey};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneId {
    Left = 0,
    Right = 1,
}

impl PaneId {
    pub fn other(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(i: usize) -> Self {
        if i == 0 { Self::Left } else { Self::Right }
    }
}

#[derive(Clone, Copy, Debug)]
enum Step {
    Record,
    Replace,
    Back,
    Forward,
}

#[derive(Clone, Debug)]
pub struct Tab {
    pub path: PathBuf,
    pub entries: Vec<Entry>,
    pub cursor: usize,
    pub marked: HashSet<PathBuf>,
    pub back: Vec<PathBuf>,
    pub forward: Vec<PathBuf>,
    pub token: u64,
    /// Flat hit list. Entry paths are absolute (`FEED`), not children of `path`.
    pub feed: bool,
    // The tab stays in the old folder until the listing for `token` arrives.
    pending: Option<(PathBuf, Step)>,
}

impl Tab {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            entries: Vec::new(),
            cursor: 0,
            marked: HashSet::new(),
            back: Vec::new(),
            forward: Vec::new(),
            token: fresh_token(),
            feed: false,
            pending: None,
        }
    }

    fn wait_for(&mut self, path: PathBuf, step: Step) -> u64 {
        self.pending = Some((path, step));
        self.token = fresh_token();
        self.token
    }

    fn commit(&mut self, path: PathBuf, step: Step) {
        match step {
            Step::Record if self.path != path => {
                self.back.push(self.path.clone());
                self.forward.clear();
            }
            Step::Record | Step::Replace => {}
            Step::Back => {
                self.back.pop();
                self.forward.push(self.path.clone());
            }
            Step::Forward => {
                self.forward.pop();
                self.back.push(self.path.clone());
            }
        }
        self.path = path;
        self.cursor = 0;
        self.marked.clear();
    }
}

#[derive(Clone, Debug)]
pub struct Pane {
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub filter: String,
    pub sort: SortKey,
    pub ascending: bool,
    pub thumbs: bool,
}

impl Pane {
    fn new(path: PathBuf) -> Self {
        Self {
            tabs: vec![Tab::new(path)],
            active: 0,
            filter: String::new(),
            sort: SortKey::Name,
            ascending: true,
            thumbs: false,
        }
    }

    pub fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    pub fn tab_mut(&mut self) -> &mut Tab {
        let i = self.active;
        &mut self.tabs[i]
    }
}

#[derive(Clone, Debug)]
pub struct AppState {
    pub panes: [Pane; 2],
    pub active: PaneId,
    pub show_hidden: bool,
    pub quick_view: bool,
    pub panel_terminal: bool,
    pub preview_gen: u64,
    pub bookmarks: Vec<(String, PathBuf)>,
}

impl AppState {
    pub fn new(left: PathBuf, right: PathBuf) -> Self {
        Self {
            panes: [Pane::new(left), Pane::new(right)],
            active: PaneId::Left,
            show_hidden: false,
            quick_view: false,
            panel_terminal: false,
            preview_gen: 0,
            bookmarks: Vec::new(),
        }
    }

    pub fn pane(&self, id: PaneId) -> &Pane {
        &self.panes[id.index()]
    }

    pub fn pane_mut(&mut self, id: PaneId) -> &mut Pane {
        &mut self.panes[id.index()]
    }

    pub fn tab(&self, id: PaneId) -> &Tab {
        self.pane(id).tab()
    }

    pub fn tab_mut(&mut self, id: PaneId) -> &mut Tab {
        self.pane_mut(id).tab_mut()
    }

    pub fn visible(&self, id: PaneId) -> Vec<usize> {
        let pane = self.pane(id);
        listing::visible_indices(
            &pane.tab().entries,
            pane.sort,
            pane.ascending,
            self.show_hidden,
            &pane.filter,
        )
    }

    pub fn cursor_entry(&self, id: PaneId) -> Option<&Entry> {
        let vis = self.visible(id);
        let cursor = self.tab(id).cursor;
        vis.get(cursor).and_then(|&i| self.tab(id).entries.get(i))
    }

    pub fn show_hits(&mut self, id: PaneId, paths: &[PathBuf]) {
        let entries = paths.iter().filter_map(|path| hit_entry(path)).collect();
        let tab = self.tab_mut(id);
        tab.feed = true;
        tab.entries = entries;
        tab.cursor = 0;
        tab.marked.clear();
    }

    pub fn selected_paths(&self, id: PaneId) -> Vec<PathBuf> {
        let dir = self.tab(id).path.clone();
        let marked_set = self.tab(id).marked.clone();
        let vis = self.visible(id);
        let entries = &self.tab(id).entries;
        let marked: Vec<PathBuf> = vis
            .iter()
            .filter_map(|&i| {
                let entry = &entries[i];
                if entry.name.as_ref() == ".." {
                    return None;
                }
                let path = listing::entry_path(&dir, entry);
                marked_set.contains(&path).then_some(path)
            })
            .collect();
        if !marked.is_empty() {
            return marked;
        }
        self.cursor_entry(id)
            .filter(|e| e.name.as_ref() != "..")
            .map(|e| listing::entry_path(&dir, e))
            .into_iter()
            .collect()
    }

    pub fn toggle_mark(&mut self) {
        self.toggle_at_cursor(true);
    }

    // First press marks every visible row except `..`. When those are already
    // marked, the next press clears the whole set, including filtered rows.
    pub fn toggle_visible_marks(&mut self) -> bool {
        let id = self.active;
        let vis = self.visible(id);
        let dir = self.tab(id).path.clone();
        let paths: Vec<PathBuf> = vis
            .iter()
            .filter_map(|&i| {
                let entry = &self.tab(id).entries[i];
                if entry.name.as_ref() == ".." {
                    return None;
                }
                Some(listing::entry_path(&dir, entry))
            })
            .collect();
        if paths.is_empty() {
            return false;
        }
        let all = paths.iter().all(|path| self.tab(id).marked.contains(path));
        let tab = self.tab_mut(id);
        if all {
            tab.marked.clear();
            false
        } else {
            for path in paths {
                tab.marked.insert(path);
            }
            true
        }
    }

    pub fn toggle_mark_stay(&mut self) {
        self.toggle_at_cursor(false);
    }

    fn toggle_at_cursor(&mut self, advance: bool) {
        let id = self.active;
        let vis = self.visible(id);
        let cursor = self.tab(id).cursor;
        let Some(&idx) = vis.get(cursor) else {
            return;
        };
        let entry = &self.tab(id).entries[idx];
        if entry.name.as_ref() == ".." {
            return;
        }
        let path = listing::entry_path(&self.tab(id).path, entry);
        let tab = self.tab_mut(id);
        if !tab.marked.insert(path.clone()) {
            tab.marked.remove(&path);
        }
        if advance && cursor + 1 < vis.len() {
            tab.cursor = cursor + 1;
        }
        if advance && cursor + 1 < vis.len() {
            self.preview_gen = self.preview_gen.wrapping_add(1);
        }
    }

    pub fn mirror_to_other(&mut self) -> (PaneId, PathBuf, u64) {
        let src = self.active;
        let path = self.tab(src).path.clone();
        let dest = src.other();
        let token = self.navigate(dest, path.clone(), true);
        (dest, path, token)
    }

    pub fn navigate(&mut self, id: PaneId, path: PathBuf, record: bool) -> u64 {
        self.tab_mut(id).wait_for(path, if record { Step::Record } else { Step::Replace })
    }

    pub fn enter(&mut self) -> Option<(PaneId, PathBuf, u64)> {
        let id = self.active;
        let next = {
            let entry = self.cursor_entry(id)?;
            if !entry.is_dir() && entry.name.as_ref() != ".." {
                return None;
            }
            listing::entry_path(&self.tab(id).path, entry)
        };
        let token = self.navigate(id, next.clone(), true);
        Some((id, next, token))
    }

    pub fn up(&mut self) -> Option<(PaneId, PathBuf, u64)> {
        let id = self.active;
        let parent = self.tab(id).path.parent()?.to_path_buf();
        let token = self.navigate(id, parent.clone(), true);
        Some((id, parent, token))
    }

    // A refresh during a pending move reloads the target, not the folder on screen.
    pub fn refresh(&mut self, id: PaneId) -> (PathBuf, u64) {
        let tab = self.tab_mut(id);
        tab.token = fresh_token();
        let path = tab.pending.as_ref().map_or(&tab.path, |(path, _)| path).clone();
        (path, tab.token)
    }

    pub fn go_back(&mut self, id: PaneId) -> Option<(PathBuf, u64)> {
        let tab = self.tab_mut(id);
        let prev = tab.back.last()?.clone();
        Some((prev.clone(), tab.wait_for(prev, Step::Back)))
    }

    pub fn go_forward(&mut self, id: PaneId) -> Option<(PathBuf, u64)> {
        let tab = self.tab_mut(id);
        let next = tab.forward.last()?.clone();
        Some((next.clone(), tab.wait_for(next, Step::Forward)))
    }

    pub fn apply_listing(&mut self, id: PaneId, tab_index: usize, token: u64, entries: Vec<Entry>) -> bool {
        let show_hidden = self.show_hidden;
        let pane = self.pane_mut(id);
        let sort = pane.sort;
        let ascending = pane.ascending;
        let filter = pane.filter.clone();
        let Some(tab) = pane.tabs.iter_mut().find(|tab| tab.token == token) else {
            return false;
        };
        let _ = tab_index;
        // A reload of the same folder keeps the cursor on its file, not on its row number.
        let kept = tab.pending.is_none().then(|| {
            let visible = listing::visible_indices(&tab.entries, sort, ascending, show_hidden, &filter);
            visible.get(tab.cursor).map(|&slot| tab.entries[slot].name.clone())
        });
        // The filter belongs to the folder on screen. A new folder starts unfiltered.
        let left = tab.pending.as_ref().is_some_and(|(next, _)| next != &tab.path);
        if let Some((path, step)) = tab.pending.take() {
            tab.commit(path, step);
        }
        tab.feed = false;
        tab.entries = entries;
        let shown = if left { "" } else { filter.as_str() };
        let visible = listing::visible_indices(&tab.entries, sort, ascending, show_hidden, shown);
        if let Some(at) = kept.flatten().and_then(|name| visible.iter().position(|&slot| tab.entries[slot].name == name)) {
            tab.cursor = at;
        } else if visible.is_empty() {
            tab.cursor = 0;
        } else if tab.cursor >= visible.len() {
            tab.cursor = visible.len() - 1;
        }
        if left {
            pane.filter.clear();
        }
        true
    }

    // Failed or timed out: stay in the old folder. A fresh token makes a late answer miss.
    pub fn cancel_load(&mut self, id: PaneId, token: u64) -> bool {
        let Some(tab) = self.pane_mut(id).tabs.iter_mut().find(|tab| tab.token == token) else {
            return false;
        };
        tab.pending = None;
        tab.token = fresh_token();
        true
    }

    pub fn new_tab(&mut self, id: PaneId) -> (usize, PathBuf, u64) {
        let path = self.tab(id).path.clone();
        let pane = self.pane_mut(id);
        pane.tabs.push(Tab::new(path.clone()));
        pane.active = pane.tabs.len() - 1;
        let token = pane.tab().token;
        (pane.active, path, token)
    }

    pub fn close_tab(&mut self, id: PaneId) {
        let pane = self.pane_mut(id);
        if pane.tabs.len() == 1 {
            return;
        }
        pane.tabs.remove(pane.active);
        if pane.active >= pane.tabs.len() {
            pane.active = pane.tabs.len() - 1;
        }
    }

    pub fn next_tab(&mut self, id: PaneId, back: bool) {
        let pane = self.pane_mut(id);
        let n = pane.tabs.len();
        if n <= 1 {
            return;
        }
        if back {
            pane.active = (pane.active + n - 1) % n;
        } else {
            pane.active = (pane.active + 1) % n;
        }
    }

    pub fn move_cursor(&mut self, delta: isize) {
        let id = self.active;
        let len = self.visible(id).len() as isize;
        if len == 0 {
            return;
        }
        let cur = self.tab(id).cursor as isize;
        let next = (cur + delta).clamp(0, len - 1) as usize;
        self.tab_mut(id).cursor = next;
        self.preview_gen = self.preview_gen.wrapping_add(1);
    }

    pub fn set_cursor(&mut self, index: usize) {
        let id = self.active;
        let len = self.visible(id).len();
        if len == 0 {
            return;
        }
        self.tab_mut(id).cursor = index.min(len - 1);
        self.preview_gen = self.preview_gen.wrapping_add(1);
    }

    pub fn transfer_items(&self, same_dir_name: Option<&str>) -> Vec<(PathBuf, PathBuf)> {
        let sources = self.selected_paths(self.active);
        if let Some(name) = same_dir_name {
            return sources
                .into_iter()
                .take(1)
                .map(|src| {
                    let parent = src.parent().unwrap_or(Path::new(".")).to_path_buf();
                    (src, parent.join(name))
                })
                .collect();
        }
        let dest_dir = self.tab(self.active.other()).path.clone();
        sources
            .into_iter()
            .map(|src| {
                let name = src.file_name().unwrap_or_default().to_os_string();
                (src, dest_dir.join(name))
            })
            .collect()
    }
}

fn hit_entry(path: &Path) -> Option<Entry> {
    use std::os::unix::fs::MetadataExt;
    let name = path.file_name()?.to_string_lossy().into_owned();
    let meta = fs::symlink_metadata(path).ok()?;
    let mut flags = crate::entry::FEED;
    if meta.is_dir() && !meta.file_type().is_symlink() {
        flags |= crate::entry::DIR;
    }
    if meta.file_type().is_symlink() {
        flags |= crate::entry::LINK;
    }
    let mtime = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|dur| dur.as_secs() as i64)
        .unwrap_or(0);
    Some(Entry {
        name: name.into(),
        raw: Some(path.as_os_str().into()),
        size: meta.len(),
        mtime,
        mode: meta.mode(),
        flags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::{Entry, DIR};

    fn file(name: &str) -> Entry {
        Entry {
            name: name.into(),
            raw: None,
            size: 1,
            mtime: 0,
            mode: 0o644,
            flags: 0,
        }
    }

    #[test]
    fn ctrl_a_marks_visible_then_clears_all() {
        let mut state = AppState::new("/left".into(), "/right".into());
        state.tab_mut(PaneId::Left).entries = vec![Entry::dotdot(), file("a.txt"), file("b.txt")];
        assert!(state.toggle_visible_marks());
        let marked = &state.tab(PaneId::Left).marked;
        assert!(marked.contains(&PathBuf::from("/left/a.txt")));
        assert!(marked.contains(&PathBuf::from("/left/b.txt")));
        assert!(!marked.iter().any(|path| path.ends_with("..")));
        assert!(!state.toggle_visible_marks());
        assert!(state.tab(PaneId::Left).marked.is_empty());
    }

    #[test]
    fn ctrl_a_respects_the_filter() {
        let mut state = AppState::new("/left".into(), "/right".into());
        state.tab_mut(PaneId::Left).entries = vec![Entry::dotdot(), file("a.txt"), file("b.txt")];
        state.pane_mut(PaneId::Left).filter = "a".into();
        assert!(state.toggle_visible_marks());
        let marked = &state.tab(PaneId::Left).marked;
        assert!(marked.contains(&PathBuf::from("/left/a.txt")));
        assert!(!marked.contains(&PathBuf::from("/left/b.txt")));
    }

    fn dir(name: &str) -> Entry {
        Entry { flags: DIR, ..file(name) }
    }

    #[test]
    fn directory_change_drops_the_filter() {
        let mut state = AppState::new("/left".into(), "/right".into());
        state.tab_mut(PaneId::Left).entries = vec![Entry::dotdot(), dir("Documents"), file("notes.txt")];
        state.pane_mut(PaneId::Left).filter = "d".into();
        state.set_cursor(1);
        let (_, path, token) = state.enter().unwrap();
        assert_eq!(path, PathBuf::from("/left/Documents"));
        assert!(state.apply_listing(
            PaneId::Left,
            0,
            token,
            vec![Entry::dotdot(), file("budget.txt"), file("draft.txt")]
        ));
        assert!(state.pane(PaneId::Left).filter.is_empty());
        assert_eq!(state.visible(PaneId::Left).len(), 3);

        state.pane_mut(PaneId::Left).filter = "b".into();
        let (_, token) = state.refresh(PaneId::Left);
        assert!(state.apply_listing(
            PaneId::Left,
            0,
            token,
            vec![Entry::dotdot(), file("budget.txt"), file("draft.txt")]
        ));
        assert_eq!(state.pane(PaneId::Left).filter, "b");
        assert_eq!(state.visible(PaneId::Left).len(), 2);
    }

    #[test]
    fn failed_enter_keeps_the_filter() {
        let mut state = AppState::new("/left".into(), "/right".into());
        state.tab_mut(PaneId::Left).entries = vec![Entry::dotdot(), dir("Documents")];
        state.pane_mut(PaneId::Left).filter = "d".into();
        state.set_cursor(1);
        let (_, _, token) = state.enter().unwrap();
        assert!(state.cancel_load(PaneId::Left, token));
        assert_eq!(state.pane(PaneId::Left).filter, "d");
    }

    #[test]
    fn ctrl_a_clears_marks_hidden_by_the_filter() {
        let mut state = AppState::new("/left".into(), "/right".into());
        state.tab_mut(PaneId::Left).entries = vec![Entry::dotdot(), file("a.txt"), file("b.txt")];
        state.toggle_visible_marks();
        state.pane_mut(PaneId::Left).filter = "a".into();
        assert!(!state.toggle_visible_marks());
        assert!(state.tab(PaneId::Left).marked.is_empty());
    }
}

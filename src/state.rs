use std::cell::RefCell;
use std::collections::HashSet;
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
    // The tab stays in the old folder until the listing for `token` arrives.
    pending: Option<(PathBuf, Step)>,
    // Changes with every new listing, so the visible-row cache knows the entries changed.
    rev: u64,
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
            pending: None,
            rev: fresh_token(),
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

#[derive(Clone, Debug, PartialEq)]
struct VisibleKey {
    tab: usize,
    rev: u64,
    // Covers code that sets `entries` directly instead of through a listing.
    entries: (usize, usize),
    sort: SortKey,
    ascending: bool,
    show_hidden: bool,
    filter: String,
}

#[derive(Clone, Debug)]
pub struct Pane {
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub filter: String,
    pub sort: SortKey,
    pub ascending: bool,
    // `visible` runs several times per key. Large folders are only sorted again when an input changes.
    visible: RefCell<Option<(VisibleKey, Vec<usize>)>>,
}

impl Pane {
    fn new(path: PathBuf) -> Self {
        Self {
            tabs: vec![Tab::new(path)],
            active: 0,
            filter: String::new(),
            sort: SortKey::Name,
            ascending: true,
            visible: RefCell::new(None),
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
        let tab = pane.tab();
        let key = VisibleKey {
            tab: pane.active,
            rev: tab.rev,
            entries: (tab.entries.as_ptr() as usize, tab.entries.len()),
            sort: pane.sort,
            ascending: pane.ascending,
            show_hidden: self.show_hidden,
            filter: pane.filter.clone(),
        };
        if let Some((cached, vis)) = pane.visible.borrow().as_ref() {
            if *cached == key {
                return vis.clone();
            }
        }
        let vis = listing::visible_indices(&tab.entries, pane.sort, pane.ascending, self.show_hidden, &pane.filter);
        *pane.visible.borrow_mut() = Some((key, vis.clone()));
        vis
    }

    pub fn cursor_entry(&self, id: PaneId) -> Option<&Entry> {
        let vis = self.visible(id);
        let cursor = self.tab(id).cursor;
        vis.get(cursor).and_then(|&i| self.tab(id).entries.get(i))
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
        // Going up lands on the folder you came from.
        let land = tab.pending.take().and_then(|(path, step)| {
            let old = tab.path.clone();
            tab.commit(path, step);
            let first = old.strip_prefix(&tab.path).ok()?.components().next()?;
            Some(tab.path.join(first))
        });
        tab.entries = entries;
        tab.rev = fresh_token();
        let vis = listing::visible_indices(&tab.entries, sort, ascending, show_hidden, &filter);
        if let Some(pos) = land.and_then(|land| vis.iter().position(|&i| listing::entry_path(&tab.path, &tab.entries[i]) == land)) {
            tab.cursor = pos;
        }
        if vis.is_empty() {
            tab.cursor = 0;
        } else if tab.cursor >= vis.len() {
            tab.cursor = vis.len() - 1;
        }
        true
    }

    // Failed or timed out: stay in the old folder. A fresh token makes a late answer miss.
    pub fn cancel_load(&mut self, id: PaneId, token: u64) -> bool {
        let Some(tab) = self.pane_mut(id).tabs.iter_mut().find(|tab| tab.token == token) else {
            return false;
        };
        // A history entry that cannot load is dropped, so the older ones stay reachable.
        match tab.pending.take() {
            Some((_, Step::Back)) => drop(tab.back.pop()),
            Some((_, Step::Forward)) => drop(tab.forward.pop()),
            _ => {}
        }
        tab.token = fresh_token();
        true
    }

    // Hidden files, the filter or a tab switch can shrink the list under the cursor.
    pub fn clamp_cursors(&mut self) {
        let show_hidden = self.show_hidden;
        for pane in &mut self.panes {
            for tab in &mut pane.tabs {
                let len = listing::visible_indices(&tab.entries, pane.sort, pane.ascending, show_hidden, &pane.filter).len();
                tab.cursor = tab.cursor.min(len.saturating_sub(1));
            }
        }
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

    pub fn select_tab(&mut self, id: PaneId, index: usize) {
        let pane = self.pane_mut(id);
        if index < pane.tabs.len() {
            pane.active = index;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::Entry;

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

    #[test]
    fn ctrl_a_clears_marks_hidden_by_the_filter() {
        let mut state = AppState::new("/left".into(), "/right".into());
        state.tab_mut(PaneId::Left).entries = vec![Entry::dotdot(), file("a.txt"), file("b.txt")];
        state.toggle_visible_marks();
        state.pane_mut(PaneId::Left).filter = "a".into();
        assert!(!state.toggle_visible_marks());
        assert!(state.tab(PaneId::Left).marked.is_empty());
    }

    fn dir(name: &str) -> Entry {
        Entry { mode: 0o755, flags: crate::entry::DIR, ..file(name) }
    }

    #[test]
    fn up_lands_on_the_folder_you_came_from() {
        let mut state = AppState::new("/a/b".into(), "/right".into());
        let (id, _, token) = state.up().unwrap();
        state.apply_listing(id, 0, token, vec![Entry::dotdot(), dir("x"), dir("b"), dir("y")]);
        assert_eq!(state.tab(id).path, PathBuf::from("/a"));
        assert_eq!(state.cursor_entry(id).map(|e| e.name.to_string()), Some("b".into()));
    }

    #[test]
    fn failed_back_step_drops_the_entry() {
        let mut state = AppState::new("/one".into(), "/right".into());
        let token = state.navigate(PaneId::Left, "/two".into(), true);
        state.apply_listing(PaneId::Left, 0, token, vec![Entry::dotdot()]);
        let token = state.navigate(PaneId::Left, "/three".into(), true);
        state.apply_listing(PaneId::Left, 0, token, vec![Entry::dotdot()]);
        let (_, token) = state.go_back(PaneId::Left).unwrap();
        state.cancel_load(PaneId::Left, token);
        assert_eq!(state.go_back(PaneId::Left).map(|(path, _)| path), Some(PathBuf::from("/one")));
    }

    #[test]
    fn visible_follows_new_listings_and_sort() {
        let mut state = AppState::new("/left".into(), "/right".into());
        let token = state.tab(PaneId::Left).token;
        state.apply_listing(PaneId::Left, 0, token, vec![Entry::dotdot(), file("b"), file("a")]);
        assert_eq!(state.visible(PaneId::Left), vec![0, 2, 1]);
        state.pane_mut(PaneId::Left).ascending = false;
        assert_eq!(state.visible(PaneId::Left), vec![0, 1, 2]);
        let token = state.refresh(PaneId::Left).1;
        state.apply_listing(PaneId::Left, 0, token, vec![Entry::dotdot(), file("c")]);
        assert_eq!(state.visible(PaneId::Left), vec![0, 1]);
    }

    #[test]
    fn hiding_files_clamps_the_cursor() {
        let mut state = AppState::new("/left".into(), "/right".into());
        state.show_hidden = true;
        let hidden = |name| Entry { flags: crate::entry::HIDDEN, ..file(name) };
        state.tab_mut(PaneId::Left).entries = vec![Entry::dotdot(), file("a"), hidden(".b"), hidden(".c")];
        state.set_cursor(3);
        state.show_hidden = false;
        state.clamp_cursors();
        assert!(state.cursor_entry(PaneId::Left).is_some());
    }
}

use std::collections::HashSet;
use std::path::{Path, PathBuf};

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

#[derive(Clone, Debug)]
pub struct Tab {
    pub path: PathBuf,
    pub entries: Vec<Entry>,
    pub cursor: usize,
    pub marked: HashSet<PathBuf>,
    pub back: Vec<PathBuf>,
    pub forward: Vec<PathBuf>,
    pub token: u64,
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
            token: 1,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Pane {
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub filter: String,
    pub sort: SortKey,
    pub ascending: bool,
}

impl Pane {
    fn new(path: PathBuf) -> Self {
        Self {
            tabs: vec![Tab::new(path)],
            active: 0,
            filter: String::new(),
            sort: SortKey::Name,
            ascending: true,
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
        let entry_name = self.tab(id).entries[idx].name.clone();
        if entry_name.as_ref() == ".." {
            return;
        }
        let path = self.tab(id).path.join(entry_name.as_ref());
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
        let tab = self.tab_mut(id);
        if record && tab.path != path {
            tab.back.push(tab.path.clone());
            tab.forward.clear();
        }
        tab.path = path;
        tab.entries.clear();
        tab.cursor = 0;
        tab.marked.clear();
        tab.token += 1;
        tab.token
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

    pub fn refresh(&mut self, id: PaneId) -> (PathBuf, u64) {
        let tab = self.tab_mut(id);
        tab.token += 1;
        (tab.path.clone(), tab.token)
    }

    pub fn go_back(&mut self, id: PaneId) -> Option<(PathBuf, u64)> {
        let prev = self.tab_mut(id).back.pop()?;
        let tab = self.tab_mut(id);
        tab.forward.push(tab.path.clone());
        tab.path = prev.clone();
        tab.entries.clear();
        tab.cursor = 0;
        tab.marked.clear();
        tab.token += 1;
        Some((prev, tab.token))
    }

    pub fn go_forward(&mut self, id: PaneId) -> Option<(PathBuf, u64)> {
        let next = self.tab_mut(id).forward.pop()?;
        let tab = self.tab_mut(id);
        tab.back.push(tab.path.clone());
        tab.path = next.clone();
        tab.entries.clear();
        tab.cursor = 0;
        tab.marked.clear();
        tab.token += 1;
        Some((next, tab.token))
    }

    pub fn apply_listing(&mut self, id: PaneId, tab_index: usize, token: u64, entries: Vec<Entry>) -> bool {
        let show_hidden = self.show_hidden;
        let pane = self.pane_mut(id);
        let sort = pane.sort;
        let ascending = pane.ascending;
        let filter = pane.filter.clone();
        let Some(tab) = pane.tabs.get_mut(tab_index) else {
            return false;
        };
        if tab.token != token {
            return false;
        }
        tab.entries = entries;
        let vis_len = listing::visible_indices(&tab.entries, sort, ascending, show_hidden, &filter).len();
        if vis_len == 0 {
            tab.cursor = 0;
        } else if tab.cursor >= vis_len {
            tab.cursor = vis_len - 1;
        }
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

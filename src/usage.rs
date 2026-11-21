// Disk usage of a folder: the size of every direct child, counted by a pool of threads.
// No recursion anywhere. The tree is walked with a work stack on the heap, so depth cannot overflow the stack.
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// A branch deeper than this is not entered. A bind mount loop or a broken tree ends here.
pub const MAX_DEPTH: u32 = 512;
// Folders down to this depth get a total of their own, which the cache can keep.
const NODE_DEPTH: u32 = 3;
const MAX_NODES: usize = 50_000;
const TICK: Duration = Duration::from_millis(100);
// How often a long folder listing looks at the cancel flag and hands in its count.
const CHECK_EVERY: u64 = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Mode {
    /// File lengths, not the space on disk.
    pub apparent: bool,
    /// Do not enter a folder on another mount.
    pub same_device: bool,
}

impl Mode {
    pub const DISK: Mode = Mode { apparent: false, same_device: true };
}

#[derive(Clone, Debug)]
pub struct Options {
    pub root: PathBuf,
    pub mode: Mode,
    pub threads: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Child {
    pub name: OsString,
    pub is_dir: bool,
    /// A folder on another mount that was left alone.
    pub foreign: bool,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub children: Arc<[Child]>,
    /// One value per child, in the order of `children`.
    pub bytes: Vec<u64>,
    pub total: u64,
    /// Names read so far. Stands still when the disk or the server does not answer.
    pub entries: u64,
    pub current: PathBuf,
}

#[derive(Debug)]
pub struct Report {
    pub children: Arc<[Child]>,
    pub bytes: Vec<u64>,
    pub total: u64,
    pub entries: u64,
    pub unreadable: usize,
    pub too_deep: usize,
    pub skipped_mounts: usize,
    /// Folders down to the third level with the size of everything below them, root first.
    pub nodes: Vec<(PathBuf, u64)>,
    pub cancelled: bool,
    /// A worker panicked. The numbers are partial.
    pub failed: bool,
}

struct NodeMeta {
    path: PathBuf,
    parent: u32,
}

struct Work {
    node: u32,
    top: u32,
    dir: PathBuf,
    depth: u32,
}

struct RootScan {
    children: Vec<Child>,
    bytes: Vec<u64>,
    own: Vec<u64>,
    nodes: Vec<NodeMeta>,
    work: Vec<Work>,
    dirs: HashSet<(u64, u64)>,
    links: HashSet<(u64, u64)>,
    entries: u64,
    unreadable: usize,
    skipped: usize,
    dev: u64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panicked worker must not take the others, or the window, down with it.
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn weigh(md: &fs::Metadata, mode: Mode) -> u64 {
    if mode.apparent {
        if md.file_type().is_file() { md.len() } else { 0 }
    } else {
        md.blocks().saturating_mul(512)
    }
}

// A second name for a file already counted. Only files with several links need the set.
fn repeat(links: &mut HashSet<(u64, u64)>, md: &fs::Metadata) -> bool {
    md.nlink() > 1 && !md.is_dir() && !links.insert((md.dev(), md.ino()))
}

fn scan_root(opts: &Options, cancelled: &impl Fn() -> bool) -> io::Result<Option<RootScan>> {
    let meta = fs::symlink_metadata(&opts.root)?;
    let mut scan = RootScan {
        children: Vec::new(),
        bytes: Vec::new(),
        own: vec![weigh(&meta, opts.mode)],
        nodes: vec![NodeMeta { path: opts.root.clone(), parent: 0 }],
        work: Vec::new(),
        dirs: HashSet::new(),
        links: HashSet::new(),
        entries: 0,
        unreadable: 0,
        skipped: 0,
        dev: meta.dev(),
    };
    scan.dirs.insert((meta.dev(), meta.ino()));
    for entry in fs::read_dir(&opts.root)? {
        scan.entries += 1;
        if scan.entries % CHECK_EVERY == 0 && cancelled() {
            return Ok(None);
        }
        let Ok(entry) = entry else {
            scan.unreadable += 1;
            continue;
        };
        let md = match entry.metadata() {
            Ok(md) => md,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => {
                scan.unreadable += 1;
                continue;
            }
        };
        let top = scan.children.len() as u32;
        let mut child = Child { name: entry.file_name(), is_dir: md.is_dir(), foreign: false };
        let own = weigh(&md, opts.mode);
        if md.is_dir() {
            if opts.mode.same_device && md.dev() != scan.dev {
                child.foreign = true;
                scan.skipped += 1;
                scan.own[0] += own;
            } else if scan.dirs.insert((md.dev(), md.ino())) {
                let node = scan.nodes.len() as u32;
                scan.nodes.push(NodeMeta { path: entry.path(), parent: 0 });
                scan.own.push(own);
                scan.work.push(Work { node, top, dir: entry.path(), depth: 1 });
            } else {
                scan.own[0] += own;
            }
            scan.bytes.push(own);
        } else {
            let weight = if repeat(&mut scan.links, &md) { 0 } else { own };
            scan.own[0] += weight;
            scan.bytes.push(weight);
        }
        scan.children.push(child);
    }
    Ok(Some(scan))
}

struct Shared<'a> {
    opts: &'a Options,
    dev: u64,
    queue: Mutex<Vec<Work>>,
    wake: Condvar,
    // Folders queued or being read. Zero means the walk is over.
    pending: AtomicUsize,
    stop: AtomicBool,
    failed: AtomicBool,
    own: Vec<AtomicU64>,
    top: Vec<AtomicU64>,
    total: AtomicU64,
    nodes: Mutex<Vec<NodeMeta>>,
    dirs: Mutex<HashSet<(u64, u64)>>,
    links: Mutex<HashSet<(u64, u64)>>,
    entries: AtomicU64,
    unreadable: AtomicUsize,
    too_deep: AtomicUsize,
    skipped: AtomicUsize,
    current: Mutex<PathBuf>,
}

impl Shared<'_> {
    fn over(&self) -> bool {
        self.stop.load(Ordering::Relaxed) || self.pending.load(Ordering::Acquire) == 0
    }

    fn add_own(&self, node: u32, n: u64) {
        if n > 0 {
            self.own[node as usize].fetch_add(n, Ordering::Relaxed);
            self.total.fetch_add(n, Ordering::Relaxed);
        }
    }

    fn add_top(&self, top: u32, n: u64) {
        if n > 0 {
            self.top[top as usize].fetch_add(n, Ordering::Relaxed);
        }
    }

    // A folder near the top gets its own total. Past the cap it counts for the nearest one above.
    fn node_for(&self, dir: &Path, parent: u32) -> u32 {
        let mut nodes = lock(&self.nodes);
        if nodes.len() < self.own.len() {
            nodes.push(NodeMeta { path: dir.to_path_buf(), parent });
            return (nodes.len() - 1) as u32;
        }
        parent
    }

    fn push(&self, work: Work) {
        self.pending.fetch_add(1, Ordering::AcqRel);
        lock(&self.queue).push(work);
        self.wake.notify_one();
    }

    fn next(&self, cancelled: &impl Fn() -> bool) -> Option<Work> {
        let mut queue = lock(&self.queue);
        loop {
            if cancelled() {
                self.stop.store(true, Ordering::Relaxed);
            }
            if self.stop.load(Ordering::Relaxed) {
                return None;
            }
            if let Some(work) = queue.pop() {
                return Some(work);
            }
            if self.pending.load(Ordering::Acquire) == 0 {
                return None;
            }
            queue = self.wake.wait_timeout(queue, TICK).unwrap_or_else(|poisoned| poisoned.into_inner()).0;
        }
    }

    fn finish_one(&self) {
        if self.pending.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.wake.notify_all();
        }
    }
}

fn process(sh: &Shared, work: &Work, cancelled: &impl Fn() -> bool) {
    #[cfg(test)]
    if work.dir.file_name().is_some_and(|name| name == "__panic__") {
        panic!("test panic");
    }
    if let Ok(mut current) = sh.current.try_lock() {
        current.clone_from(&work.dir);
    }
    let read = match fs::read_dir(&work.dir) {
        Ok(read) => read,
        Err(err) => {
            if err.kind() != io::ErrorKind::NotFound {
                sh.unreadable.fetch_add(1, Ordering::Relaxed);
            }
            return;
        }
    };
    let mode = sh.opts.mode;
    // What the folder's own files add, to its node and to the top-level child it sits under.
    let (mut node_sum, mut top_sum, mut seen) = (0u64, 0u64, 0u64);
    for entry in read {
        seen += 1;
        if seen % CHECK_EVERY == 0 {
            sh.add_own(work.node, std::mem::take(&mut node_sum));
            sh.add_top(work.top, std::mem::take(&mut top_sum));
            sh.entries.fetch_add(CHECK_EVERY, Ordering::Relaxed);
            if cancelled() {
                sh.stop.store(true, Ordering::Relaxed);
            }
            if sh.stop.load(Ordering::Relaxed) {
                sh.entries.fetch_add(seen % CHECK_EVERY, Ordering::Relaxed);
                return;
            }
        }
        let Ok(entry) = entry else {
            sh.unreadable.fetch_add(1, Ordering::Relaxed);
            continue;
        };
        let md = match entry.metadata() {
            Ok(md) => md,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => {
                sh.unreadable.fetch_add(1, Ordering::Relaxed);
                continue;
            }
        };
        let weight = weigh(&md, mode);
        if !md.is_dir() {
            let weight = if repeat(&mut lock(&sh.links), &md) { 0 } else { weight };
            node_sum += weight;
            top_sum += weight;
            continue;
        }
        if mode.same_device && md.dev() != sh.dev {
            sh.skipped.fetch_add(1, Ordering::Relaxed);
            node_sum += weight;
            top_sum += weight;
            continue;
        }
        // The same folder reached twice, which only a bind mount can do.
        if !lock(&sh.dirs).insert((md.dev(), md.ino())) {
            continue;
        }
        if work.depth + 1 > MAX_DEPTH {
            sh.too_deep.fetch_add(1, Ordering::Relaxed);
            node_sum += weight;
            top_sum += weight;
            continue;
        }
        let path = entry.path();
        let node = if work.depth < NODE_DEPTH { sh.node_for(&path, work.node) } else { work.node };
        sh.add_own(node, weight);
        top_sum += weight;
        sh.push(Work { node, top: work.top, dir: path, depth: work.depth + 1 });
    }
    sh.add_own(work.node, node_sum);
    sh.add_top(work.top, top_sum);
    sh.entries.fetch_add(seen % CHECK_EVERY, Ordering::Relaxed);
}

fn worker(sh: &Shared, cancelled: &impl Fn() -> bool) {
    while let Some(work) = sh.next(cancelled) {
        let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| process(sh, &work, cancelled)));
        if done.is_err() {
            sh.failed.store(true, Ordering::Relaxed);
            sh.stop.store(true, Ordering::Relaxed);
            sh.wake.notify_all();
            return;
        }
        sh.finish_one();
    }
}

/// Counts `opts.root`. `progress` is called about ten times a second from the calling thread.
/// A root that is a symlink is an error; a root that is a file reports its own size.
pub fn run(
    opts: &Options,
    cancelled: &(impl Fn() -> bool + Sync),
    progress: &mut dyn FnMut(&Snapshot),
) -> io::Result<Report> {
    let meta = fs::symlink_metadata(&opts.root)?;
    if meta.file_type().is_symlink() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "symlink"));
    }
    if !meta.is_dir() {
        let total = weigh(&meta, opts.mode);
        return Ok(Report {
            children: Arc::from(Vec::new()),
            bytes: Vec::new(),
            total,
            entries: 1,
            unreadable: 0,
            too_deep: 0,
            skipped_mounts: 0,
            nodes: Vec::new(),
            cancelled: false,
            failed: false,
        });
    }
    let Some(root) = scan_root(opts, cancelled)? else {
        return Ok(Report {
            children: Arc::from(Vec::new()),
            bytes: Vec::new(),
            total: 0,
            entries: 0,
            unreadable: 0,
            too_deep: 0,
            skipped_mounts: 0,
            nodes: Vec::new(),
            cancelled: true,
            failed: false,
        });
    };
    let RootScan { children, bytes, own, nodes, work, dirs, links, entries, unreadable, skipped, dev } = root;
    let children: Arc<[Child]> = Arc::from(children);
    let room = nodes.len() + MAX_NODES;
    let total: u64 = own.iter().sum();
    let sh = Shared {
        opts,
        dev,
        queue: Mutex::new(Vec::new()),
        wake: Condvar::new(),
        pending: AtomicUsize::new(0),
        stop: AtomicBool::new(false),
        failed: AtomicBool::new(false),
        own: (0..room).map(|i| AtomicU64::new(own.get(i).copied().unwrap_or(0))).collect(),
        top: bytes.iter().map(|&n| AtomicU64::new(n)).collect(),
        total: AtomicU64::new(total),
        nodes: Mutex::new(nodes),
        dirs: Mutex::new(dirs),
        links: Mutex::new(links),
        entries: AtomicU64::new(entries),
        unreadable: AtomicUsize::new(unreadable),
        too_deep: AtomicUsize::new(0),
        skipped: AtomicUsize::new(skipped),
        current: Mutex::new(opts.root.clone()),
    };
    let snapshot = |sh: &Shared| Snapshot {
        children: Arc::clone(&children),
        bytes: sh.top.iter().map(|n| n.load(Ordering::Relaxed)).collect(),
        total: sh.total.load(Ordering::Relaxed),
        entries: sh.entries.load(Ordering::Relaxed),
        current: lock(&sh.current).clone(),
    };
    progress(&snapshot(&sh));
    let workers = if work.is_empty() { 0 } else { opts.threads.max(1) };
    for item in work {
        sh.push(item);
    }
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| worker(&sh, cancelled));
        }
        let mut last = sh.entries.load(Ordering::Relaxed);
        while workers > 0 {
            {
                let queue = lock(&sh.queue);
                let _ = sh.wake.wait_timeout(queue, TICK);
            }
            if cancelled() {
                sh.stop.store(true, Ordering::Relaxed);
                sh.wake.notify_all();
            }
            if sh.over() {
                break;
            }
            let now = sh.entries.load(Ordering::Relaxed);
            if now != last {
                last = now;
                progress(&snapshot(&sh));
            }
        }
    });
    let nodes = lock(&sh.nodes);
    let mut sums: Vec<u64> = (0..nodes.len()).map(|i| sh.own[i].load(Ordering::Relaxed)).collect();
    for i in (1..nodes.len()).rev() {
        let parent = nodes[i].parent as usize;
        sums[parent] = sums[parent].saturating_add(sums[i]);
    }
    let report = Report {
        children,
        bytes: sh.top.iter().map(|n| n.load(Ordering::Relaxed)).collect(),
        total: sums.first().copied().unwrap_or(0),
        entries: sh.entries.load(Ordering::Relaxed),
        unreadable: sh.unreadable.load(Ordering::Relaxed),
        too_deep: sh.too_deep.load(Ordering::Relaxed),
        skipped_mounts: sh.skipped.load(Ordering::Relaxed),
        nodes: nodes.iter().zip(&sums).map(|(node, sum)| (node.path.clone(), *sum)).collect(),
        cancelled: sh.stop.load(Ordering::Relaxed) && !sh.failed.load(Ordering::Relaxed),
        failed: sh.failed.load(Ordering::Relaxed),
    };
    Ok(report)
}

/// Share of `total` in whole percent, 0 to 100. A total of zero gives 0, never NaN.
pub fn percent(part: u64, total: u64) -> f64 {
    if total == 0 { 0.0 } else { (part as f64 / total as f64 * 100.0).clamp(0.0, 100.0) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Medium {
    Fast,
    Rotational,
    Network,
}

pub fn is_network_fs(fstype: &str) -> bool {
    matches!(fstype, "cifs" | "smb" | "smb3" | "nfs" | "nfs4" | "ceph" | "glusterfs" | "9p" | "afs")
        || fstype == "fuse"
        || fstype.starts_with("fuse.")
}

pub fn medium_from(fstype: &str, rotational: Option<bool>) -> Medium {
    if is_network_fs(fstype) {
        Medium::Network
    } else if rotational == Some(true) {
        Medium::Rotational
    } else {
        Medium::Fast
    }
}

/// A spinning disk seeks itself slow with many readers, and a server answers one question at a time.
pub fn threads_for(medium: Medium, cores: usize) -> usize {
    match medium {
        Medium::Fast => cores.clamp(2, 8),
        Medium::Rotational => 1,
        Medium::Network => 3,
    }
}

/// What the folder lives on, from /proc/mounts and /sys.
pub fn medium_of(root: &Path) -> Medium {
    let fstype = fs::read_to_string("/proc/mounts")
        .ok()
        .and_then(|text| crate::places::mount_entry(&text, root))
        .map(|(_, _, fstype, _)| fstype)
        .unwrap_or_default();
    let rotational = fs::metadata(root).ok().and_then(|md| rotational(md.dev()));
    medium_from(&fstype, rotational)
}

fn rotational(dev: u64) -> Option<bool> {
    let (major, minor) = (libc::major(dev), libc::minor(dev));
    let base = PathBuf::from(format!("/sys/dev/block/{major}:{minor}"));
    // A partition keeps the queue settings on the disk above it.
    let text = fs::read_to_string(base.join("queue/rotational")).or_else(|_| fs::read_to_string(base.join("../queue/rotational"))).ok()?;
    Some(text.trim() == "1")
}

pub fn cores() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get())
}

/// Notices a count that stands still: a server that stopped answering, a disk that sleeps.
#[derive(Clone, Copy, Debug)]
pub struct Watchdog {
    entries: u64,
    moved: Instant,
}

impl Watchdog {
    pub fn new(now: Instant) -> Self {
        Self { entries: 0, moved: now }
    }

    pub fn feed(&mut self, entries: u64, now: Instant) {
        if entries != self.entries {
            self.entries = entries;
            self.moved = now;
        }
    }

    pub fn stalled(&self, now: Instant, limit: Duration) -> bool {
        now.saturating_duration_since(self.moved) >= limit
    }

    pub fn quiet_for(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.moved)
    }
}

/// The mount point and file system type holding `dir`.
pub fn mount_of(dir: &Path) -> Option<(PathBuf, String)> {
    let text = fs::read_to_string("/proc/mounts").ok()?;
    crate::places::mount_entry(&text, dir).map(|(mount, _, fstype, _)| (mount, fstype))
}

pub const CACHE_CAP: usize = 50_000;
/// Smaller folders are counted again in a blink, so they are not worth a place.
pub const MIN_CACHED: u64 = 1 << 20;
pub const TTL: Duration = Duration::from_secs(10 * 60);
pub const TTL_NETWORK: Duration = Duration::from_secs(30 * 60);

struct Cached {
    bytes: u64,
    at: Instant,
    ttl: Duration,
    stale: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub bytes: u64,
    pub age: Duration,
    /// Too old, or something changed below it. Fine to show, but count again.
    pub stale: bool,
}

pub struct Cache {
    map: HashMap<(PathBuf, Mode), Cached>,
    cap: usize,
}

impl Default for Cache {
    fn default() -> Self {
        Self::with_cap(CACHE_CAP)
    }
}

impl Cache {
    pub fn with_cap(cap: usize) -> Self {
        Self { map: HashMap::new(), cap }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Keeps every folder of a finished count that is big enough, the root included.
    pub fn store(&mut self, report: &Report, mode: Mode, network: bool, now: Instant) {
        if report.cancelled || report.failed {
            return;
        }
        let ttl = if network { TTL_NETWORK } else { TTL };
        for (path, bytes) in &report.nodes {
            if *bytes >= MIN_CACHED {
                self.map.insert((path.clone(), mode), Cached { bytes: *bytes, at: now, ttl, stale: false });
            }
        }
        if self.map.len() > self.cap {
            let mut ages: Vec<_> = self.map.iter().map(|(key, cached)| (cached.at, key.clone())).collect();
            ages.sort_by_key(|(at, _)| *at);
            // A tenth extra, so the next store does not sort again.
            let drop = self.map.len() - self.cap + self.cap / 10;
            for (_, key) in ages.into_iter().take(drop) {
                self.map.remove(&key);
            }
        }
    }

    pub fn get(&self, path: &Path, mode: Mode, now: Instant) -> Option<Hit> {
        let cached = self.map.get(&(path.to_path_buf(), mode))?;
        let age = now.saturating_duration_since(cached.at);
        Some(Hit { bytes: cached.bytes, age, stale: cached.stale || age > cached.ttl })
    }

    /// Something changed in `path`. It and every folder above it keep their number but count as stale.
    pub fn touch(&mut self, path: &Path) {
        for ((key, _), cached) in self.map.iter_mut() {
            if path.starts_with(key) {
                cached.stale = true;
            }
        }
    }

    /// `path` was removed or replaced. Whatever was kept below it is wrong.
    pub fn forget_below(&mut self, path: &Path) {
        self.map.retain(|(key, _), _| !(key.starts_with(path) && key != path));
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }
}

/// The share as the label beside the bar: whole percent, and "<1%" for something small but not empty.
pub fn percent_text(part: u64, total: u64) -> String {
    let share = percent(part, total);
    if part > 0 && share < 0.5 { "<1%".into() } else { format!("{share:.0}%") }
}

/// What one row shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub bytes: u64,
    /// 0 to 1, the share of everything in the folder.
    pub frac: f64,
    pub text: String,
    /// A folder on another mount: no number, no bar.
    pub foreign: bool,
}

#[derive(Clone, Debug)]
struct Item {
    bytes: u64,
    foreign: bool,
    known: bool,
}

/// The sizes of the children of one folder, in the form the panel draws them. Kept apart from the entries so a
/// refresh of the listing does not lose them.
#[derive(Clone, Debug)]
pub struct View {
    pub path: PathBuf,
    pub mode: Mode,
    /// Still counting: the numbers can grow.
    pub partial: bool,
    /// Taken from an earlier count. Shown dimmed until a new one replaces it.
    pub stale: bool,
    items: HashMap<String, Item>,
    total: u64,
}

impl View {
    pub fn from_counts(path: &Path, mode: Mode, children: &[Child], bytes: &[u64], partial: bool) -> Self {
        let items: HashMap<String, Item> = children
            .iter()
            .zip(bytes)
            .map(|(child, bytes)| {
                (child.name.to_string_lossy().into_owned(), Item { bytes: *bytes, foreign: child.foreign, known: true })
            })
            .collect();
        let mut view = Self { path: path.to_path_buf(), mode, partial, stale: false, items, total: 0 };
        view.total = view.sum();
        view
    }

    /// What the cache knows before the count has said anything: folders it kept, files by their length.
    /// `entries` is (name, is a folder, length).
    pub fn prefilled(path: &Path, mode: Mode, entries: &[(String, bool, u64)], cache: &Cache, now: Instant) -> Self {
        let items = entries
            .iter()
            .map(|(name, is_dir, len)| {
                let item = if *is_dir {
                    match cache.get(&path.join(name), mode, now) {
                        Some(hit) => Item { bytes: hit.bytes, foreign: false, known: true },
                        None => Item { bytes: 0, foreign: false, known: false },
                    }
                } else {
                    Item { bytes: *len, foreign: false, known: true }
                };
                (name.clone(), item)
            })
            .collect();
        let mut view = Self { path: path.to_path_buf(), mode, partial: true, stale: true, items, total: 0 };
        view.total = view.sum();
        view
    }

    fn sum(&self) -> u64 {
        self.items.values().filter(|item| item.known && !item.foreign).fold(0, |sum, item| sum.saturating_add(item.bytes))
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// None for a name the count has not reached yet, or does not know.
    pub fn cell(&self, name: &str) -> Option<Cell> {
        let item = self.items.get(name)?;
        if item.foreign {
            return Some(Cell { bytes: 0, frac: 0.0, text: String::new(), foreign: true });
        }
        if !item.known {
            return None;
        }
        Some(Cell {
            bytes: item.bytes,
            frac: percent(item.bytes, self.total) / 100.0,
            text: percent_text(item.bytes, self.total),
            foreign: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rc-usage-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn apparent() -> Mode {
        Mode { apparent: true, same_device: true }
    }

    fn count(root: &Path, mode: Mode, threads: usize) -> Report {
        let opts = Options { root: root.to_path_buf(), mode, threads };
        run(&opts, &|| false, &mut |_| {}).unwrap()
    }

    fn bytes_of(report: &Report, name: &str) -> u64 {
        let at = report.children.iter().position(|c| c.name == name).unwrap();
        report.bytes[at]
    }

    fn write(path: &Path, len: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![b'x'; len]).unwrap();
    }

    #[test]
    fn sums_nested_folders_per_child() {
        let dir = scratch("nested");
        write(&dir.join("a/one"), 100);
        write(&dir.join("a/deep/er/two"), 20);
        write(&dir.join("b/three"), 5);
        write(&dir.join("top"), 7);
        let report = count(&dir, apparent(), 4);
        assert_eq!(report.total, 132);
        assert_eq!(bytes_of(&report, "a"), 120);
        assert_eq!(bytes_of(&report, "b"), 5);
        assert_eq!(bytes_of(&report, "top"), 7);
        assert!(!report.cancelled && !report.failed);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hard_link_counts_once() {
        let dir = scratch("hard");
        write(&dir.join("a/file"), 50);
        fs::create_dir_all(dir.join("b")).unwrap();
        fs::hard_link(dir.join("a/file"), dir.join("b/other")).unwrap();
        fs::hard_link(dir.join("a/file"), dir.join("third")).unwrap();
        assert_eq!(count(&dir, apparent(), 4).total, 50);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn symlinks_are_not_followed() {
        let dir = scratch("links");
        write(&dir.join("real/file"), 10);
        symlink("/etc", dir.join("to_etc")).unwrap();
        symlink("/etc/hosts", dir.join("to_hosts")).unwrap();
        symlink(&dir, dir.join("real/loop")).unwrap();
        let report = count(&dir, apparent(), 2);
        assert_eq!(report.total, 10);
        assert_eq!(report.unreadable, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unreadable_folder_does_not_stop_the_count() {
        let dir = scratch("denied");
        write(&dir.join("ok/file"), 9);
        write(&dir.join("locked/file"), 100);
        write(&dir.join("also/inner/file"), 1);
        fs::set_permissions(dir.join("locked"), fs::Permissions::from_mode(0)).unwrap();
        let report = count(&dir, apparent(), 3);
        fs::set_permissions(dir.join("locked"), fs::Permissions::from_mode(0o700)).unwrap();
        if unsafe { libc::geteuid() } != 0 {
            assert_eq!(report.total, 10);
            assert_eq!(report.unreadable, 1);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_root_symlink_root_and_file_root() {
        let dir = scratch("roots");
        assert!(run(&Options { root: dir.join("nope"), mode: apparent(), threads: 1 }, &|| false, &mut |_| {}).is_err());
        symlink(&dir, dir.join("link")).unwrap();
        let err = run(&Options { root: dir.join("link"), mode: apparent(), threads: 1 }, &|| false, &mut |_| {}).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        write(&dir.join("file"), 33);
        assert_eq!(count(&dir.join("file"), apparent(), 1).total, 33);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sparse_file_differs_by_mode() {
        let dir = scratch("sparse");
        let file = fs::File::create(dir.join("hole")).unwrap();
        file.set_len(1 << 30).unwrap();
        assert_eq!(count(&dir, apparent(), 1).total, 1 << 30);
        assert!(count(&dir, Mode::DISK, 1).total < 1 << 20);
        let _ = fs::remove_dir_all(&dir);
    }

    // About two thousand levels fit in PATH_MAX. A thread with a 256 KiB stack walks them: nothing here recurses.
    #[test]
    fn very_deep_tree_does_not_overflow_the_stack() {
        let dir = scratch("deep");
        let mut path = dir.clone();
        let mut levels = 0;
        while path.as_os_str().len() < 3900 && fs::create_dir(path.join("d")).is_ok() {
            path.push("d");
            levels += 1;
        }
        assert!(levels > 1500, "{levels}");
        let root = dir.clone();
        let report = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(move || count(&root, apparent(), 2))
            .unwrap()
            .join()
            .unwrap();
        assert!(!report.failed && !report.cancelled);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn depth_cap_is_reported_not_crashed() {
        let dir = scratch("cap");
        let sh_opts = Options { root: dir.clone(), mode: apparent(), threads: 1 };
        // Directly feed a Work item deeper than the cap.
        write(&dir.join("x/file"), 1);
        let sh = Shared {
            opts: &sh_opts,
            dev: fs::metadata(&dir).unwrap().dev(),
            queue: Mutex::new(Vec::new()),
            wake: Condvar::new(),
            pending: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            own: vec![AtomicU64::new(0), AtomicU64::new(0)],
            top: vec![AtomicU64::new(0)],
            total: AtomicU64::new(0),
            nodes: Mutex::new(Vec::new()),
            dirs: Mutex::new(HashSet::new()),
            links: Mutex::new(HashSet::new()),
            entries: AtomicU64::new(0),
            unreadable: AtomicUsize::new(0),
            too_deep: AtomicUsize::new(0),
            skipped: AtomicUsize::new(0),
            current: Mutex::new(dir.clone()),
        };
        process(&sh, &Work { node: 0, top: 0, dir: dir.clone(), depth: MAX_DEPTH }, &|| false);
        assert_eq!(sh.too_deep.load(Ordering::Relaxed), 1);
        assert!(sh.queue.lock().unwrap().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn wide_folder_cancels_quickly() {
        let dir = scratch("wide");
        for i in 0..60_000 {
            fs::File::create(dir.join(format!("f{i}"))).unwrap();
        }
        let sub = dir.join("sub");
        fs::create_dir_all(&sub).unwrap();
        for i in 0..60_000 {
            fs::File::create(sub.join(format!("f{i}"))).unwrap();
        }
        let start = Instant::now();
        let flag = AtomicBool::new(false);
        let opts = Options { root: dir.clone(), mode: apparent(), threads: 2 };
        let report = std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(5));
                flag.store(true, Ordering::Relaxed);
            });
            run(&opts, &|| flag.load(Ordering::Relaxed), &mut |_| {}).unwrap()
        });
        assert!(report.cancelled);
        assert!(start.elapsed() < Duration::from_millis(1500), "{:?}", start.elapsed());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn panic_in_a_worker_is_reported_not_propagated() {
        let dir = scratch("panic");
        write(&dir.join("fine/file"), 4);
        fs::create_dir_all(dir.join("trouble/__panic__")).unwrap();
        let report = count(&dir, apparent(), 4);
        assert!(report.failed);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn thread_count_does_not_change_the_answer() {
        let dir = scratch("threads");
        for i in 0..20 {
            for j in 0..5 {
                write(&dir.join(format!("d{i}/s{j}/f")), i * 10 + j + 1);
            }
        }
        let one = count(&dir, apparent(), 1);
        let eight = count(&dir, apparent(), 8);
        assert_eq!(one.total, eight.total);
        assert_eq!(one.bytes.iter().sum::<u64>(), eight.bytes.iter().sum::<u64>());
        assert_eq!(one.nodes.len(), eight.nodes.len());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn progress_grows_and_never_exceeds_the_end() {
        let dir = scratch("progress");
        for i in 0..30 {
            write(&dir.join(format!("d{i}/f")), 10);
        }
        let mut seen = Vec::new();
        let opts = Options { root: dir.clone(), mode: apparent(), threads: 2 };
        let report = run(&opts, &|| false, &mut |snap| seen.push((snap.total, snap.entries))).unwrap();
        assert!(!seen.is_empty());
        assert!(seen.windows(2).all(|pair| pair[0].0 <= pair[1].0 && pair[0].1 <= pair[1].1));
        assert!(seen.iter().all(|(total, _)| *total <= report.total));
        assert_eq!(report.total, 300);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn nodes_hold_the_total_of_everything_below() {
        let dir = scratch("nodes");
        write(&dir.join("a/b/c/d/e/file"), 1000);
        write(&dir.join("a/b/other"), 1);
        let report = count(&dir, apparent(), 3);
        let find = |p: &Path| report.nodes.iter().find(|(path, _)| path == p).unwrap().1;
        assert_eq!(find(&dir), 1001);
        assert_eq!(find(&dir.join("a")), 1001);
        assert_eq!(find(&dir.join("a/b")), 1001);
        assert_eq!(find(&dir.join("a/b/c")), 1000);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn other_mount_is_marked_and_not_entered() {
        // /proc is its own mount on every Linux. Only the top level is read here, so this stays fast.
        let opts = Options { root: PathBuf::from("/"), mode: Mode::DISK, threads: 1 };
        let scan = scan_root(&opts, &|| false).unwrap().unwrap();
        let proc_at = scan.children.iter().position(|c| c.name == "proc").unwrap();
        assert!(scan.children[proc_at].foreign);
        assert!(scan.work.iter().all(|w| w.dir != Path::new("/proc")));
        let all = Options { mode: Mode { apparent: false, same_device: false }, ..opts };
        let scan = scan_root(&all, &|| false).unwrap().unwrap();
        assert!(scan.work.iter().any(|w| w.dir == Path::new("/proc")));
    }

    fn open_fds() -> usize {
        fs::read_dir("/proc/self/fd").map_or(0, |dir| dir.count())
    }

    fn live_threads() -> usize {
        fs::read_dir("/proc/self/task").map_or(0, |dir| dir.count())
    }

    fn many_folders(dir: &Path, folders: usize, files: usize) {
        for i in 0..folders {
            let sub = dir.join(format!("d{}/e{}", i % 50, i));
            fs::create_dir_all(&sub).unwrap();
            for j in 0..files {
                fs::write(sub.join(format!("f{j}")), b"x").unwrap();
            }
        }
    }

    #[test]
    #[ignore = "stress: cargo test --release --lib -- --ignored --test-threads=1 usage_stress"]
    fn usage_stress_cancel_returns_quickly() {
        let dir = scratch("stress-cancel");
        many_folders(&dir, 3000, 40);
        let opts = Options { root: dir.clone(), mode: Mode::DISK, threads: 8 };
        let flag = AtomicBool::new(false);
        let full = Instant::now();
        run(&opts, &|| false, &mut |_| {}).unwrap();
        let full = full.elapsed();
        let mut late = Duration::ZERO;
        for _ in 0..5 {
            flag.store(false, Ordering::Relaxed);
            let raised = Mutex::new(None);
            let report = std::thread::scope(|scope| {
                scope.spawn(|| {
                    std::thread::sleep(full / 4);
                    *raised.lock().unwrap() = Some(Instant::now());
                    flag.store(true, Ordering::Relaxed);
                });
                run(&opts, &|| flag.load(Ordering::Relaxed), &mut |_| {}).unwrap()
            });
            let back = Instant::now();
            if report.cancelled {
                late = late.max(back.duration_since(raised.lock().unwrap().unwrap()));
            }
        }
        assert!(late < Duration::from_millis(250), "slowest stop {late:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore = "stress: cargo test --release --lib -- --ignored --test-threads=1 usage_stress"]
    fn usage_stress_repeated_runs_leak_nothing() {
        let dir = scratch("stress-leak");
        many_folders(&dir, 200, 5);
        let opts = Options { root: dir.clone(), mode: Mode::DISK, threads: 4 };
        let (fds, threads) = (open_fds(), live_threads());
        for round in 0..1000 {
            let flag = AtomicBool::new(round % 2 == 1);
            run(&opts, &|| flag.load(Ordering::Relaxed), &mut |_| {}).unwrap();
        }
        assert_eq!(open_fds(), fds);
        assert_eq!(live_threads(), threads);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore = "stress: cargo test --release --lib -- --ignored --test-threads=1 usage_stress"]
    fn usage_stress_changing_tree_never_fails() {
        let dir = scratch("stress-race");
        many_folders(&dir, 400, 10);
        let stop = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let mut n = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    let sub = dir.join(format!("d{}/e{}", n % 50, n % 400));
                    let _ = fs::remove_dir_all(&sub);
                    let _ = fs::create_dir_all(&sub);
                    let _ = fs::write(sub.join("new"), b"yy");
                    n += 1;
                }
            });
            for _ in 0..20 {
                let opts = Options { root: dir.clone(), mode: Mode::DISK, threads: 8 };
                let report = run(&opts, &|| false, &mut |_| {}).unwrap();
                assert!(!report.failed && !report.cancelled);
            }
            stop.store(true, Ordering::Relaxed);
        });
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore = "stress: cargo test --release --lib -- --ignored --test-threads=1 usage_stress"]
    fn usage_stress_half_the_folders_unreadable() {
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let dir = scratch("stress-denied");
        for i in 0..100 {
            write(&dir.join(format!("d{i}/file")), 7);
            if i % 2 == 1 {
                fs::set_permissions(dir.join(format!("d{i}")), fs::Permissions::from_mode(0)).unwrap();
            }
        }
        let report = count(&dir, apparent(), 8);
        for i in (1..100).step_by(2) {
            fs::set_permissions(dir.join(format!("d{i}")), fs::Permissions::from_mode(0o700)).unwrap();
        }
        assert_eq!(report.total, 50 * 7);
        assert_eq!(report.unreadable, 50);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn odd_names_do_not_panic() {
        use std::os::unix::ffi::OsStrExt;
        let dir = scratch("odd");
        let raw = std::ffi::OsStr::from_bytes(b"bad-\xff\xfe-name");
        write(&dir.join(raw), 3);
        write(&dir.join("line\nbreak/inner"), 4);
        write(&dir.join("x".repeat(255)), 5);
        let report = count(&dir, apparent(), 2);
        assert_eq!(report.total, 12);
        assert_eq!(report.unreadable, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn view_gives_each_row_its_share() {
        let children = [
            Child { name: "big".into(), is_dir: true, foreign: false },
            Child { name: "small".into(), is_dir: true, foreign: false },
            Child { name: "tiny.txt".into(), is_dir: false, foreign: false },
            Child { name: "mnt".into(), is_dir: true, foreign: true },
        ];
        let view = View::from_counts(Path::new("/x"), Mode::DISK, &children, &[750, 249, 1, 9999], false);
        assert_eq!(view.total(), 1000);
        let big = view.cell("big").unwrap();
        assert_eq!((big.bytes, big.text.as_str()), (750, "75%"));
        assert!((big.frac - 0.75).abs() < 1e-9);
        assert_eq!(view.cell("tiny.txt").unwrap().text, "<1%");
        let mount = view.cell("mnt").unwrap();
        assert!(mount.foreign && mount.text.is_empty());
        assert!(view.cell("missing").is_none());
        assert!(!view.partial && !view.stale);
    }

    #[test]
    fn empty_folder_view_never_divides_by_zero() {
        let children = [Child { name: "a".into(), is_dir: true, foreign: false }];
        let view = View::from_counts(Path::new("/x"), Mode::DISK, &children, &[0], false);
        let cell = view.cell("a").unwrap();
        assert_eq!((cell.frac, cell.text.as_str()), (0.0, "0%"));
        assert_eq!(percent_text(0, 0), "0%");
        assert_eq!(percent_text(1, 1_000_000), "<1%");
        assert_eq!(percent_text(1, 1), "100%");
    }

    #[test]
    fn prefilled_view_uses_the_cache_and_marks_the_rest_unknown() {
        let now = Instant::now();
        let mut cache = Cache::default();
        cache.store(&report_of(&[("/r", 5 * MIN_CACHED), ("/r/known", 3 * MIN_CACHED)]), Mode::DISK, false, now);
        let entries = [("known".to_string(), true, 0), ("other".to_string(), true, 0), ("file".to_string(), false, 10)];
        let view = View::prefilled(Path::new("/r"), Mode::DISK, &entries, &cache, now);
        assert!(view.partial && view.stale);
        assert_eq!(view.cell("known").unwrap().bytes, 3 * MIN_CACHED);
        assert!(view.cell("other").is_none());
        assert_eq!(view.cell("file").unwrap().bytes, 10);
    }

    #[test]
    fn watchdog_fires_only_when_the_count_stands_still() {
        let t0 = Instant::now();
        let limit = Duration::from_secs(10);
        let mut dog = Watchdog::new(t0);
        dog.feed(100, t0 + Duration::from_secs(4));
        assert!(!dog.stalled(t0 + Duration::from_secs(13), limit));
        dog.feed(100, t0 + Duration::from_secs(13));
        assert!(dog.stalled(t0 + Duration::from_secs(14), limit));
        dog.feed(101, t0 + Duration::from_secs(15));
        assert!(!dog.stalled(t0 + Duration::from_secs(16), limit));
        assert_eq!(dog.quiet_for(t0 + Duration::from_secs(16)), Duration::from_secs(1));
    }

    #[test]
    fn share_is_never_nan_or_over_one_hundred() {
        assert_eq!(percent(0, 0), 0.0);
        assert_eq!(percent(5, 0), 0.0);
        assert_eq!(percent(50, 100), 50.0);
        assert_eq!(percent(200, 100), 100.0);
        assert!(percent(u64::MAX, 1).is_finite());
    }

    #[test]
    fn network_filesystems_are_recognised() {
        for kind in ["cifs", "smb3", "nfs", "nfs4", "fuse.sshfs", "fuse.rclone", "fuse"] {
            assert!(is_network_fs(kind), "{kind}");
        }
        for kind in ["ext4", "btrfs", "tmpfs", "fuseblk", "xfs", "overlay"] {
            assert!(!is_network_fs(kind), "{kind}");
        }
        let text = "//srv/share /mnt/my\\040share cifs rw 0 0\n/dev/sda1 / ext4 rw 0 0\n";
        let (mount, _, kind, _) = crate::places::mount_entry(text, Path::new("/mnt/my share/docs")).unwrap();
        assert_eq!(mount, PathBuf::from("/mnt/my share"));
        assert_eq!(medium_from(&kind, None), Medium::Network);
    }

    #[test]
    fn threads_follow_the_medium() {
        assert_eq!(threads_for(Medium::Fast, 1), 2);
        assert_eq!(threads_for(Medium::Fast, 28), 8);
        assert_eq!(threads_for(Medium::Rotational, 28), 1);
        assert!(threads_for(Medium::Network, 28) <= 4);
        assert_eq!(medium_from("ext4", Some(true)), Medium::Rotational);
        assert_eq!(medium_from("ext4", Some(false)), Medium::Fast);
        assert_eq!(medium_from("ext4", None), Medium::Fast);
        assert_eq!(medium_from("cifs", Some(true)), Medium::Network);
    }

    fn report_of(nodes: &[(&str, u64)]) -> Report {
        Report {
            children: Arc::from(Vec::new()),
            bytes: Vec::new(),
            total: 0,
            entries: 0,
            unreadable: 0,
            too_deep: 0,
            skipped_mounts: 0,
            nodes: nodes.iter().map(|(p, n)| (PathBuf::from(p), *n)).collect(),
            cancelled: false,
            failed: false,
        }
    }

    #[test]
    fn cache_keeps_big_folders_and_expires() {
        let now = Instant::now();
        let mut cache = Cache::default();
        cache.store(&report_of(&[("/a", 10 * MIN_CACHED), ("/a/small", 5), ("/a/big", 3 * MIN_CACHED)]), Mode::DISK, false, now);
        assert_eq!(cache.len(), 2);
        assert!(cache.get(Path::new("/a/small"), Mode::DISK, now).is_none());
        let hit = cache.get(Path::new("/a/big"), Mode::DISK, now).unwrap();
        assert!(!hit.stale);
        assert!(cache.get(Path::new("/a/big"), Mode::DISK, now + TTL + Duration::from_secs(1)).unwrap().stale);
        // The same folder counted another way is another entry.
        assert!(cache.get(Path::new("/a/big"), apparent(), now).is_none());
    }

    #[test]
    fn network_results_live_longer() {
        let now = Instant::now();
        let mut cache = Cache::default();
        cache.store(&report_of(&[("/net", 2 * MIN_CACHED)]), Mode::DISK, true, now);
        let later = now + TTL + Duration::from_secs(1);
        assert!(!cache.get(Path::new("/net"), Mode::DISK, later).unwrap().stale);
        assert!(cache.get(Path::new("/net"), Mode::DISK, now + TTL_NETWORK + Duration::from_secs(1)).unwrap().stale);
    }

    #[test]
    fn cache_marks_parents_stale_and_forgets_below() {
        let now = Instant::now();
        let mut cache = Cache::default();
        let big = 4 * MIN_CACHED;
        cache.store(&report_of(&[("/r", big), ("/r/a", big), ("/r/a/x", big), ("/r/b", big)]), Mode::DISK, false, now);
        cache.touch(Path::new("/r/a/x/new.txt"));
        let stale = |p: &str| cache.get(Path::new(p), Mode::DISK, now).unwrap().stale;
        assert!(stale("/r") && stale("/r/a") && stale("/r/a/x"));
        assert!(!stale("/r/b"));
        cache.forget_below(Path::new("/r/a"));
        assert!(cache.get(Path::new("/r/a"), Mode::DISK, now).is_some());
        assert!(cache.get(Path::new("/r/a/x"), Mode::DISK, now).is_none());
        assert!(cache.get(Path::new("/r/b"), Mode::DISK, now).is_some());
    }

    #[test]
    fn cache_is_capped_and_drops_the_oldest() {
        let start = Instant::now();
        let mut cache = Cache::with_cap(10);
        for i in 0..40u64 {
            let name = format!("/p{i}");
            cache.store(&report_of(&[(name.as_str(), 2 * MIN_CACHED)]), Mode::DISK, false, start + Duration::from_secs(i));
        }
        assert!(cache.len() <= 10);
        assert!(cache.get(Path::new("/p39"), Mode::DISK, start).is_some());
        assert!(cache.get(Path::new("/p0"), Mode::DISK, start).is_none());
    }

    #[test]
    fn cancelled_or_failed_counts_are_not_cached() {
        let now = Instant::now();
        let mut cache = Cache::default();
        let mut report = report_of(&[("/x", 9 * MIN_CACHED)]);
        report.cancelled = true;
        cache.store(&report, Mode::DISK, false, now);
        assert!(cache.is_empty());
    }
}

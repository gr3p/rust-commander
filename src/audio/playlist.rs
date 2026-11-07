use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Repeat {
    /// Play to the end of the list, then stop.
    #[default]
    Off,
    /// Start again after the last track.
    All,
    /// Play the same track again when it ends.
    One,
}

impl Repeat {
    /// The next setting on a button that cycles through them.
    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::All => "all",
            Self::One => "one",
        }
    }

    /// Reads a saved name. Anything unknown means off.
    pub fn from_name(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "all" => Self::All,
            "one" => Self::One,
            _ => Self::Off,
        }
    }
}

/// The tracks of one folder and the order they play in.
pub struct Playlist {
    items: Vec<PathBuf>,
    // Indexes into `items`, in the order they play.
    order: Vec<usize>,
    pos: usize,
    shuffle: bool,
    repeat: Repeat,
    rng: u64,
}

impl Playlist {
    /// `current` is the track that plays first. It joins the list if it is not in it.
    pub fn new(mut items: Vec<PathBuf>, current: &Path, shuffle: bool, repeat: Repeat, seed: u64) -> Self {
        let at = match items.iter().position(|item| item == current) {
            Some(at) => at,
            None => {
                items.push(current.to_path_buf());
                items.len() - 1
            }
        };
        let mut list = Self { items, order: Vec::new(), pos: 0, shuffle, repeat, rng: seed | 1 };
        list.rebuild(at);
        list
    }

    pub fn items(&self) -> &[PathBuf] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn current(&self) -> Option<&Path> {
        self.order.get(self.pos).and_then(|&at| self.items.get(at)).map(PathBuf::as_path)
    }

    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    pub fn repeat(&self) -> Repeat {
        self.repeat
    }

    pub fn set_repeat(&mut self, repeat: Repeat) {
        self.repeat = repeat;
    }

    /// Turning shuffle on or off keeps the current track playing.
    pub fn set_shuffle(&mut self, on: bool) {
        if on == self.shuffle {
            return;
        }
        self.shuffle = on;
        let at = self.order.get(self.pos).copied().unwrap_or(0);
        self.rebuild(at);
    }

    /// Adds songs to the end of the list. Ones already in the list are left out. Returns how many were added.
    /// With shuffle on, they are mixed in with what has not played yet.
    pub fn append(&mut self, paths: Vec<PathBuf>) -> usize {
        let before = self.items.len();
        for path in paths {
            if !self.items.contains(&path) {
                self.items.push(path);
            }
        }
        let added = self.items.len() - before;
        self.order.extend(before..self.items.len());
        if self.shuffle && added > 0 {
            let from = (self.pos + 1).min(self.order.len());
            let mut tail = self.order.split_off(from);
            self.shuffle_slice(&mut tail);
            self.order.extend(tail);
        }
        added
    }

    /// The listener moved to `path`. False if it is not in the list.
    pub fn set_current(&mut self, path: &Path) -> bool {
        let Some(at) = self.items.iter().position(|item| item == path) else { return false };
        match self.order.iter().position(|&slot| slot == at) {
            Some(pos) => self.pos = pos,
            None => self.rebuild(at),
        }
        true
    }

    /// The track after this one. `auto` is a track that ended by itself, which repeat-one replays.
    /// None means the list is over.
    pub fn next(&mut self, auto: bool) -> Option<&Path> {
        if self.items.is_empty() {
            return None;
        }
        if auto && self.repeat == Repeat::One {
            return self.current();
        }
        if self.pos + 1 < self.order.len() {
            self.pos += 1;
        } else if self.repeat == Repeat::Off {
            return None;
        } else {
            self.wrap();
        }
        self.current()
    }

    /// The track before this one. None at the start of a list that does not repeat.
    pub fn previous(&mut self) -> Option<&Path> {
        if self.items.is_empty() {
            return None;
        }
        if self.pos > 0 {
            self.pos -= 1;
        } else if self.repeat == Repeat::Off {
            return None;
        } else {
            self.pos = self.order.len() - 1;
        }
        self.current()
    }

    /// The end of the list on a repeating playlist: a shuffled one gets a fresh order.
    fn wrap(&mut self) {
        if self.shuffle {
            let last = self.order.last().copied();
            let mut fresh: Vec<usize> = (0..self.items.len()).collect();
            self.shuffle_slice(&mut fresh);
            // The same track twice in a row would look like a stuck player.
            if fresh.len() > 1 && fresh.first().copied() == last {
                fresh.swap(0, 1);
            }
            self.order = fresh;
        }
        self.pos = 0;
    }

    /// Lays out the play order with `first` as the current track.
    fn rebuild(&mut self, first: usize) {
        if self.shuffle {
            let mut rest: Vec<usize> = (0..self.items.len()).filter(|&at| at != first).collect();
            self.shuffle_slice(&mut rest);
            self.order = std::iter::once(first).chain(rest).collect();
            self.pos = 0;
        } else {
            self.order = (0..self.items.len()).collect();
            self.pos = first;
        }
    }

    fn shuffle_slice(&mut self, slice: &mut [usize]) {
        for i in (1..slice.len()).rev() {
            let j = (self.random() % (i as u64 + 1)) as usize;
            slice.swap(i, j);
        }
    }

    // xorshift64*. Good enough to mix a folder, and it needs no crate.
    fn random(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn songs(n: usize) -> Vec<PathBuf> {
        (1..=n).map(|i| PathBuf::from(format!("/music/{i:02}.mp3"))).collect()
    }

    fn name(path: Option<&Path>) -> String {
        path.map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).unwrap_or_else(|| "-".into())
    }

    fn list(n: usize, at: usize, shuffle: bool, repeat: Repeat, seed: u64) -> Playlist {
        let items = songs(n);
        let current = items[at].clone();
        Playlist::new(items, &current, shuffle, repeat, seed)
    }

    #[test]
    fn plays_in_order_and_stops_at_the_end() {
        let mut p = list(3, 0, false, Repeat::Off, 1);
        assert_eq!(name(p.current()), "01.mp3");
        assert_eq!(name(p.next(true)), "02.mp3");
        assert_eq!(name(p.next(true)), "03.mp3");
        assert_eq!(name(p.next(true)), "-");
        // Staying at the end, so Previous still works from there.
        assert_eq!(name(p.current()), "03.mp3");
        assert_eq!(name(p.previous()), "02.mp3");
        assert_eq!(name(p.previous()), "01.mp3");
        assert_eq!(name(p.previous()), "-");
        assert_eq!(name(p.current()), "01.mp3");
    }

    #[test]
    fn starts_in_the_middle_of_the_folder() {
        let mut p = list(5, 2, false, Repeat::Off, 1);
        assert_eq!(name(p.current()), "03.mp3");
        assert_eq!(name(p.next(false)), "04.mp3");
        assert_eq!(name(p.previous()), "03.mp3");
        assert_eq!(name(p.previous()), "02.mp3");
    }

    #[test]
    fn repeat_all_wraps_in_both_directions() {
        let mut p = list(3, 2, false, Repeat::All, 1);
        assert_eq!(name(p.next(true)), "01.mp3");
        assert_eq!(name(p.previous()), "03.mp3");
        assert_eq!(name(p.next(false)), "01.mp3");
    }

    #[test]
    fn repeat_one_replays_when_a_track_ends_but_next_still_moves_on() {
        let mut p = list(3, 1, false, Repeat::One, 1);
        assert_eq!(name(p.next(true)), "02.mp3");
        assert_eq!(name(p.next(true)), "02.mp3");
        assert_eq!(name(p.next(false)), "03.mp3");
        // At the end of the list a manual Next wraps, since the list repeats.
        assert_eq!(name(p.next(false)), "01.mp3");
    }

    #[test]
    fn a_single_track_repeats_or_stops() {
        let mut off = list(1, 0, false, Repeat::Off, 1);
        assert_eq!(name(off.next(true)), "-");
        assert_eq!(name(off.previous()), "-");
        let mut all = list(1, 0, false, Repeat::All, 1);
        assert_eq!(name(all.next(true)), "01.mp3");
        assert_eq!(name(all.previous()), "01.mp3");
        let mut shuffled = list(1, 0, true, Repeat::All, 3);
        assert_eq!(name(shuffled.next(true)), "01.mp3");
    }

    #[test]
    fn a_track_that_is_not_listed_joins_the_list() {
        let p = Playlist::new(songs(2), Path::new("/music/other.flac"), false, Repeat::Off, 1);
        assert_eq!(p.len(), 3);
        assert_eq!(name(p.current()), "other.flac");
    }

    #[test]
    fn shuffle_plays_every_track_once_then_stops() {
        for seed in [1u64, 2, 3, 42, 9999] {
            let mut p = list(12, 5, true, Repeat::Off, seed);
            // The chosen track still plays first.
            assert_eq!(name(p.current()), "06.mp3");
            let mut seen = vec![name(p.current())];
            while let Some(next) = p.next(true) {
                seen.push(name(Some(next)));
                assert!(seen.len() <= 12, "never ends");
            }
            let mut sorted = seen.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(sorted.len(), 12, "seed {seed}: {seen:?}");
            assert_eq!(seen.len(), 12);
        }
    }

    #[test]
    fn shuffle_really_shuffles_and_differs_by_seed() {
        let order = |seed| {
            let mut p = list(20, 0, true, Repeat::Off, seed);
            let mut out = vec![name(p.current())];
            while let Some(n) = p.next(true) {
                out.push(name(Some(n)));
            }
            out
        };
        let (a, b) = (order(1), order(2));
        let plain: Vec<String> = (1..=20).map(|i| format!("{i:02}.mp3")).collect();
        assert_ne!(a, plain);
        assert_ne!(a, b);
        assert_eq!(order(1), a, "the same seed gives the same order");
    }

    #[test]
    fn shuffle_with_repeat_all_starts_a_new_round_without_a_double() {
        let mut p = list(6, 0, true, Repeat::All, 7);
        let mut last = name(p.current());
        let mut rounds = 0;
        for step in 0..40 {
            let now = name(p.next(true));
            assert_ne!(now, last, "step {step}: the same track twice in a row");
            if step % 6 == 5 {
                rounds += 1;
            }
            last = now;
        }
        assert!(rounds >= 6);
    }

    #[test]
    fn previous_walks_back_through_what_shuffle_played() {
        let mut p = list(8, 3, true, Repeat::Off, 11);
        let mut played = vec![name(p.current())];
        for _ in 0..4 {
            played.push(name(p.next(false)));
        }
        for expect in played.iter().rev().skip(1) {
            assert_eq!(&name(p.previous()), expect);
        }
        assert_eq!(name(p.previous()), "-");
    }

    #[test]
    fn moving_to_a_track_by_hand_continues_from_there() {
        let mut p = list(5, 0, false, Repeat::Off, 1);
        assert!(p.set_current(Path::new("/music/04.mp3")));
        assert_eq!(name(p.next(true)), "05.mp3");
        assert!(!p.set_current(Path::new("/music/zzz.mp3")));
        assert_eq!(name(p.current()), "05.mp3");

        let mut s = list(5, 0, true, Repeat::Off, 5);
        assert!(s.set_current(Path::new("/music/03.mp3")));
        assert_eq!(name(s.current()), "03.mp3");
        assert!(s.next(true).is_some() || s.len() == 1);
    }

    #[test]
    fn added_songs_play_after_the_ones_in_the_list() {
        let mut p = list(2, 0, false, Repeat::Off, 1);
        let more = vec![PathBuf::from("/other/a.mp3"), PathBuf::from("/music/02.mp3"), PathBuf::from("/other/b.mp3"), PathBuf::from("/other/a.mp3")];
        // 02.mp3 is already there and a.mp3 is given twice.
        assert_eq!(p.append(more), 2);
        assert_eq!(p.len(), 4);
        assert_eq!(name(p.current()), "01.mp3");
        let played: Vec<String> = std::iter::from_fn(|| p.next(true).map(|song| name(Some(song)))).collect();
        assert_eq!(played, ["02.mp3", "a.mp3", "b.mp3"]);
        assert_eq!(p.append(vec![]), 0);
    }

    #[test]
    fn songs_added_after_the_end_carry_on_from_it() {
        let mut p = list(2, 1, false, Repeat::Off, 1);
        assert_eq!(name(p.next(true)), "-");
        assert_eq!(p.append(vec![PathBuf::from("/other/c.mp3")]), 1);
        assert_eq!(name(p.current()), "02.mp3");
        assert_eq!(name(p.next(false)), "c.mp3");
    }

    #[test]
    fn added_songs_are_shuffled_in_without_touching_what_has_played() {
        let mut p = list(6, 2, true, Repeat::Off, 7);
        let head = [name(p.current()), name(p.next(false)), name(p.next(false))];
        let extra: Vec<PathBuf> = (1..=5).map(|i| PathBuf::from(format!("/other/x{i}.mp3"))).collect();
        assert_eq!(p.append(extra), 5);
        let mut rest: Vec<String> = std::iter::from_fn(|| p.next(true).map(|song| name(Some(song)))).collect();
        // Everything plays exactly once: the 6 songs and the 5 new ones, and the first three stay where they were.
        let mut all: Vec<String> = head.to_vec();
        all.append(&mut rest);
        assert_eq!(all.len(), 11);
        let mut sorted = all.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 11);
        assert_eq!(&all[..3], &head);
    }

    #[test]
    fn switching_shuffle_keeps_the_current_track() {
        let mut p = list(6, 3, false, Repeat::Off, 1);
        p.set_shuffle(true);
        assert!(p.shuffle());
        assert_eq!(name(p.current()), "04.mp3");
        p.set_shuffle(false);
        assert_eq!(name(p.current()), "04.mp3");
        assert_eq!(name(p.next(true)), "05.mp3");
        p.set_shuffle(false);
        assert_eq!(name(p.current()), "05.mp3");
    }

    #[test]
    fn repeat_cycles_and_round_trips_through_its_name() {
        let mut r = Repeat::Off;
        let mut seen = Vec::new();
        for _ in 0..4 {
            seen.push(r);
            r = r.cycle();
        }
        assert_eq!(seen, vec![Repeat::Off, Repeat::All, Repeat::One, Repeat::Off]);
        for r in [Repeat::Off, Repeat::All, Repeat::One] {
            assert_eq!(Repeat::from_name(r.name()), r);
        }
        assert_eq!(Repeat::from_name(" ALL "), Repeat::All);
        assert_eq!(Repeat::from_name("nonsense"), Repeat::Off);
        assert_eq!(Repeat::from_name(""), Repeat::Off);
    }

    #[test]
    fn repeat_can_change_while_playing() {
        let mut p = list(2, 1, false, Repeat::Off, 1);
        assert_eq!(name(p.next(true)), "-");
        p.set_repeat(Repeat::All);
        assert_eq!(p.repeat(), Repeat::All);
        assert_eq!(name(p.next(true)), "01.mp3");
        p.set_repeat(Repeat::One);
        assert_eq!(name(p.next(true)), "01.mp3");
    }
}

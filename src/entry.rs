pub const DIR: u8 = 1;
pub const LINK: u8 = 2;
pub const HIDDEN: u8 = 4;

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: Box<str>,
    pub size: u64,
    pub mtime: i64,
    pub flags: u8,
}

impl Entry {
    pub fn dotdot() -> Self {
        Self {
            name: "..".into(),
            size: 0,
            mtime: 0,
            flags: DIR,
        }
    }

    pub fn is_dir(&self) -> bool {
        self.flags & DIR != 0
    }

    pub fn is_symlink(&self) -> bool {
        self.flags & LINK != 0
    }

    pub fn is_hidden(&self) -> bool {
        self.flags & HIDDEN != 0
    }
}

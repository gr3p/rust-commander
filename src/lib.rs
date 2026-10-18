pub fn version() -> &'static str {
    env!("RC_VERSION")
}

pub fn app_title() -> &'static str {
    env!("RC_TITLE")
}

pub mod archive;
pub mod encode;
pub mod sizes;
pub mod commands;
pub mod config;
pub mod log;
pub mod entry;
pub mod hotlist;
pub mod external;
pub mod icons;
pub mod imageview;
pub mod listing;
pub mod lister;
pub mod mount;
pub mod ops;
pub mod pack;
pub mod places;
pub mod props;
pub mod sandbox;
pub mod search;
pub mod sfv;
pub mod sum;
pub mod state;
pub mod syntax;
pub mod theme;

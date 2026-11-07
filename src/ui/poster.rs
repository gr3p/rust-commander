use relm4::gtk;
use relm4::gtk::glib;
use relm4::gtk::prelude::*;
use relm4::gtk::subclass::prelude::*;

/// The cover is never bigger than this share of the window's shorter side, nor than `LARGEST` pixels.
const SHARE: f64 = 0.38;
const LARGEST: f64 = 420.0;
const SMALLEST: f64 = 64.0;

/// The edge of the square a cover gets: as much room as there is, within a limit set by the window.
pub fn edge(room_w: i32, room_h: i32, window_w: i32, window_h: i32) -> i32 {
    let limit = (f64::from(window_w.min(window_h)) * SHARE).clamp(SMALLEST, LARGEST);
    // A window too small for the smallest cover still shows what fits.
    f64::from(room_w).min(f64::from(room_h)).min(limit).max(0.0) as i32
}

mod imp {
    use std::cell::RefCell;

    use super::*;

    #[derive(Default)]
    pub struct Poster {
        pub child: RefCell<Option<gtk::Widget>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Poster {
        const NAME: &'static str = "RustCommanderPoster";
        type Type = super::Poster;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Poster {
        fn dispose(&self) {
            if let Some(child) = self.child.take() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for Poster {
        // It asks for nothing, so the player's other rows keep their room and the cover takes what is left.
        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            (0, 0, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let Some(child) = self.child.borrow().clone() else { return };
            let (window_w, window_h) = self
                .obj()
                .root()
                .map_or((width, height), |root| (root.upcast_ref::<gtk::Widget>().width(), root.upcast_ref::<gtk::Widget>().height()));
            let side = edge(width, height, window_w, window_h);
            let (x, y) = ((width - side) / 2, (height - side) / 2);
            child.size_allocate(&gtk::Allocation::new(x, y, side, side), -1);
        }
    }
}

glib::wrapper! {
    /// Holds one child as a centered square that stays within a size set by the window.
    pub struct Poster(ObjectSubclass<imp::Poster>) @extends gtk::Widget, @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Poster {
    pub fn new(child: &impl IsA<gtk::Widget>) -> Self {
        let poster: Self = glib::Object::new();
        child.set_parent(&poster);
        poster.imp().child.replace(Some(child.clone().upcast()));
        poster.set_hexpand(true);
        poster.set_vexpand(true);
        poster
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cover_fills_the_room_up_to_a_limit_set_by_the_window() {
        // A big window and plenty of room: the share of the window's shorter side rules.
        assert_eq!(edge(900, 900, 1920, 1080), (1080.0 * SHARE) as i32);
        // The limit never passes the largest cover.
        assert_eq!(edge(2000, 2000, 5000, 5000), LARGEST as i32);
        // Little room: the room rules.
        assert_eq!(edge(300, 120, 1920, 1080), 120);
        assert_eq!(edge(80, 500, 1920, 1080), 80);
    }

    #[test]
    fn a_small_window_still_gets_a_cover_and_nothing_goes_negative() {
        assert_eq!(edge(500, 500, 100, 100), SMALLEST as i32);
        assert_eq!(edge(0, 0, 1200, 760), 0);
        assert_eq!(edge(-5, 40, 1200, 760), 0);
    }

    #[test]
    fn the_limit_grows_with_the_window() {
        assert!(edge(2000, 2000, 1200, 760) < edge(2000, 2000, 1920, 1080));
    }
}

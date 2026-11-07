use std::cell::Cell;
use std::f64::consts::{FRAC_PI_2, PI, TAU};
use std::rc::Rc;

use relm4::gtk;
use relm4::gtk::cairo::{Context, LineCap, LineJoin};
use relm4::gtk::prelude::*;

/// Icons for the player, drawn with cairo in the widget's own text color. A theme, a hover or an
/// "on" state then recolors them through CSS, and no icon font has to exist on the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Previous,
    Play,
    Pause,
    Next,
    Shuffle,
    Repeat,
    RepeatOne,
    Gear,
    /// A chevron down: fold the player into its small bar.
    Minimize,
    /// A chevron up: unfold the small bar into the full player.
    Expand,
    Close,
    /// Equalizer bars: the visualizer.
    Bars,
    Speaker,
    SpeakerMuted,
}

fn icon_area(icon: Icon, side: i32) -> (gtk::DrawingArea, Rc<Cell<Icon>>) {
    let state = Rc::new(Cell::new(icon));
    let area = gtk::DrawingArea::new();
    area.set_content_width(side);
    area.set_content_height(side);
    area.set_can_target(false);
    let shown = Rc::clone(&state);
    area.set_draw_func(move |area, cr, w, h| {
        let color = area.color();
        cr.set_source_rgba(f64::from(color.red()), f64::from(color.green()), f64::from(color.blue()), f64::from(color.alpha()));
        draw(cr, shown.get(), f64::from(w), f64::from(h));
    });
    (area, state)
}

fn set_class(widget: &impl IsA<gtk::Widget>, class: &str, on: bool) -> bool {
    if widget.has_css_class(class) == on {
        return false;
    }
    if on {
        widget.add_css_class(class);
    } else {
        widget.remove_css_class(class);
    }
    true
}

/// A flat button holding one icon.
pub struct IconButton {
    pub button: gtk::Button,
    area: gtk::DrawingArea,
    icon: Rc<Cell<Icon>>,
}

impl IconButton {
    /// `side` is the icon's size in pixels. `class` is the CSS class that styles the button.
    pub fn new(icon: Icon, side: i32, class: &str) -> Self {
        let (area, icon) = icon_area(icon, side);
        let button = gtk::Button::new();
        button.set_child(Some(&area));
        button.add_css_class("media-icon");
        button.add_css_class(class);
        // The list keeps the keyboard, so Space and the arrows still reach the player after a click.
        button.set_focusable(false);
        button.set_focus_on_click(false);
        Self { button, area, icon }
    }

    pub fn set_icon(&self, icon: Icon) {
        if self.icon.get() != icon {
            self.icon.set(icon);
            self.area.queue_draw();
        }
    }

    /// A lit state, in the accent color.
    pub fn set_on(&self, on: bool) {
        if set_class(&self.button, "on", on) {
            self.area.queue_draw();
        }
    }

    pub fn set_tooltip(&self, text: &str) {
        self.button.set_tooltip_text(Some(text));
    }
}

/// An icon that opens a popover.
pub struct IconMenu {
    pub button: gtk::MenuButton,
    area: gtk::DrawingArea,
}

impl IconMenu {
    pub fn new(icon: Icon, side: i32, class: &str, popover: &gtk::Popover) -> Self {
        let (area, _) = icon_area(icon, side);
        let button = gtk::MenuButton::new();
        button.set_child(Some(&area));
        button.set_popover(Some(popover));
        button.set_always_show_arrow(false);
        button.add_css_class("media-icon");
        button.add_css_class(class);
        button.set_focusable(false);
        button.set_focus_on_click(false);
        Self { button, area }
    }

    pub fn set_tooltip(&self, text: &str) {
        self.button.set_tooltip_text(Some(text));
        self.area.queue_draw();
    }
}

fn style(cr: &Context, width: f64) {
    cr.set_line_width(width);
    cr.set_line_cap(LineCap::Round);
    cr.set_line_join(LineJoin::Round);
}

fn draw(cr: &Context, icon: Icon, w: f64, h: f64) {
    // A unit square in the middle, with a little room around it.
    let side = w.min(h);
    let pad = side * 0.12;
    let u = side - 2.0 * pad;
    cr.save().ok();
    cr.translate((w - u) / 2.0, (h - u) / 2.0);
    cr.scale(u, u);
    let stroke = 0.11;
    match icon {
        Icon::Play => triangle(cr, 0.2, 0.08, 0.2, 0.92, 0.92, 0.5),
        Icon::Pause => {
            rounded(cr, 0.14, 0.1, 0.27, 0.8, 0.06);
            rounded(cr, 0.59, 0.1, 0.27, 0.8, 0.06);
        }
        Icon::Previous => {
            rounded(cr, 0.06, 0.12, 0.14, 0.76, 0.04);
            triangle(cr, 0.92, 0.12, 0.92, 0.88, 0.3, 0.5);
        }
        Icon::Next => {
            rounded(cr, 0.8, 0.12, 0.14, 0.76, 0.04);
            triangle(cr, 0.08, 0.12, 0.08, 0.88, 0.7, 0.5);
        }
        Icon::Shuffle => shuffle(cr, stroke),
        Icon::Repeat => repeat(cr, stroke, false),
        Icon::RepeatOne => repeat(cr, stroke, true),
        Icon::Gear => gear(cr),
        Icon::Minimize => chevron(cr, stroke * 1.3, true),
        Icon::Expand => chevron(cr, stroke * 1.3, false),
        Icon::Close => {
            style(cr, stroke * 1.3);
            cr.move_to(0.2, 0.2);
            cr.line_to(0.8, 0.8);
            cr.move_to(0.8, 0.2);
            cr.line_to(0.2, 0.8);
            cr.stroke().ok();
        }
        Icon::Bars => {
            for (i, top) in [0.45, 0.15, 0.3, 0.05, 0.35].into_iter().enumerate() {
                rounded(cr, 0.06 + i as f64 * 0.19, top + 0.05, 0.13, 0.9 - top - 0.05, 0.04);
            }
        }
        Icon::Speaker => speaker(cr, stroke, false),
        Icon::SpeakerMuted => speaker(cr, stroke, true),
    }
    cr.restore().ok();
}

fn triangle(cr: &Context, x1: f64, y1: f64, x2: f64, y2: f64, x3: f64, y3: f64) {
    cr.set_line_join(LineJoin::Round);
    cr.set_line_width(0.08);
    cr.move_to(x1, y1);
    cr.line_to(x2, y2);
    cr.line_to(x3, y3);
    cr.close_path();
    // Fill and a thin stroke in the same color round the corners.
    cr.fill_preserve().ok();
    cr.stroke().ok();
}

fn rounded(cr: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, FRAC_PI_2, PI);
    cr.arc(x + r, y + r, r, PI, 3.0 * FRAC_PI_2);
    cr.close_path();
    cr.fill().ok();
}

fn chevron(cr: &Context, width: f64, down: bool) {
    style(cr, width);
    let (ends, tip) = if down { (0.36, 0.66) } else { (0.64, 0.34) };
    cr.new_path();
    cr.move_to(0.2, ends);
    cr.line_to(0.5, tip);
    cr.line_to(0.8, ends);
    cr.stroke().ok();
}

fn arrow_head(cr: &Context, x: f64, y: f64, dir: f64, size: f64) {
    // `dir` is the angle the arrow points along.
    let (a, b) = (dir + 2.5, dir - 2.5);
    cr.move_to(x, y);
    cr.line_to(x + size * a.cos(), y + size * a.sin());
    cr.line_to(x + size * b.cos(), y + size * b.sin());
    cr.close_path();
    cr.fill().ok();
}

fn shuffle(cr: &Context, stroke: f64) {
    style(cr, stroke);
    // One line runs from bottom left to top right, the other from top left to bottom right.
    cr.move_to(0.06, 0.72);
    cr.curve_to(0.4, 0.72, 0.5, 0.28, 0.76, 0.28);
    cr.stroke().ok();
    cr.move_to(0.06, 0.28);
    cr.curve_to(0.4, 0.28, 0.5, 0.72, 0.76, 0.72);
    cr.stroke().ok();
    arrow_head(cr, 0.96, 0.28, 0.0, 0.26);
    arrow_head(cr, 0.96, 0.72, 0.0, 0.26);
}

fn repeat(cr: &Context, stroke: f64, one: bool) {
    style(cr, stroke);
    // A loop: out along the top to the right, back along the bottom to the left.
    cr.new_sub_path();
    cr.move_to(0.18, 0.5);
    cr.line_to(0.18, 0.4);
    cr.curve_to(0.18, 0.28, 0.24, 0.25, 0.36, 0.25);
    cr.line_to(0.74, 0.25);
    cr.stroke().ok();
    cr.move_to(0.82, 0.5);
    cr.line_to(0.82, 0.6);
    cr.curve_to(0.82, 0.72, 0.76, 0.75, 0.64, 0.75);
    cr.line_to(0.26, 0.75);
    cr.stroke().ok();
    arrow_head(cr, 0.96, 0.25, 0.0, 0.24);
    arrow_head(cr, 0.04, 0.75, PI, 0.24);
    if one {
        // A small 1 in the middle.
        cr.set_line_width(stroke * 0.85);
        cr.move_to(0.46, 0.43);
        cr.line_to(0.52, 0.38);
        cr.line_to(0.52, 0.62);
        cr.stroke().ok();
    }
}

fn gear(cr: &Context) {
    let (cx, cy) = (0.5, 0.5);
    let teeth = 8;
    let (outer, inner) = (0.47, 0.36);
    cr.new_path();
    for i in 0..teeth {
        let base = TAU * f64::from(i) / f64::from(teeth);
        let half = TAU / f64::from(teeth) / 4.0;
        for (angle, radius) in [(base - half, inner), (base - half * 0.6, outer), (base + half * 0.6, outer), (base + half, inner)] {
            let (x, y) = (cx + radius * angle.cos(), cy + radius * angle.sin());
            if i == 0 && angle == base - half {
                cr.move_to(x, y);
            } else {
                cr.line_to(x, y);
            }
        }
    }
    cr.close_path();
    // A hole in the middle, cut with the even-odd rule.
    cr.new_sub_path();
    cr.arc(cx, cy, 0.17, 0.0, TAU);
    cr.set_fill_rule(relm4::gtk::cairo::FillRule::EvenOdd);
    cr.fill().ok();
    cr.set_fill_rule(relm4::gtk::cairo::FillRule::Winding);
}

fn speaker(cr: &Context, stroke: f64, muted: bool) {
    // The cone.
    cr.move_to(0.08, 0.38);
    cr.line_to(0.28, 0.38);
    cr.line_to(0.5, 0.18);
    cr.line_to(0.5, 0.82);
    cr.line_to(0.28, 0.62);
    cr.line_to(0.08, 0.62);
    cr.close_path();
    cr.fill().ok();
    style(cr, stroke);
    if muted {
        cr.move_to(0.66, 0.36);
        cr.line_to(0.94, 0.64);
        cr.move_to(0.94, 0.36);
        cr.line_to(0.66, 0.64);
        cr.stroke().ok();
    } else {
        cr.new_sub_path();
        cr.arc(0.5, 0.5, 0.24, -0.9, 0.9);
        cr.stroke().ok();
        cr.new_sub_path();
        cr.arc(0.5, 0.5, 0.42, -0.9, 0.9);
        cr.stroke().ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use relm4::gtk::cairo::{Format, ImageSurface};

    const ALL: [Icon; 14] = [
        Icon::Previous,
        Icon::Play,
        Icon::Pause,
        Icon::Next,
        Icon::Shuffle,
        Icon::Repeat,
        Icon::RepeatOne,
        Icon::Gear,
        Icon::Minimize,
        Icon::Expand,
        Icon::Close,
        Icon::Bars,
        Icon::Speaker,
        Icon::SpeakerMuted,
    ];

    fn painted(icon: Icon, w: i32, h: i32) -> usize {
        let mut surface = ImageSurface::create(Format::ARgb32, w, h).unwrap();
        {
            let cr = Context::new(&surface).unwrap();
            cr.set_source_rgba(1.0, 1.0, 1.0, 1.0);
            draw(&cr, icon, f64::from(w), f64::from(h));
        }
        let data = surface.data().unwrap();
        data.chunks_exact(4).filter(|px| px[3] > 0).count()
    }

    #[test]
    fn every_icon_paints_something_and_stays_inside_its_box() {
        for icon in ALL {
            for (w, h) in [(16, 16), (24, 24), (40, 40), (32, 20), (20, 32)] {
                let count = painted(icon, w, h);
                assert!(count > (w * h) as usize / 30, "{icon:?} at {w}x{h} painted only {count} pixels");
                assert!(count < (w * h) as usize, "{icon:?} at {w}x{h} filled the whole box");
            }
        }
    }

    #[test]
    fn the_icons_differ_from_each_other() {
        let mut shapes = Vec::new();
        for icon in ALL {
            let mut surface = ImageSurface::create(Format::ARgb32, 24, 24).unwrap();
            {
                let cr = Context::new(&surface).unwrap();
                cr.set_source_rgba(1.0, 1.0, 1.0, 1.0);
                draw(&cr, icon, 24.0, 24.0);
            }
            shapes.push((icon, surface.data().unwrap().to_vec()));
        }
        for (i, (a, shape_a)) in shapes.iter().enumerate() {
            for (b, shape_b) in &shapes[i + 1..] {
                assert_ne!(shape_a, shape_b, "{a:?} and {b:?} look the same");
            }
        }
    }

    #[test]
    fn zero_sized_boxes_do_not_panic() {
        for icon in ALL {
            let surface = ImageSurface::create(Format::ARgb32, 1, 1).unwrap();
            let cr = Context::new(&surface).unwrap();
            draw(&cr, icon, 0.0, 0.0);
            draw(&cr, icon, 1.0, 1.0);
        }
    }
}

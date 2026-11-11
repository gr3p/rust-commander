use std::cell::RefCell;
use std::f64::consts::{FRAC_PI_2, PI, TAU};
use std::rc::Rc;

use relm4::gtk;
use relm4::gtk::cairo::{Context, LinearGradient};
use relm4::gtk::prelude::*;

use rust_commander::audio::{Analyzer, Scope, VisualStyle, WINDOW};

type Rgb = (f64, f64, f64);

/// The theme colors the pictures are drawn in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colors {
    pub accent: Rgb,
    pub bright: Rgb,
}

impl Colors {
    pub fn from_theme() -> Self {
        let palette = rust_commander::theme::current();
        let parse = |hex: &str, fallback: Rgb| {
            gtk::gdk::RGBA::parse(hex).map_or(fallback, |c| (f64::from(c.red()), f64::from(c.green()), f64::from(c.blue())))
        };
        Self { accent: parse(&palette.accent, (0.48, 0.64, 0.97)), bright: parse(&palette.bright, (0.75, 0.79, 0.96)) }
    }
}

/// One frame to draw.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub levels: Vec<f32>,
    pub peaks: Vec<f32>,
    pub wave: Vec<f32>,
    pub bass: f32,
}

fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, a.2 + (b.2 - a.2) * t)
}

fn rgba(cr: &Context, c: Rgb, alpha: f64, density: f64) {
    cr.set_source_rgba(c.0, c.1, c.2, (alpha * density).clamp(0.0, 1.0));
}

/// 50 is the picture as it was drawn before the knobs. 0 is still, 100 is twice as strong.
fn gain(strength: u8) -> f32 {
    f32::from(strength.min(100)) / 50.0
}

/// 50 is the built-in fall. 0 snaps down, 100 barely moves.
fn fall_scale(smooth: u8) -> f32 {
    let t = f32::from(smooth.min(100)) / 100.0;
    1.8 - 1.6 * t
}

/// 50 is the original ink. 0 disappears, 100 is twice as visible.
fn ink(density: u8) -> f64 {
    f64::from(density.min(100)) / 50.0
}

/// Draws one style over a `w` by `h` area. `density` 1 is the original softness, so text stays readable.
fn draw_ink(cr: &Context, style: VisualStyle, w: f64, h: f64, scene: &Scene, colors: &Colors, density: f64) {
    if w < 4.0 || h < 4.0 {
        return;
    }
    cr.save().ok();
    match style {
        VisualStyle::Bars => bars(cr, w, h, scene, colors, density),
        VisualStyle::Wave => wave(cr, w, h, scene, colors, density),
        VisualStyle::Radial => radial(cr, w, h, scene, colors, density),
        VisualStyle::Ribbon => ribbon(cr, w, h, scene, colors, density),
    }
    cr.restore().ok();
}

fn bars(cr: &Context, w: f64, h: f64, scene: &Scene, colors: &Colors, density: f64) {
    let n = scene.levels.len();
    if n == 0 {
        return;
    }
    let gap = (w / n as f64 * 0.22).clamp(2.0, 6.0);
    let bar = ((w - gap * (n as f64 + 1.0)) / n as f64).max(1.0);
    let top_limit = h * 0.92;
    for (i, level) in scene.levels.iter().enumerate() {
        let x = gap + i as f64 * (bar + gap);
        let height = (f64::from(*level) * top_limit).max(2.0);
        let gradient = LinearGradient::new(0.0, h, 0.0, h - height);
        gradient.add_color_stop_rgba(0.0, colors.accent.0, colors.accent.1, colors.accent.2, (0.07 * density).clamp(0.0, 1.0));
        let tip = mix(colors.accent, colors.bright, 0.7);
        gradient.add_color_stop_rgba(1.0, tip.0, tip.1, tip.2, (0.3 * density).clamp(0.0, 1.0));
        cr.set_source(&gradient).ok();
        round_top(cr, x, h - height, bar, height, (bar / 2.0).min(4.0));
        cr.fill().ok();
        if let Some(peak) = scene.peaks.get(i) {
            let y = h - f64::from(*peak) * top_limit - 5.0;
            rgba(cr, colors.bright, 0.5, density);
            round_top(cr, x, y.min(h - height - 4.0).max(0.0), bar, 3.0, 1.5);
            cr.fill().ok();
        }
    }
}

fn round_top(cr: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h);
    cr.new_sub_path();
    cr.move_to(x, y + h);
    cr.line_to(x, y + r);
    cr.arc(x + r, y + r, r, PI, 3.0 * FRAC_PI_2);
    cr.line_to(x + w - r, y);
    cr.arc(x + w - r, y + r, r, 3.0 * FRAC_PI_2, TAU);
    cr.line_to(x + w, y + h);
    cr.close_path();
}

fn wave(cr: &Context, w: f64, h: f64, scene: &Scene, colors: &Colors, density: f64) {
    let n = scene.wave.len();
    if n < 2 {
        return;
    }
    let mid = h * 0.5;
    let amp = h * 0.32;
    let point = |i: usize| (i as f64 / (n - 1) as f64 * w, mid - f64::from(scene.wave[i]) * amp);
    let trace = |cr: &Context| {
        let (x0, y0) = point(0);
        cr.move_to(x0, y0);
        for i in 1..n {
            let (x, y) = point(i);
            let (px, py) = point(i - 1);
            cr.curve_to((px + x) / 2.0, py, (px + x) / 2.0, y, x, y);
        }
    };
    cr.set_line_cap(gtk::cairo::LineCap::Round);
    cr.set_line_join(gtk::cairo::LineJoin::Round);
    // A soft glow, then the line itself.
    for (width, alpha, color) in [(12.0, 0.08, colors.accent), (7.0, 0.16, colors.accent), (3.0, 0.7, mix(colors.accent, colors.bright, 0.5))] {
        cr.set_line_width(width);
        rgba(cr, color, alpha, density);
        trace(cr);
        cr.stroke().ok();
    }
}

fn radial(cr: &Context, w: f64, h: f64, scene: &Scene, colors: &Colors, density: f64) {
    let n = scene.levels.len();
    if n == 0 {
        return;
    }
    let size = w.min(h);
    let bass = f64::from(scene.bass).clamp(0.0, 1.0);
    let (cx, cy) = (w / 2.0, h / 2.0);
    let r0 = size * 0.22 * (1.0 + 0.14 * bass);
    let reach = size * 0.26;
    cr.set_line_cap(gtk::cairo::LineCap::Round);
    rgba(cr, colors.accent, 0.12 + 0.2 * bass, density);
    cr.set_line_width(2.0);
    cr.new_sub_path();
    cr.arc(cx, cy, r0 * 0.96, 0.0, TAU);
    cr.stroke().ok();
    let ray = (TAU * r0 / (2.0 * n as f64) * 0.55).clamp(2.0, 7.0);
    cr.set_line_width(ray);
    for (i, level) in scene.levels.iter().enumerate() {
        let level = f64::from(*level);
        let t = (i as f64 + 0.5) / n as f64;
        // Each band is drawn on the right and mirrored on the left, low notes at the bottom.
        for side in [1.0f64, -1.0] {
            let a = FRAC_PI_2 - side * t * PI;
            let (ux, uy) = (a.cos(), a.sin());
            let to = r0 + 3.0 + level * reach;
            rgba(cr, mix(colors.accent, colors.bright, level), 0.28 + 0.4 * level, density);
            cr.move_to(cx + ux * r0, cy + uy * r0);
            cr.line_to(cx + ux * to, cy + uy * to);
            cr.stroke().ok();
        }
    }
}

fn ribbon(cr: &Context, w: f64, h: f64, scene: &Scene, colors: &Colors, density: f64) {
    let n = scene.levels.len();
    if n < 2 {
        return;
    }
    let top = h * 0.9;
    let curve = |cr: &Context, values: &[f32], lift: f64| {
        let at = |i: usize| (i as f64 / (n - 1) as f64 * w, h - f64::from(values[i]) * top - lift);
        let (x0, y0) = at(0);
        cr.move_to(x0, y0);
        // Catmull-Rom through the points, as cubic curves.
        for i in 0..n - 1 {
            let (p0, p1, p2, p3) = (at(i.saturating_sub(1)), at(i), at(i + 1), at((i + 2).min(n - 1)));
            cr.curve_to(
                p1.0 + (p2.0 - p0.0) / 6.0,
                p1.1 + (p2.1 - p0.1) / 6.0,
                p2.0 - (p3.0 - p1.0) / 6.0,
                p2.1 - (p3.1 - p1.1) / 6.0,
                p2.0,
                p2.1,
            );
        }
    };
    if scene.peaks.len() == n {
        cr.set_line_width(1.5);
        rgba(cr, colors.bright, 0.25, density);
        curve(cr, &scene.peaks, 0.0);
        cr.stroke().ok();
    }
    curve(cr, &scene.levels, 0.0);
    cr.line_to(w, h);
    cr.line_to(0.0, h);
    cr.close_path();
    let gradient = LinearGradient::new(0.0, h, 0.0, h * 0.1);
    gradient.add_color_stop_rgba(0.0, colors.accent.0, colors.accent.1, colors.accent.2, (0.03 * density).clamp(0.0, 1.0));
    gradient.add_color_stop_rgba(1.0, colors.accent.0, colors.accent.1, colors.accent.2, (0.3 * density).clamp(0.0, 1.0));
    cr.set_source(&gradient).ok();
    cr.fill().ok();
    cr.set_line_width(2.5);
    cr.set_line_join(gtk::cairo::LineJoin::Round);
    rgba(cr, mix(colors.accent, colors.bright, 0.6), 0.6, density);
    curve(cr, &scene.levels, 0.0);
    cr.stroke().ok();
}

struct State {
    analyzer: Analyzer,
    style: VisualStyle,
    on: bool,
    strength: u8,
    smooth: u8,
    density: u8,
    scope: Option<Scope>,
    samples: Vec<f32>,
    scene: Scene,
    colors: Colors,
    colors_at: u64,
    last_frame: Option<i64>,
    // Nothing to show and nothing changing, so a frame needs no redraw.
    resting: bool,
}

/// The drawing area behind the player. It follows the music while it is on, shown and playing.
pub struct Visualizer {
    pub area: gtk::DrawingArea,
    state: Rc<RefCell<State>>,
}

impl Visualizer {
    pub fn new(style: VisualStyle, on: bool) -> Self {
        let state = Rc::new(RefCell::new(State {
            analyzer: Analyzer::new(),
            style,
            on,
            strength: 50,
            smooth: 50,
            density: 50,
            scope: None,
            samples: Vec::new(),
            scene: Scene::default(),
            colors: Colors::from_theme(),
            colors_at: rust_commander::theme::version(),
            last_frame: None,
            resting: true,
        }));
        let area = gtk::DrawingArea::new();
        area.set_hexpand(true);
        area.set_vexpand(true);
        area.set_can_target(false);
        let shown = Rc::clone(&state);
        area.set_draw_func(move |_, cr, w, h| {
            let state = shown.borrow();
            if state.on {
                draw_ink(cr, state.style, f64::from(w), f64::from(h), &state.scene, &state.colors, ink(state.density));
            }
        });
        let live = Rc::clone(&state);
        area.add_tick_callback(move |area, clock| {
            tick(area, clock.frame_time(), &mut live.borrow_mut());
            gtk::glib::ControlFlow::Continue
        });
        Self { area, state }
    }

    pub fn set_scope(&self, scope: Scope) {
        self.state.borrow_mut().scope = Some(scope);
    }

    pub fn set_style(&self, style: VisualStyle) {
        let mut state = self.state.borrow_mut();
        if state.style != style {
            state.style = style;
            state.resting = false;
            self.area.queue_draw();
        }
    }

    /// Strength, smoothness and density, each 0 to 100. 50 is the original picture.
    pub fn set_tune(&self, strength: u8, smooth: u8, density: u8) {
        let mut state = self.state.borrow_mut();
        let strength = strength.min(100);
        let smooth = smooth.min(100);
        let density = density.min(100);
        if state.strength == strength && state.smooth == smooth && state.density == density {
            return;
        }
        state.strength = strength;
        state.smooth = smooth;
        state.density = density;
        state.analyzer.set_fall(fall_scale(smooth));
        state.resting = false;
        self.area.queue_draw();
    }

    pub fn set_on(&self, on: bool) {
        let mut state = self.state.borrow_mut();
        if state.on != on {
            state.on = on;
            state.resting = false;
            self.area.queue_draw();
        }
    }
}

fn tick(area: &gtk::DrawingArea, frame_time: i64, state: &mut State) {
    let dt = state.last_frame.map_or(0.016, |last| ((frame_time - last) as f32 / 1_000_000.0).clamp(0.001, 0.1));
    state.last_frame = Some(frame_time);
    if !state.on || !area.is_mapped() {
        return;
    }
    let version = rust_commander::theme::version();
    if version != state.colors_at {
        state.colors = Colors::from_theme();
        state.colors_at = version;
    }
    let (rate, playing) = match &state.scope {
        Some(scope) => {
            let mut samples = std::mem::take(&mut state.samples);
            let rate = scope.read(WINDOW, &mut samples);
            state.samples = samples;
            (rate, scope.playing())
        }
        None => (0, false),
    };
    // A pause holds the sound still, so the bars are fed silence and fall.
    if !playing {
        state.samples.clear();
    }
    let width = area.width().max(1) as usize;
    let bands = (width / 14).clamp(16, 64);
    let points = (width / 6).clamp(48, 220);
    let samples = std::mem::take(&mut state.samples);
    let frame = state.analyzer.update(&samples, if rate == 0 { 44_100 } else { rate }, dt, bands, points);
    let busy = frame.levels.iter().any(|l| *l > 0.002) || frame.wave.iter().any(|w| w.abs() > 0.002);
    let gain = gain(state.strength);
    state.scene.levels.clear();
    state.scene.levels.extend(frame.levels.iter().map(|level| (level * gain).clamp(0.0, 1.0)));
    state.scene.peaks.clear();
    state.scene.peaks.extend(frame.peaks.iter().map(|level| (level * gain).clamp(0.0, 1.0)));
    state.scene.wave.clear();
    state.scene.wave.extend(frame.wave.iter().map(|sample| (sample * gain).clamp(-1.0, 1.0)));
    state.scene.bass = (frame.bass * gain).clamp(0.0, 1.0);
    state.samples = samples;
    if busy || !state.resting {
        area.queue_draw();
    }
    state.resting = !busy;
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::cairo::{Format, ImageSurface};

    fn colors() -> Colors {
        Colors { accent: (0.3, 0.5, 1.0), bright: (0.9, 0.95, 1.0) }
    }

    fn scene(n: usize, level: f32) -> Scene {
        Scene {
            levels: (0..n).map(|i| level * (1.0 - i as f32 / n as f32 * 0.5)).collect(),
            peaks: (0..n).map(|i| (level * 1.1 * (1.0 - i as f32 / n as f32 * 0.5)).min(1.0)).collect(),
            wave: (0..n * 3).map(|i| (i as f32 * 0.3).sin() * level).collect(),
            bass: level,
        }
    }

    fn render(style: VisualStyle, w: i32, h: i32, scene: &Scene) -> Vec<u8> {
        let mut surface = ImageSurface::create(Format::ARgb32, w, h).unwrap();
        {
            let cr = Context::new(&surface).unwrap();
            draw_ink(&cr, style, f64::from(w), f64::from(h), scene, &colors(), 1.0);
        }
        surface.data().unwrap().to_vec()
    }

    fn coverage(pixels: &[u8]) -> f64 {
        pixels.chunks_exact(4).filter(|px| px[3] > 8).count() as f64 / (pixels.len() / 4) as f64
    }

    #[test]
    fn every_style_draws_something_but_never_the_whole_area() {
        for style in VisualStyle::ALL {
            let pixels = render(style, 400, 300, &scene(32, 0.7));
            let cover = coverage(&pixels);
            assert!(cover > 0.01, "{style:?} drew almost nothing: {cover}");
            assert!(cover < 0.9, "{style:?} covers the text: {cover}");
        }
    }

    #[test]
    fn louder_music_draws_more() {
        for style in VisualStyle::ALL {
            let quiet = coverage(&render(style, 300, 200, &scene(32, 0.15)));
            let loud = coverage(&render(style, 300, 200, &scene(32, 0.9)));
            assert!(loud > quiet, "{style:?}: {quiet} then {loud}");
        }
    }

    #[test]
    fn empty_and_tiny_scenes_and_areas_are_fine() {
        for style in VisualStyle::ALL {
            render(style, 300, 200, &Scene::default());
            render(style, 300, 200, &scene(1, 0.5));
            render(style, 1, 1, &scene(16, 0.5));
            render(style, 3, 3, &scene(16, 0.5));
            // Wild values are clamped by the analyzer, but drawing must not hang on them either.
            let mut wild = scene(16, 0.5);
            wild.levels[3] = 5.0;
            wild.wave[2] = -9.0;
            render(style, 120, 80, &wild);
        }
    }

    #[test]
    fn silence_draws_a_thin_baseline_at_most() {
        for style in [VisualStyle::Bars, VisualStyle::Ribbon, VisualStyle::Radial] {
            let cover = coverage(&render(style, 300, 200, &scene(32, 0.0)));
            assert!(cover < 0.12, "{style:?} fills {cover} in silence");
        }
    }

    #[test]
    fn the_colors_come_from_the_given_palette() {
        let red = Colors { accent: (1.0, 0.0, 0.0), bright: (1.0, 0.2, 0.2) };
        let mut surface = ImageSurface::create(Format::ARgb32, 200, 120).unwrap();
        {
            let cr = Context::new(&surface).unwrap();
            draw_ink(&cr, VisualStyle::Bars, 200.0, 120.0, &scene(16, 0.8), &red, 1.0);
        }
        let data = surface.data().unwrap();
        // ARGB32 is premultiplied BGRA on little-endian: red must dominate blue wherever it is drawn.
        let (mut r, mut b) = (0u64, 0u64);
        for px in data.chunks_exact(4).filter(|px| px[3] > 8) {
            r += u64::from(px[2]);
            b += u64::from(px[0]);
        }
        assert!(r > b * 3, "red {r} blue {b}");
    }

    fn weight(style: VisualStyle, density: f64) -> u64 {
        let mut surface = ImageSurface::create(Format::ARgb32, 300, 200).unwrap();
        {
            let cr = Context::new(&surface).unwrap();
            draw_ink(&cr, style, 300.0, 200.0, &scene(32, 0.7), &colors(), density);
        }
        surface.data().unwrap().chunks_exact(4).map(|px| u64::from(px[3])).sum()
    }

    #[test]
    fn density_fades_the_picture_and_can_deepen_it() {
        let faint = weight(VisualStyle::Bars, 0.4);
        let normal = weight(VisualStyle::Bars, 1.0);
        let deep = weight(VisualStyle::Bars, 2.0);
        assert!(faint < normal && normal < deep, "{faint} {normal} {deep}");
    }

    #[test]
    fn the_middle_of_each_knob_is_the_original_picture() {
        assert_eq!(gain(50), 1.0);
        assert_eq!(gain(0), 0.0);
        assert_eq!(gain(100), 2.0);
        assert!((fall_scale(50) - 1.0).abs() < 1e-6);
        assert!(fall_scale(0) > fall_scale(100));
        assert_eq!(ink(50), 1.0);
    }
}

use super::fft::Fft;

/// Samples the spectrum is taken over: 46 ms at 44.1 kHz.
pub const WINDOW: usize = 2048;

const LOW_HZ: f32 = 40.0;
const HIGH_HZ: f32 = 16_000.0;
// Loudness mapped onto the bars: this far below full scale is empty, this far below is full.
const FLOOR_DB: f32 = -72.0;
const CEIL_DB: f32 = -8.0;
// Highs carry less energy in music, so they get a lift to show at all.
const TILT_DB_PER_OCTAVE: f32 = 3.0;
const RELEASE_PER_SECOND: f32 = 1.7;
const GRAVITY: f32 = 3.2;

/// What a visualizer needs for one frame.
pub struct Frame<'a> {
    /// Loudness per band, 0 to 1, low pitch first.
    pub levels: &'a [f32],
    /// A cap above each band that stays high a moment and then falls with gravity.
    pub peaks: &'a [f32],
    /// The wave shape, -1 to 1.
    pub wave: &'a [f32],
    /// How hard the lowest bands hit, 0 to 1.
    pub bass: f32,
}

pub struct Analyzer {
    fft: Fft,
    window: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
    levels: Vec<f32>,
    peaks: Vec<f32>,
    fall: Vec<f32>,
    wave: Vec<f32>,
    bass: f32,
    // Scales the wave toward the full height whatever the loudness.
    wave_gain: f32,
    // 1 is the built-in fall. Lower hangs, higher snaps.
    fall_scale: f32,
}

impl Default for Analyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl Analyzer {
    pub fn new() -> Self {
        let window = (0..WINDOW)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (WINDOW - 1) as f32).cos())
            .collect();
        Self {
            fft: Fft::new(WINDOW),
            window,
            re: vec![0.0; WINDOW],
            im: vec![0.0; WINDOW],
            levels: Vec::new(),
            peaks: Vec::new(),
            fall: Vec::new(),
            wave: Vec::new(),
            bass: 0.0,
            wave_gain: 1.0,
            fall_scale: 1.0,
        }
    }

    /// How fast the bars and their caps fall. 1 is the built-in speed.
    pub fn set_fall(&mut self, scale: f32) {
        self.fall_scale = scale.clamp(0.2, 1.8);
    }

    /// Forgets the bars, for a new track.
    pub fn reset(&mut self) {
        self.levels.fill(0.0);
        self.peaks.fill(0.0);
        self.fall.fill(0.0);
        self.wave.fill(0.0);
        self.bass = 0.0;
        self.wave_gain = 1.0;
    }

    /// `samples` are the most recent mono samples, newest last. `dt` is the time since the last
    /// call in seconds, which sets how fast the bars fall.
    pub fn update(&mut self, samples: &[f32], rate: u32, dt: f32, bands: usize, points: usize) -> Frame<'_> {
        let bands = bands.max(1);
        if self.levels.len() != bands {
            self.levels = vec![0.0; bands];
            self.peaks = vec![0.0; bands];
            self.fall = vec![0.0; bands];
        }
        let dt = dt.clamp(0.0, 0.25);
        let rate = rate.max(8_000) as f32;
        let release = RELEASE_PER_SECOND * self.fall_scale;
        let gravity = GRAVITY * self.fall_scale;

        // The newest WINDOW samples, with silence in front if there are fewer.
        let have = samples.len().min(WINDOW);
        let lead = WINDOW - have;
        self.re[..lead].fill(0.0);
        for (i, s) in samples[samples.len() - have..].iter().enumerate() {
            self.re[lead + i] = if s.is_finite() { *s } else { 0.0 };
        }
        let mut peak = 0.0f32;
        for (i, w) in self.window.iter().enumerate() {
            peak = peak.max(self.re[i].abs());
            self.re[i] *= w;
        }
        self.im.fill(0.0);
        self.fft.transform(&mut self.re, &mut self.im);

        let high = HIGH_HZ.min(rate * 0.45);
        let bins = WINDOW / 2;
        let hz_per_bin = rate / WINDOW as f32;
        for band in 0..bands {
            let lo = LOW_HZ * (high / LOW_HZ).powf(band as f32 / bands as f32);
            let hi = LOW_HZ * (high / LOW_HZ).powf((band + 1) as f32 / bands as f32);
            let first = ((lo / hz_per_bin) as usize).clamp(1, bins - 1);
            let last = ((hi / hz_per_bin).ceil() as usize).clamp(first + 1, bins);
            let mut mag = 0.0f32;
            for k in first..last {
                mag = mag.max((self.re[k] * self.re[k] + self.im[k] * self.im[k]).sqrt());
            }
            // A Hann window passes half the amplitude, so 4/n gives the sine's own height.
            let amplitude = mag * 4.0 / WINDOW as f32;
            let centre = (lo * hi).sqrt();
            let db = 20.0 * amplitude.max(1e-7).log10() + TILT_DB_PER_OCTAVE * (centre / 1000.0).log2();
            let value = ((db - FLOOR_DB) / (CEIL_DB - FLOOR_DB)).clamp(0.0, 1.0);

            let level = &mut self.levels[band];
            *level = if value > *level { value } else { value.max(*level - release * dt) };
            let (cap, fall) = (&mut self.peaks[band], &mut self.fall[band]);
            if *level >= *cap {
                *cap = *level;
                *fall = 0.0;
            } else {
                *fall += gravity * dt;
                *cap = (*cap - *fall * dt).max(*level);
            }
        }
        let lowest = bands.min(4);
        self.bass = self.levels[..lowest].iter().sum::<f32>() / lowest as f32;

        // The wave: the loudest sample in each slice keeps its shape at any width.
        let points = points.max(2);
        let recent = &samples[samples.len() - have..];
        self.wave.resize(points, 0.0);
        let target = (0.9 / peak.max(0.02)).min(20.0);
        self.wave_gain += (target - self.wave_gain) * if target < self.wave_gain { 0.5 } else { 0.05 };
        for i in 0..points {
            let from = i * recent.len() / points;
            let to = ((i + 1) * recent.len() / points).max(from + 1).min(recent.len());
            let mut strongest = 0.0f32;
            for s in recent.get(from..to).unwrap_or(&[]) {
                if s.abs() > strongest.abs() && s.is_finite() {
                    strongest = *s;
                }
            }
            self.wave[i] = (strongest * self.wave_gain).clamp(-1.0, 1.0);
        }
        Frame { levels: &self.levels, peaks: &self.peaks, wave: &self.wave, bass: self.bass }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, rate: u32, amp: f32, len: usize) -> Vec<f32> {
        (0..len).map(|i| amp * (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin()).collect()
    }

    fn loudest(levels: &[f32]) -> usize {
        levels.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0
    }

    fn band_of(hz: f32, rate: u32, bands: usize) -> usize {
        let high = HIGH_HZ.min(rate as f32 * 0.45);
        ((hz / LOW_HZ).ln() / (high / LOW_HZ).ln() * bands as f32) as usize
    }

    #[test]
    fn a_tone_lights_the_band_it_sits_in() {
        for hz in [100.0f32, 440.0, 1000.0, 4000.0, 12000.0] {
            let mut a = Analyzer::new();
            let frame = a.update(&sine(hz, 44_100, 0.5, 4096), 44_100, 0.016, 40, 64);
            let top = loudest(frame.levels);
            let want = band_of(hz, 44_100, 40);
            assert!(top.abs_diff(want) <= 1, "{hz} Hz lit band {top}, expected {want}");
            assert!(frame.levels[top] > 0.5, "{hz} Hz is too faint: {}", frame.levels[top]);
        }
    }

    #[test]
    fn silence_is_flat_and_nothing_panics_on_odd_input() {
        let mut a = Analyzer::new();
        let frame = a.update(&[], 44_100, 0.016, 16, 32);
        assert!(frame.levels.iter().all(|l| *l == 0.0));
        assert_eq!(frame.wave.len(), 32);
        let frame = a.update(&[0.0; 100], 48_000, 0.016, 1, 2);
        assert_eq!(frame.levels.len(), 1);
        let frame = a.update(&[f32::NAN, f32::INFINITY, 0.5, -0.5], 44_100, 0.016, 8, 8);
        assert!(frame.levels.iter().all(|l| l.is_finite() && (0.0..=1.0).contains(l)));
        assert!(frame.wave.iter().all(|w| w.is_finite()));
        // A wild dt and tiny rates are tamed.
        let frame = a.update(&sine(300.0, 8_000, 0.5, 512), 100, 99.0, 8, 8);
        assert!(frame.levels.iter().all(|l| (0.0..=1.0).contains(l)));
    }

    #[test]
    fn levels_stay_inside_zero_to_one_even_for_full_scale_noise() {
        let mut a = Analyzer::new();
        let mut x = 12345u32;
        let noise: Vec<f32> = (0..WINDOW)
            .map(|_| {
                x = x.wrapping_mul(1664525).wrapping_add(1013904223);
                (x >> 8) as f32 / (1 << 23) as f32 - 1.0
            })
            .collect();
        let frame = a.update(&noise, 44_100, 0.016, 48, 100);
        assert!(frame.levels.iter().all(|l| (0.0..=1.0).contains(l)));
        assert!(frame.peaks.iter().all(|l| (0.0..=1.0).contains(l)));
        assert!(frame.wave.iter().all(|w| (-1.0..=1.0).contains(w)));
        assert!(frame.levels.iter().sum::<f32>() > 5.0, "noise should light many bands");
    }

    #[test]
    fn the_bars_fall_slowly_when_the_sound_stops() {
        let mut a = Analyzer::new();
        let tone = sine(440.0, 44_100, 0.8, 4096);
        let band = band_of(440.0, 44_100, 32);
        let high = a.update(&tone, 44_100, 0.016, 32, 16).levels[band];
        assert!(high > 0.6);
        let mut last = high;
        let mut steps = 0;
        let silence = vec![0.0; 4096];
        loop {
            let level = a.update(&silence, 44_100, 0.016, 32, 16).levels[band];
            assert!(level <= last + 1e-6, "the bar rose in silence");
            last = level;
            steps += 1;
            if level == 0.0 {
                break;
            }
            assert!(steps < 200, "never reaches zero");
        }
        // It takes a moment, not one frame: well over a tenth of a second.
        assert!(steps > 6, "fell in {steps} frames");
    }

    #[test]
    fn a_faster_fall_reaches_silence_sooner() {
        let tone = sine(440.0, 44_100, 0.8, 4096);
        let band = band_of(440.0, 44_100, 32);
        let silence = vec![0.0; 4096];
        let steps = |scale: f32| {
            let mut a = Analyzer::new();
            a.set_fall(scale);
            a.update(&tone, 44_100, 0.016, 32, 16);
            let mut n = 0;
            loop {
                let level = a.update(&silence, 44_100, 0.016, 32, 16).levels[band];
                n += 1;
                if level == 0.0 || n > 400 {
                    break;
                }
            }
            n
        };
        assert!(steps(1.8) < steps(1.0));
    }

    #[test]
    fn the_peak_cap_hangs_above_the_bar_and_then_accelerates_down() {
        let mut a = Analyzer::new();
        let band = band_of(440.0, 44_100, 32);
        let top = a.update(&sine(440.0, 44_100, 0.8, 4096), 44_100, 0.016, 32, 16).peaks[band];
        let silence = vec![0.0; 4096];
        let mut caps = vec![top];
        for _ in 0..60 {
            let frame = a.update(&silence, 44_100, 0.016, 32, 16);
            assert!(frame.peaks[band] >= frame.levels[band], "the cap is under its bar");
            caps.push(frame.peaks[band]);
        }
        assert!(caps.windows(2).all(|w| w[1] <= w[0] + 1e-6), "the cap rose");
        // Falling with gravity: each step drops more than the one before, until it lands.
        let early = caps[0] - caps[5];
        let later = caps[5] - caps[10];
        assert!(later > early || caps[10] == 0.0, "no acceleration: {early} then {later}");
        assert!(caps.last().unwrap() < &top);
    }

    #[test]
    fn bass_follows_the_low_notes() {
        let mut a = Analyzer::new();
        let low = a.update(&sine(60.0, 44_100, 0.8, 4096), 44_100, 0.016, 32, 16).bass;
        a.reset();
        let high = a.update(&sine(9000.0, 44_100, 0.8, 4096), 44_100, 0.016, 32, 16).bass;
        assert!(low > 0.3 && high < 0.05, "{low} vs {high}");
    }

    #[test]
    fn the_wave_fills_the_height_for_quiet_and_loud_music_alike() {
        for amp in [0.05f32, 0.9] {
            let mut a = Analyzer::new();
            let samples = sine(200.0, 44_100, amp, 4096);
            let mut frame_max = 0.0f32;
            for _ in 0..80 {
                let frame = a.update(&samples, 44_100, 0.016, 16, 120);
                assert_eq!(frame.wave.len(), 120);
                frame_max = frame.wave.iter().fold(0.0f32, |m, w| m.max(w.abs()));
            }
            assert!(frame_max > 0.6, "amp {amp}: the wave only reaches {frame_max}");
        }
    }

    #[test]
    fn changing_the_band_count_starts_clean() {
        let mut a = Analyzer::new();
        a.update(&sine(440.0, 44_100, 0.8, 4096), 44_100, 0.016, 32, 16);
        let frame = a.update(&[], 44_100, 0.016, 12, 16);
        assert_eq!(frame.levels.len(), 12);
        assert_eq!(frame.peaks.len(), 12);
        assert!(frame.levels.iter().all(|l| *l == 0.0));
    }
}

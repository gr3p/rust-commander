/// A radix-2 FFT for the visualizer. Small and exact enough for drawing, and it needs no crate.
pub struct Fft {
    n: usize,
    cos: Vec<f32>,
    sin: Vec<f32>,
    rev: Vec<usize>,
}

impl Fft {
    /// `n` must be a power of two of at least 2.
    pub fn new(n: usize) -> Self {
        assert!(n >= 2 && n.is_power_of_two(), "FFT size must be a power of two");
        let bits = n.trailing_zeros();
        let rev = (0..n).map(|i| i.reverse_bits() >> (usize::BITS - bits)).collect();
        let (mut cos, mut sin) = (Vec::with_capacity(n / 2), Vec::with_capacity(n / 2));
        for k in 0..n / 2 {
            let angle = -2.0 * std::f64::consts::PI * k as f64 / n as f64;
            cos.push(angle.cos() as f32);
            sin.push(angle.sin() as f32);
        }
        Self { n, cos, sin, rev }
    }

    pub fn size(&self) -> usize {
        self.n
    }

    /// In place. `re` and `im` hold `n` values each.
    pub fn transform(&self, re: &mut [f32], im: &mut [f32]) {
        assert!(re.len() == self.n && im.len() == self.n, "wrong buffer size");
        for i in 0..self.n {
            let j = self.rev[i];
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= self.n {
            let half = len / 2;
            let step = self.n / len;
            for start in (0..self.n).step_by(len) {
                for k in 0..half {
                    let (wr, wi) = (self.cos[k * step], self.sin[k * step]);
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len *= 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive(input: &[f32]) -> (Vec<f64>, Vec<f64>) {
        let n = input.len();
        let (mut re, mut im) = (vec![0.0; n], vec![0.0; n]);
        for k in 0..n {
            for (t, x) in input.iter().enumerate() {
                let angle = -2.0 * std::f64::consts::PI * (k * t) as f64 / n as f64;
                re[k] += f64::from(*x) * angle.cos();
                im[k] += f64::from(*x) * angle.sin();
            }
        }
        (re, im)
    }

    #[test]
    fn matches_a_plain_dft() {
        for n in [2usize, 4, 8, 64, 256] {
            let input: Vec<f32> = (0..n).map(|i| ((i * 37 + 11) % 101) as f32 / 50.0 - 1.0).collect();
            let (want_re, want_im) = naive(&input);
            let (mut re, mut im) = (input.clone(), vec![0.0; n]);
            Fft::new(n).transform(&mut re, &mut im);
            for k in 0..n {
                assert!((f64::from(re[k]) - want_re[k]).abs() < 1e-3, "n={n} bin {k} re");
                assert!((f64::from(im[k]) - want_im[k]).abs() < 1e-3, "n={n} bin {k} im");
            }
        }
    }

    #[test]
    fn a_sine_on_a_bin_lands_in_that_bin() {
        let n = 1024;
        let bin = 37;
        let mut re: Vec<f32> = (0..n).map(|i| (2.0 * std::f32::consts::PI * bin as f32 * i as f32 / n as f32).sin()).collect();
        let mut im = vec![0.0; n];
        Fft::new(n).transform(&mut re, &mut im);
        let mag: Vec<f32> = (0..n / 2).map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt()).collect();
        let top = mag.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
        assert_eq!(top, bin);
        // A unit sine has magnitude n/2 in its bin, and almost nothing elsewhere.
        assert!((mag[bin] - n as f32 / 2.0).abs() < 1.0, "{}", mag[bin]);
        assert!(mag.iter().enumerate().filter(|(k, _)| *k != bin).all(|(_, m)| *m < 0.5));
    }

    #[test]
    fn silence_and_a_constant_behave() {
        let fft = Fft::new(16);
        let (mut re, mut im) = (vec![0.0; 16], vec![0.0; 16]);
        fft.transform(&mut re, &mut im);
        assert!(re.iter().chain(&im).all(|v| *v == 0.0));
        let (mut re, mut im) = (vec![1.0; 16], vec![0.0; 16]);
        fft.transform(&mut re, &mut im);
        assert!((re[0] - 16.0).abs() < 1e-4);
        assert!(re[1..].iter().chain(&im).all(|v| v.abs() < 1e-4));
        assert_eq!(fft.size(), 16);
    }

    #[test]
    #[should_panic(expected = "power of two")]
    fn a_size_that_is_not_a_power_of_two_is_refused() {
        let _ = Fft::new(100);
    }
}

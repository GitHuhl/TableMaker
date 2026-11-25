//! Synthetic test voices (for tests and demos): a glottal source through
//! formant resonators.

use std::f64::consts::PI;

/// A vowel by source and filter: a glottal-like pulse train at `f0`
/// through resonators at the given formants (Hz, bandwidth Hz).
pub fn vowel(f0: f64, formants: &[(f64, f64)], sr: u32, secs: f64) -> Vec<f64> {
    Voice {
        f0,
        formants: formants.to_vec(),
        ..Default::default()
    }
    .render(sr, secs)
}

/// A synthetic voice with the instabilities real ones have.
#[derive(Clone, Debug, Default)]
pub struct Voice {
    pub f0: f64,
    pub formants: Vec<(f64, f64)>,
    /// Vibrato rate (Hz) and extent (cents, peak).
    pub vibrato: Option<(f64, f64)>,
    /// Random period-to-period variation (fraction of the period, standard deviation).
    pub jitter: f64,
    /// Aspiration noise added to the source, relative to the pulses.
    pub breath: f64,
    pub seed: u64,
}

impl Voice {
    pub fn render(&self, sr: u32, secs: f64) -> Vec<f64> {
        let srf = sr as f64;
        let n = (secs * srf) as usize;
        let mut seed = self.seed.wrapping_add(0x9E3779B97F4A7C15);
        let rand = move || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut gauss = {
            let mut r = rand;
            move || -> (f64, f64) {
                let (u, v) = (r().max(1e-12), r());
                let m = (-2.0 * u.ln()).sqrt();
                (m * (2.0 * PI * v).cos(), r() - 0.5)
            }
        };
        let mut phase = 0.0;
        let mut factor = 1.0;
        let x: Vec<f64> = (0..n)
            .map(|i| {
                let t = i as f64 / srf;
                let vib = self
                    .vibrato
                    .map_or(0.0, |(rate, cents)| cents * (2.0 * PI * rate * t).sin());
                phase += self.f0 * factor * 2f64.powf(vib / 1200.0) / srf;
                let (g, u) = gauss();
                let noise = self.breath * u * 2.0;
                if phase >= 1.0 {
                    phase -= 1.0;
                    factor = 1.0 + self.jitter * g;
                    1.0 + noise
                } else {
                    noise
                }
            })
            .collect();
        filter(x, &self.formants, srf)
    }
}

fn filter(mut x: Vec<f64>, formants: &[(f64, f64)], srf: f64) -> Vec<f64> {
    let n = x.len();
    // Glottal source: the pulses through two one-pole low-passes, a smooth
    // -12 dB/octave slope with no notches, then lip radiation (+6 dB/octave).
    let pole = (-2.0 * PI * 100.0 / srf).exp();
    for _ in 0..2 {
        let mut y = 0.0;
        for v in x.iter_mut() {
            y = *v + pole * y;
            *v = y;
        }
    }
    for i in (1..n).rev() {
        x[i] -= x[i - 1];
    }
    for &(f, bw) in formants {
        let r = (-PI * bw / srf).exp();
        let (c1, c2) = (2.0 * r * (2.0 * PI * f / srf).cos(), -r * r);
        let gain = 1.0 - c1 - c2;
        let (mut y1, mut y2) = (0.0, 0.0);
        for v in x.iter_mut() {
            let y = gain * *v + c1 * y1 + c2 * y2;
            y2 = y1;
            y1 = y;
            *v = y;
        }
    }
    let peak = x.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    x.iter().map(|v| 0.5 * v / peak).collect()
}

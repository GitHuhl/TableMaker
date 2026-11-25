//! Synthetic test voices (for tests and demos): a glottal source through
//! formant resonators.

use std::f64::consts::PI;

/// A vowel by source and filter: a glottal-like pulse train at `f0`
/// through resonators at the given formants (Hz, bandwidth Hz).
pub fn vowel(f0: f64, formants: &[(f64, f64)], sr: u32, secs: f64) -> Vec<f64> {
    let srf = sr as f64;
    let n = (secs * srf) as usize;
    // Glottal source: an impulse train through two one-pole low-passes, a smooth
    // -12 dB/octave slope with no notches, then lip radiation (+6 dB/octave).
    let mut phase = 0.0;
    let mut x: Vec<f64> = (0..n)
        .map(|_| {
            phase += f0 / srf;
            if phase >= 1.0 {
                phase -= 1.0;
                1.0
            } else {
                0.0
            }
        })
        .collect();
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

//! Spectral envelopes: a cycle's harmonic amplitudes laid out over
//! frequency in Hz, so the timbre can be played back at any pitch with its
//! formants where they were recorded.

use crate::signal::Fft;
use serde::{Deserialize, Serialize};

/// Spacing of envelope points (Hz).
pub const STEP_HZ: f64 = 20.0;
/// Envelope points: 0 Hz up to just over 20 kHz.
pub const POINTS: usize = 1025;
/// Level given where the recording holds nothing (dB below the peak).
pub const FLOOR_DB: f64 = -90.0;

/// Level in dB at every `STEP_HZ`, peak at 0 dB.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub db: Vec<f32>,
}

impl Envelope {
    /// From a cycle rendered at any frame size (bin h = harmonic h) with
    /// fundamental `f0`, of a recording holding frequencies up to `band_limit`.
    /// Between harmonics the level is interpolated in dB; below the
    /// fundamental it holds; above the band it falls to the floor.
    pub fn from_cycle(fft: &mut Fft, frame: &[f64], f0: f64, band_limit: f64) -> Self {
        let spec = fft.spectrum(frame);
        let top = ((band_limit * 0.98 / f0).floor() as usize)
            .min(frame.len() / 2 - 1)
            .max(1);
        let amp_db: Vec<f64> = (0..=top)
            .map(|h| {
                if h == 0 {
                    0.0
                } else {
                    20.0 * (spec[h].norm() + 1e-12).log10()
                }
            })
            .collect();
        let peak = amp_db[1..]
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        let db = (0..POINTS)
            .map(|i| {
                let f = i as f64 * STEP_HZ;
                let pos = f / f0;
                let v = if pos <= 1.0 {
                    amp_db[1]
                } else if pos >= top as f64 {
                    if pos > top as f64 + 1.0 {
                        peak + FLOOR_DB
                    } else {
                        amp_db[top] + (peak + FLOOR_DB - amp_db[top]) * (pos - top as f64)
                    }
                } else {
                    let h = pos.floor() as usize;
                    amp_db[h] + (amp_db[h + 1] - amp_db[h]) * (pos - h as f64)
                };
                (v - peak).max(FLOOR_DB) as f32
            })
            .collect();
        Self { db }
    }

    /// Level (dB) at a frequency.
    pub fn at(&self, freq: f64) -> f64 {
        let pos = (freq / STEP_HZ).max(0.0);
        let i = pos.floor() as usize;
        if i + 1 >= self.db.len() {
            return FLOOR_DB;
        }
        let (a, b) = (self.db[i] as f64, self.db[i + 1] as f64);
        a + (b - a) * (pos - i as f64)
    }

    /// The pointwise mean (in dB) of several envelopes, re-peaked at 0 dB.
    pub fn mean<'a>(envs: impl IntoIterator<Item = &'a Envelope>) -> Option<Envelope> {
        let mut sum = vec![0.0f64; POINTS];
        let mut n = 0;
        for e in envs {
            for (s, v) in sum.iter_mut().zip(&e.db) {
                *s += *v as f64;
            }
            n += 1;
        }
        if n == 0 {
            return None;
        }
        let peak = sum.iter().copied().fold(f64::NEG_INFINITY, f64::max) / n as f64;
        Some(Envelope {
            db: sum
                .iter()
                .map(|s| (s / n as f64 - peak).max(FLOOR_DB) as f32)
                .collect(),
        })
    }

    /// Timbre features for comparing envelopes: mean level in log-spaced
    /// bands from 100 Hz to 5 kHz (where vowels differ), level removed.
    pub fn features(&self) -> Vec<f64> {
        const BANDS: usize = 24;
        let (lo, hi) = (100.0f64, 5000.0f64);
        let edge = |k: usize| lo * (hi / lo).powf(k as f64 / BANDS as f64);
        let v: Vec<f64> = (0..BANDS)
            .map(|k| {
                let (a, b) = (edge(k), edge(k + 1));
                let pts = 8;
                (0..pts)
                    .map(|j| self.at(a + (b - a) * (j as f64 + 0.5) / pts as f64))
                    .sum::<f64>()
                    / pts as f64
            })
            .collect();
        let mean = v.iter().sum::<f64>() / BANDS as f64;
        v.iter().map(|x| x - mean).collect()
    }

    /// Harmonic amplitude ratio H1-H2 (dB) at a pitch: spectral tilt at the
    /// bottom of the spectrum, higher for breathier, softer phonation.
    pub fn h1_h2(&self, f0: f64) -> f64 {
        self.at(f0) - self.at(2.0 * f0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn envelope_follows_the_harmonics_in_hz() {
        // A cycle at 200 Hz whose harmonic h has amplitude 1/h.
        let n = 1024;
        let f0 = 200.0;
        let frame: Vec<f64> = (0..n)
            .map(|i| {
                (1..40)
                    .map(|h| (2.0 * PI * h as f64 * i as f64 / n as f64).sin() / h as f64)
                    .sum()
            })
            .collect();
        let e = Envelope::from_cycle(&mut Fft::new(), &frame, f0, 22050.0);
        assert!(e.at(200.0).abs() < 0.01);
        assert!((e.at(800.0) - 20.0 * (0.25f64).log10()).abs() < 0.01);
        // Midway between harmonics 4 and 5, linearly in dB.
        let mid = 0.5 * (20.0 * (0.25f64).log10() + 20.0 * (0.2f64).log10());
        assert!((e.at(900.0) - mid).abs() < 0.01);
        // Above harmonic 39 there is nothing.
        assert!(e.at(12000.0) <= FLOOR_DB + 1e-9);
    }
}

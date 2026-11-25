//! Fundamental frequency tracking (YIN), so the cycle extractor knows how
//! far apart the zero crossings that bound one period should be.

use crate::signal::Fft;
use rustfft::num_complex::Complex64;

#[derive(Clone, Copy, Debug)]
pub struct PitchSettings {
    pub fmin: f64,
    pub fmax: f64,
    /// Samples between analysis frames.
    pub hop: usize,
    /// YIN absolute threshold on the cumulative mean normalised difference.
    pub threshold: f64,
    /// Frames whose best dip is above this are unvoiced.
    pub voicing: f64,
}

impl Default for PitchSettings {
    fn default() -> Self {
        Self {
            fmin: 30.0,
            fmax: 2000.0,
            hop: 256,
            threshold: 0.15,
            voicing: 0.35,
        }
    }
}

/// A pitch track: one estimate per hop.
#[derive(Clone, Debug)]
pub struct PitchTrack {
    pub sample_rate: u32,
    pub hop: usize,
    /// Sample at the centre of the first frame.
    pub first_centre: usize,
    /// Hz, or `None` where unvoiced.
    pub f0: Vec<Option<f64>>,
}

impl PitchTrack {
    /// A constant pitch everywhere (when the user knows it).
    pub fn fixed(f0: f64, sample_rate: u32, len: usize) -> Self {
        Self {
            sample_rate,
            hop: len.max(1),
            first_centre: 0,
            f0: vec![Some(f0)],
        }
    }

    /// The pitch at a sample position (nearest frame).
    pub fn at(&self, sample: f64) -> Option<f64> {
        if self.f0.is_empty() {
            return None;
        }
        let i = ((sample - self.first_centre as f64) / self.hop as f64).round();
        self.f0[(i.max(0.0) as usize).min(self.f0.len() - 1)]
    }

    pub fn voiced(&self) -> impl Iterator<Item = f64> + '_ {
        self.f0.iter().flatten().copied()
    }

    pub fn median(&self) -> Option<f64> {
        let mut v: Vec<f64> = self.voiced().collect();
        if v.is_empty() {
            return None;
        }
        v.sort_by(f64::total_cmp);
        Some(v[v.len() / 2])
    }

    pub fn voiced_fraction(&self) -> f64 {
        self.voiced().count() as f64 / self.f0.len().max(1) as f64
    }
}

/// Track the fundamental of `x` with YIN.
pub fn track(x: &[f64], sample_rate: u32, s: &PitchSettings) -> PitchTrack {
    let sr = sample_rate as f64;
    let max_tau = ((sr / s.fmin).ceil() as usize).max(4);
    let min_tau = ((sr / s.fmax).floor() as usize).max(2);
    let window = max_tau;
    let frame = window + max_tau + 2;
    let nfft = (2 * frame).next_power_of_two();
    let mut fft = Fft::new();
    let mut f0 = Vec::new();

    // Running sum of squares for the energy terms.
    let mut sq = vec![0.0; x.len() + 1];
    for (i, v) in x.iter().enumerate() {
        sq[i + 1] = sq[i] + v * v;
    }

    let mut start = 0;
    while start + frame <= x.len() {
        let seg = &x[start..start + frame];
        let mut a = vec![Complex64::new(0.0, 0.0); nfft];
        let mut b = a.clone();
        for i in 0..frame {
            b[i].re = seg[i];
            if i < window {
                a[i].re = seg[i];
            }
        }
        fft.forward(&mut a);
        fft.forward(&mut b);
        let mut c: Vec<Complex64> = a.iter().zip(&b).map(|(p, q)| p.conj() * q).collect();
        fft.inverse(&mut c);
        // d(tau) = sum_{j<W} (x_j - x_{j+tau})^2
        let e0 = sq[start + window] - sq[start];
        let d: Vec<f64> = (0..=max_tau + 1)
            .map(|tau| {
                let et = sq[start + tau + window] - sq[start + tau];
                (e0 + et - 2.0 * c[tau].re).max(0.0)
            })
            .collect();
        // Cumulative mean normalised difference.
        let mut cmnd = vec![1.0; d.len()];
        let mut run = 0.0;
        for tau in 1..d.len() {
            run += d[tau];
            cmnd[tau] = if run > 0.0 {
                d[tau] * tau as f64 / run
            } else {
                1.0
            };
        }
        // First dip under the threshold, followed down to its minimum; else the global minimum.
        let mut best = None;
        let mut tau = min_tau;
        while tau <= max_tau {
            if cmnd[tau] < s.threshold {
                while tau < max_tau && cmnd[tau + 1] < cmnd[tau] {
                    tau += 1;
                }
                best = Some(tau);
                break;
            }
            tau += 1;
        }
        let tau = best.unwrap_or_else(|| {
            (min_tau..=max_tau)
                .min_by(|&p, &q| cmnd[p].total_cmp(&cmnd[q]))
                .unwrap_or(min_tau)
        });
        let voiced = cmnd[tau] < s.voicing && e0 > 1e-9 * window as f64;
        f0.push(voiced.then(|| {
            // Parabolic refinement of the dip.
            let (l, m, r) = (cmnd[tau - 1], cmnd[tau], cmnd[tau + 1]);
            let den = l - 2.0 * m + r;
            let shift = if den.abs() > 1e-12 {
                (0.5 * (l - r) / den).clamp(-1.0, 1.0)
            } else {
                0.0
            };
            sr / (tau as f64 + shift)
        }));
        start += s.hop;
    }

    let mut track = PitchTrack {
        sample_rate,
        hop: s.hop,
        first_centre: window,
        f0,
    };
    smooth(&mut track.f0);
    track
}

/// Median-of-five over voiced neighbours: removes isolated octave jumps.
fn smooth(f0: &mut [Option<f64>]) {
    let orig = f0.to_vec();
    for i in 0..f0.len() {
        if orig[i].is_none() {
            continue;
        }
        let mut v: Vec<f64> = orig[i.saturating_sub(2)..(i + 3).min(orig.len())]
            .iter()
            .flatten()
            .copied()
            .collect();
        v.sort_by(f64::total_cmp);
        f0[i] = Some(v[v.len() / 2]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn tracks_a_harmonic_tone() {
        let sr = 44100;
        for f in [55.0, 220.0, 1234.5] {
            let x: Vec<f64> = (0..sr / 2)
                .map(|i| {
                    let t = 2.0 * PI * f * i as f64 / sr as f64;
                    (1..8).map(|h| (h as f64 * t).sin() / h as f64).sum()
                })
                .collect();
            let tr = track(&x, sr, &PitchSettings::default());
            let m = tr.median().unwrap();
            assert!((m / f - 1.0).abs() < 0.002, "{f}: got {m}");
            assert!(tr.voiced_fraction() > 0.95);
        }
    }

    #[test]
    fn silence_is_unvoiced() {
        let tr = track(&vec![0.0; 20000], 44100, &PitchSettings::default());
        assert_eq!(tr.median(), None);
    }
}

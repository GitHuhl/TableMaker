//! Formant tracking over time: the signal is resampled to twice the highest
//! formant of interest, pre-emphasised, cut into overlapping Gaussian
//! windows, and each window's Burg LPC polynomial is solved for its
//! resonances.

use crate::lpc::{self, Formant};
use crate::resample::resample;
use std::f64::consts::PI;

#[derive(Clone, Copy, Debug)]
pub struct FormantSettings {
    /// Highest formant searched for (Hz): about 5000 for adult men, 5500 for
    /// adult women, 8000 for young children.
    pub max_formant: f64,
    /// How many formants to look for (LPC order is twice this).
    pub formants: usize,
    /// Effective window length (s); the Gaussian spans twice this.
    pub window: f64,
    /// Time between frames (s).
    pub step: f64,
    /// Pre-emphasis corner (Hz): flattens the voice's falling spectrum so high formants are found.
    pub pre_emphasis: f64,
}

impl Default for FormantSettings {
    fn default() -> Self {
        Self {
            max_formant: 5500.0,
            formants: 5,
            window: 0.025,
            step: 0.01,
            pre_emphasis: 50.0,
        }
    }
}

/// Widest resonance counted as a formant (Hz).
pub const MAX_BANDWIDTH: f64 = 500.0;

/// Formants over time.
#[derive(Clone, Debug, Default)]
pub struct FormantTrack {
    /// Time of the first frame's centre (s).
    pub start: f64,
    pub step: f64,
    /// The formants of each frame, lowest first.
    pub frames: Vec<Vec<Formant>>,
    /// Level of each frame (dB, relative to full scale).
    pub intensity: Vec<f64>,
}

impl FormantTrack {
    fn index(&self, time: f64) -> Option<usize> {
        if self.frames.is_empty() {
            return None;
        }
        let i = ((time - self.start) / self.step).round();
        Some((i.max(0.0) as usize).min(self.frames.len() - 1))
    }

    /// The formants of the frame nearest `time`.
    pub fn at(&self, time: f64) -> &[Formant] {
        self.index(time).map_or(&[], |i| &self.frames[i])
    }

    pub fn intensity_at(&self, time: f64) -> f64 {
        self.index(time)
            .map_or(f64::NEG_INFINITY, |i| self.intensity[i])
    }
}

/// Track the formants of `x` (any sample rate).
pub fn track(x: &[f64], sample_rate: u32, s: &FormantSettings) -> FormantTrack {
    let fs = 2.0 * s.max_formant;
    let mut y = resample(x, sample_rate as f64, fs);
    let a = (-2.0 * PI * s.pre_emphasis / fs).exp();
    for i in (1..y.len()).rev() {
        y[i] -= a * y[i - 1];
    }
    let n = ((2.0 * s.window * fs).round() as usize).max(8);
    let hop = ((s.step * fs).round() as usize).max(1);
    // Gaussian window, edges brought to zero.
    let edge = (-12.0f64).exp();
    let win: Vec<f64> = (0..n)
        .map(|i| {
            let t = (i as f64 + 0.5) / n as f64 - 0.5;
            ((-48.0 * t * t).exp() - edge) / (1.0 - edge)
        })
        .collect();
    let mut track = FormantTrack {
        start: n as f64 / 2.0 / fs,
        step: hop as f64 / fs,
        ..Default::default()
    };
    let mut seg = vec![0.0; n];
    let mut at = 0;
    while at + n <= y.len() {
        for (i, v) in seg.iter_mut().enumerate() {
            *v = y[at + i] * win[i];
        }
        let power = seg.iter().map(|v| v * v).sum::<f64>() / n as f64;
        track.intensity.push(10.0 * power.max(1e-20).log10());
        let poly = lpc::burg(&seg, 2 * s.formants);
        // Poles broader than this model the source's slope, not a resonance of the vocal tract.
        let mut f = lpc::formants(&poly, fs, MAX_BANDWIDTH);
        f.retain(|f| f.freq < s.max_formant - 50.0);
        f.truncate(s.formants);
        track.frames.push(f);
        at += hop;
    }
    track
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsignal::vowel;

    #[test]
    fn tracks_the_formants_of_a_vowel() {
        let target = [
            (730.0, 90.0),
            (1090.0, 110.0),
            (2440.0, 160.0),
            (3400.0, 250.0),
        ]; // "ah"
        let x = vowel(118.0, &target, 44100, 0.6);
        let t = track(
            &x,
            44100,
            &FormantSettings {
                max_formant: 5000.0,
                ..Default::default()
            },
        );
        let mid: Vec<&[Formant]> = (10..40).map(|i| t.frames[i].as_slice()).collect();
        for (k, &(want, _)) in target.iter().take(3).enumerate() {
            let mut got: Vec<f64> = mid
                .iter()
                .filter_map(|f| f.get(k))
                .map(|f| f.freq)
                .collect();
            got.sort_by(f64::total_cmp);
            let median = got[got.len() / 2];
            assert!(
                (median / want - 1.0).abs() < 0.06,
                "F{}: {median} vs {want}",
                k + 1
            );
        }
    }
}

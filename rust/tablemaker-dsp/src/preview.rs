//! Audition a table: play it at one note while sweeping through every frame.

use crate::signal::{self, Fft};

/// Render `seconds` of the table at `freq` Hz, sweeping from the first
/// frame to the last. Each frame is band-limited for the note first, so
/// the preview does not alias.
pub fn render(frames: &[Vec<f64>], freq: f64, sample_rate: u32, seconds: f64) -> Vec<f64> {
    if frames.is_empty() {
        return Vec::new();
    }
    let size = frames[0].len();
    let sr = sample_rate as f64;
    let max_h = ((sr / 2.0) / freq).floor().max(1.0) as usize;
    let mut fft = Fft::new();
    let limited: Vec<Vec<f64>> = frames
        .iter()
        .map(|f| signal::resample_periodic(&mut fft, f, size, max_h))
        .collect();
    let n = (seconds * sr) as usize;
    let fade = ((0.005 * sr) as usize).max(1);
    let mut phase = 0.0;
    let step = freq * size as f64 / sr;
    let last = (limited.len() - 1) as f64;
    (0..n)
        .map(|i| {
            let pos = last * i as f64 / (n.max(2) - 1) as f64;
            let k = (pos.floor() as usize).min(limited.len().saturating_sub(2));
            let t = if limited.len() > 1 {
                pos - k as f64
            } else {
                0.0
            };
            let read = |f: &[f64]| {
                let j = phase as usize;
                let frac = phase - j as f64;
                f[j] + (f[(j + 1) % size] - f[j]) * frac
            };
            let a = read(&limited[k]);
            let v = if limited.len() > 1 {
                a + (read(&limited[k + 1]) - a) * t
            } else {
                a
            };
            phase = (phase + step) % size as f64;
            let env = (i.min(n - 1 - i) as f64 / fade as f64).min(1.0);
            v * env
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn plays_at_the_requested_pitch() {
        let frame: Vec<f64> = (0..2048)
            .map(|i| (2.0 * PI * i as f64 / 2048.0).sin())
            .collect();
        let out = render(&[frame.clone(), frame], 441.0, 44100, 1.0);
        assert_eq!(out.len(), 44100);
        let rising = out.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        assert!((440..=442).contains(&rising), "{rising} cycles");
    }
}

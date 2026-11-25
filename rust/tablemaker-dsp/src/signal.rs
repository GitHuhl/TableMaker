//! Basic signal processing: DC removal, band-limited oversampling,
//! interpolation, zero crossings and FFT helpers.

use rustfft::num_complex::Complex64;
use rustfft::FftPlanner;
use std::f64::consts::PI;

/// Remove DC and subsonic drift with a zero-phase one-pole high-pass
/// (run forwards then backwards, so the waveform shape is not skewed).
pub fn dc_block(x: &[f64], sample_rate: u32, cutoff_hz: f64) -> Vec<f64> {
    let r = (-2.0 * PI * cutoff_hz / sample_rate as f64).exp();
    let pass = |input: &mut dyn Iterator<Item = f64>| -> Vec<f64> {
        let (mut px, mut py) = (0.0, 0.0);
        input
            .map(|v| {
                let y = v - px + r * py;
                px = v;
                py = y;
                y
            })
            .collect()
    };
    if x.len() < 2 {
        return vec![0.0; x.len()];
    }
    // Mirror the ends, long enough for the filter to settle, so a clip cut off
    // mid-waveform does not start or end with a decaying step. (Mirroring keeps the
    // local mean; odd reflection would add a DC step of twice the edge value.)
    let pad = ((sample_rate as f64 / cutoff_hz) as usize).min(x.len() - 1);
    let n = x.len();
    let padded = (1..=pad)
        .rev()
        .map(|i| x[i])
        .chain(x.iter().copied())
        .chain((1..=pad).map(|i| x[n - 1 - i]));
    let fwd = pass(&mut padded.into_iter());
    let mut back = pass(&mut fwd.iter().rev().copied());
    back.reverse();
    back[pad..pad + x.len()].to_vec()
}

fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term) = (1.0, 1.0);
    for k in 1..50 {
        term *= (x / (2.0 * k as f64)).powi(2);
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// Input samples on each side of the oversampling filter's centre.
pub const OVERSAMPLE_ZEROS: usize = 32;

/// Oversample by an integer factor with a Kaiser-windowed sinc polyphase
/// filter. Output sample `n * factor` lines up with input sample `n` (the
/// filter's delay is compensated), so positions map back by dividing by
/// `factor`.
pub fn oversample(x: &[f64], factor: usize) -> Vec<f64> {
    if factor <= 1 {
        return x.to_vec();
    }
    const ZEROS: usize = OVERSAMPLE_ZEROS;
    const BETA: f64 = 10.0;
    // Pass band ends a little below the original Nyquist frequency.
    let cutoff = 0.47 / factor as f64; // in cycles per oversampled sample
    let taps = 2 * ZEROS * factor + 1;
    let centre = (ZEROS * factor) as f64;
    let norm = bessel_i0(BETA);
    let h: Vec<f64> = (0..taps)
        .map(|i| {
            let t = i as f64 - centre;
            let sinc = if t == 0.0 {
                2.0 * cutoff
            } else {
                (2.0 * PI * cutoff * t).sin() / (PI * t)
            };
            let w = bessel_i0(BETA * (1.0 - (t / centre).powi(2)).max(0.0).sqrt()) / norm;
            sinc * w * factor as f64
        })
        .collect();
    // Phase p's sub-filter: h[p + k * factor], k = 0..2 * ZEROS (+1 for p = 0).
    let phases: Vec<Vec<f64>> = (0..factor)
        .map(|p| h.iter().skip(p).step_by(factor).copied().collect())
        .collect();
    let n = x.len();
    let at = |i: isize| {
        if i >= 0 && (i as usize) < n {
            x[i as usize]
        } else {
            0.0
        }
    };
    let mut y = vec![0.0; n * factor];
    for i in 0..n {
        for (p, sub) in phases.iter().enumerate() {
            // With the zero-stuffed input u (u[jF] = x[j]) and the filter centred at ZF,
            // y[iF + p] = sum_j h[j] u[iF + p + ZF - j]; only j = p + kF hit non-zero
            // samples, at u[(i + Z - k)F] = x[i + Z - k].
            let base = (i + ZEROS) as isize;
            let mut acc = 0.0;
            if i >= ZEROS && i + ZEROS < n {
                for (k, &c) in sub.iter().enumerate() {
                    acc += c * x[base as usize - k];
                }
            } else {
                for (k, &c) in sub.iter().enumerate() {
                    acc += c * at(base - k as isize);
                }
            }
            y[i * factor + p] = acc;
        }
    }
    y
}

/// Four-point cubic (Catmull-Rom) interpolation at a fractional position.
#[inline]
pub fn cubic(x: &[f64], pos: f64) -> f64 {
    let n = x.len() as isize;
    let i = pos.floor() as isize;
    let t = pos - i as f64;
    let g = |k: isize| x[(k.clamp(0, n - 1)) as usize];
    let (a, b, c, d) = (g(i - 1), g(i), g(i + 1), g(i + 2));
    b + 0.5 * t * (c - a + t * (2.0 * a - 5.0 * b + 4.0 * c - d + t * (3.0 * (b - c) + d - a)))
}

/// Linear interpolation at a fractional position.
#[inline]
pub fn linear(x: &[f64], pos: f64) -> f64 {
    let i = (pos.floor() as usize).min(x.len().saturating_sub(2));
    let t = pos - i as f64;
    x[i] + (x[i + 1] - x[i]) * t
}

/// A zero crossing at a fractional sample position.
#[derive(Clone, Copy, Debug)]
pub struct Crossing {
    pub pos: f64,
    /// Slope at the crossing, per sample of the signal it was found in.
    pub slope: f64,
}

/// Rising (negative to non-negative) zero crossings, located between
/// samples by linear interpolation. On an oversampled signal this is
/// accurate to a small fraction of an original sample.
pub fn rising_crossings(x: &[f64]) -> Vec<Crossing> {
    x.windows(2)
        .enumerate()
        .filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0)
        .map(|(i, w)| Crossing {
            pos: i as f64 + w[0] / (w[0] - w[1]),
            slope: w[1] - w[0],
        })
        .collect()
}

/// FFT helper owning a planner.
pub struct Fft {
    planner: FftPlanner<f64>,
}

impl Default for Fft {
    fn default() -> Self {
        Self::new()
    }
}

impl Fft {
    pub fn new() -> Self {
        Self {
            planner: FftPlanner::new(),
        }
    }

    pub fn forward(&mut self, buf: &mut [Complex64]) {
        self.planner.plan_fft_forward(buf.len()).process(buf);
    }

    /// Inverse FFT, scaled by 1/N.
    pub fn inverse(&mut self, buf: &mut [Complex64]) {
        let n = buf.len();
        self.planner.plan_fft_inverse(n).process(buf);
        let s = 1.0 / n as f64;
        buf.iter_mut().for_each(|c| *c *= s);
    }

    /// Spectrum of a real signal.
    pub fn spectrum(&mut self, x: &[f64]) -> Vec<Complex64> {
        let mut buf: Vec<Complex64> = x.iter().map(|&v| Complex64::new(v, 0.0)).collect();
        self.forward(&mut buf);
        buf
    }

    /// Real signal back from a (Hermitian) spectrum.
    pub fn real(&mut self, mut spec: Vec<Complex64>) -> Vec<f64> {
        self.inverse(&mut spec);
        spec.into_iter().map(|c| c.re).collect()
    }
}

/// Resample one period of a periodic signal to `len` samples by moving its
/// harmonics into a spectrum of the new size: exact for band-limited
/// cycles, and band-limits when shrinking. Harmonics above `max_harmonic`
/// and the DC term are dropped.
pub fn resample_periodic(
    fft: &mut Fft,
    cycle: &[f64],
    len: usize,
    max_harmonic: usize,
) -> Vec<f64> {
    let src = fft.spectrum(cycle);
    let n = cycle.len();
    // Keep strictly below both Nyquist bins so no harmonic is ambiguous.
    let keep = max_harmonic.min((n - 1) / 2).min((len - 1) / 2);
    let scale = len as f64 / n as f64;
    let mut dst = vec![Complex64::new(0.0, 0.0); len];
    for h in 1..=keep {
        dst[h] = src[h] * scale;
        dst[len - h] = src[n - h] * scale;
    }
    fft.real(dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversampling_is_aligned_and_band_limited() {
        let sr = 1000.0;
        let x: Vec<f64> = (0..400)
            .map(|i| (2.0 * PI * 37.0 * i as f64 / sr).sin())
            .collect();
        let f = 8;
        let y = oversample(&x, f);
        assert_eq!(y.len(), x.len() * f);
        // Away from the edges every oversampled point is on the original sine.
        for (m, v) in y.iter().enumerate().take(300 * f).skip(100 * f) {
            let t = m as f64 / (sr * f as f64);
            let want = (2.0 * PI * 37.0 * t).sin();
            assert!((v - want).abs() < 2e-3, "at {m}: {v} vs {want}");
        }
    }

    #[test]
    fn finds_crossings_between_samples() {
        let x: Vec<f64> = (0..100).map(|i| (i as f64 * 0.3 - 1.0).sin()).collect();
        let c = rising_crossings(&x);
        // sin(0.3i - 1) rises through zero at i = (1 + 2πk) / 0.3.
        assert!((c[0].pos - 1.0 / 0.3).abs() < 0.02);
        assert!((c[1].pos - (1.0 + 2.0 * PI) / 0.3).abs() < 0.02);
    }

    #[test]
    fn periodic_resampling_keeps_the_shape() {
        let mut fft = Fft::new();
        let wave = |n: usize| -> Vec<f64> {
            (0..n)
                .map(|i| {
                    let t = 2.0 * PI * i as f64 / n as f64;
                    t.sin() + 0.5 * (3.0 * t).sin() + 0.25 * (7.0 * t).cos()
                })
                .collect()
        };
        let up = resample_periodic(&mut fft, &wave(301), 2048, 1000);
        for (a, b) in up.iter().zip(wave(2048)) {
            assert!((a - b).abs() < 1e-9);
        }
        let down = resample_periodic(&mut fft, &wave(5000), 64, 1000);
        for (a, b) in down.iter().zip(wave(64)) {
            assert!((a - b).abs() < 1e-9);
        }
    }
}

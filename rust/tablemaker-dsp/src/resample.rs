//! Sample-rate conversion by any ratio (Kaiser-windowed sinc).

use std::f64::consts::PI;

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

/// Convert `x` from `from` Hz to `to` Hz. Downsampling low-passes at the
/// new Nyquist frequency first, so nothing folds back.
pub fn resample(x: &[f64], from: f64, to: f64) -> Vec<f64> {
    if x.is_empty() || (from - to).abs() < 1e-9 {
        return x.to_vec();
    }
    let ratio = to / from;
    let scale = ratio.min(1.0); // band limit, as a fraction of the input Nyquist
    let cutoff = 0.46 * scale; // cycles per input sample
    const ZEROS: f64 = 16.0;
    let half = ZEROS / scale; // kernel half-width in input samples
    const BETA: f64 = 9.0;
    let norm = bessel_i0(BETA);
    // The kernel, tabulated over its half-width (it is symmetric), read with linear interpolation.
    const RES: f64 = 256.0; // table points per input sample
    let len = (half * RES).ceil() as usize + 2;
    let table: Vec<f64> = (0..len)
        .map(|k| {
            let d = k as f64 / RES;
            let sinc = if d == 0.0 {
                2.0 * cutoff
            } else {
                (2.0 * PI * cutoff * d).sin() / (PI * d)
            };
            sinc * bessel_i0(BETA * (1.0 - (d / half).powi(2)).max(0.0).sqrt()) / norm
        })
        .collect();
    let kernel = |d: f64| {
        let p = d.abs() * RES;
        let k = p as usize;
        if k + 1 >= len {
            return 0.0;
        }
        table[k] + (table[k + 1] - table[k]) * (p - k as f64)
    };
    let n_out = ((x.len() as f64) * ratio).floor() as usize;
    (0..n_out)
        .map(|m| {
            let t = m as f64 / ratio;
            let lo = (t - half).ceil().max(0.0) as usize;
            let hi = ((t + half).floor() as usize).min(x.len() - 1);
            x.iter()
                .enumerate()
                .take(hi + 1)
                .skip(lo)
                .map(|(i, &v)| v * kernel(t - i as f64))
                .sum()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_tones_below_the_new_nyquist_and_drops_those_above() {
        let from = 44100.0;
        let tone = |f: f64, n: usize| -> Vec<f64> {
            (0..n)
                .map(|i| (2.0 * PI * f * i as f64 / from).sin())
                .collect()
        };
        let y = resample(&tone(1000.0, 44100), from, 11000.0);
        assert_eq!(y.len(), 11000);
        for (m, v) in y.iter().enumerate().take(10000).skip(1000) {
            let want = (2.0 * PI * 1000.0 * m as f64 / 11000.0).sin();
            assert!((v - want).abs() < 5e-3, "{m}: {v} vs {want}");
        }
        let above = resample(&tone(8000.0, 44100), from, 11000.0);
        let rms = (above[1000..10000].iter().map(|v| v * v).sum::<f64>() / 9000.0).sqrt();
        assert!(rms < 2e-3, "aliased rms {rms}");
    }
}

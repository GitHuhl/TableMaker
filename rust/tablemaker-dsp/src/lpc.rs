//! Linear prediction: Burg's method, polynomial roots, and formants from
//! the roots.

use rustfft::num_complex::Complex64;

/// Burg's method: the prediction polynomial A(z) = 1 + a1 z^-1 + ... + ap z^-p
/// (returned as [1, a1, ..., ap]) that minimises forward and backward error.
pub fn burg(x: &[f64], order: usize) -> Vec<f64> {
    let n = x.len();
    let mut f = x.to_vec();
    let mut b = x.to_vec();
    let mut a = vec![1.0];
    for m in 0..order.min(n.saturating_sub(1)) {
        let (mut num, mut den) = (0.0, 0.0);
        for i in m + 1..n {
            num += f[i] * b[i - 1];
            den += f[i] * f[i] + b[i - 1] * b[i - 1];
        }
        if den <= 0.0 {
            break;
        }
        let k = -2.0 * num / den;
        let mut next = a.clone();
        next.push(0.0);
        for i in 1..=m + 1 {
            next[i] = a.get(i).copied().unwrap_or(0.0) + k * a[m + 1 - i];
        }
        a = next;
        // Backwards, so b[i - 1] is still the previous order's value when used.
        for i in (m + 1..n).rev() {
            let (fo, bo) = (f[i], b[i - 1]);
            f[i] = fo + k * bo;
            b[i] = bo + k * fo;
        }
    }
    a
}

/// Roots of the polynomial c[0] z^p + c[1] z^(p-1) + ... + c[p] (Aberth-Ehrlich).
pub fn roots(c: &[f64]) -> Vec<Complex64> {
    let p = c.len().saturating_sub(1);
    if p == 0 || c[0] == 0.0 {
        return Vec::new();
    }
    let monic: Vec<f64> = c.iter().map(|v| v / c[0]).collect();
    let eval = |z: Complex64| -> (Complex64, Complex64) {
        // Horner for value and derivative.
        let (mut v, mut d) = (Complex64::new(1.0, 0.0), Complex64::new(0.0, 0.0));
        for &k in &monic[1..] {
            d = d * z + v;
            v = v * z + k;
        }
        (v, d)
    };
    let radius = 1.0 + monic[1..].iter().fold(0.0f64, |m, v| m.max(v.abs()));
    let r0 = radius.min(1.0);
    let mut z: Vec<Complex64> = (0..p)
        .map(|k| {
            Complex64::from_polar(
                r0 * 0.9,
                2.0 * std::f64::consts::PI * (k as f64 + 0.25) / p as f64 + 0.4,
            )
        })
        .collect();
    for _ in 0..500 {
        let mut moved = 0.0f64;
        for i in 0..p {
            let (v, d) = eval(z[i]);
            if v.norm() == 0.0 {
                continue;
            }
            let ratio = v / d;
            let sum: Complex64 = (0..p)
                .filter(|&j| j != i)
                .map(|j| (z[i] - z[j]).inv())
                .sum();
            let w = ratio / (Complex64::new(1.0, 0.0) - ratio * sum);
            if w.is_finite() {
                z[i] -= w;
                moved = moved.max(w.norm());
            }
        }
        if moved < 1e-13 {
            break;
        }
    }
    z
}

/// One formant: centre frequency and bandwidth, in Hz.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Formant {
    pub freq: f64,
    pub bw: f64,
}

/// Formants from a prediction polynomial at sample rate `fs`: the roots in
/// the upper half plane, as resonances, lowest first. Very broad
/// resonances and those at the band edges are not formants.
pub fn formants(a: &[f64], fs: f64, max_bw: f64) -> Vec<Formant> {
    let mut out: Vec<Formant> = roots(a)
        .into_iter()
        .filter(|z| z.im > 1e-9)
        .map(|z| Formant {
            freq: z.arg() * fs / (2.0 * std::f64::consts::PI),
            bw: -z.norm().ln() * fs / std::f64::consts::PI,
        })
        .filter(|f| f.freq > 50.0 && f.freq < fs / 2.0 - 50.0 && f.bw > 0.0 && f.bw < max_bw)
        .collect();
    out.sort_by(|x, y| x.freq.total_cmp(&y.freq));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn finds_polynomial_roots() {
        // (z - 0.5)(z + 0.25)(z^2 - 2 r cos t z + r^2)
        let (r, t) = (0.9f64, 0.7f64);
        let quad = [1.0, -2.0 * r * t.cos(), r * r];
        let lin = [1.0, -0.25, -0.125]; // (z - 0.5)(z + 0.25)
        let mut c = [0.0; 5];
        for (i, a) in lin.iter().enumerate() {
            for (j, b) in quad.iter().enumerate() {
                c[i + j] += a * b;
            }
        }
        let z = roots(&c);
        let has = |w: Complex64| z.iter().any(|v| (v - w).norm() < 1e-9);
        assert!(has(Complex64::new(0.5, 0.0)) && has(Complex64::new(-0.25, 0.0)));
        assert!(has(Complex64::from_polar(r, t)) && has(Complex64::from_polar(r, -t)));
    }

    #[test]
    fn burg_recovers_resonances() {
        // Noise through two resonators at 700 Hz and 1800 Hz (fs 11 kHz).
        let fs = 11000.0;
        let poles = [(700.0, 80.0), (1800.0, 120.0)];
        let mut seed = 7u64;
        let mut x: Vec<f64> = (0..4000)
            .map(|_| {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5
            })
            .collect();
        for (f, bw) in poles {
            let r = (-PI * bw / fs).exp();
            let (c1, c2) = (2.0 * r * (2.0 * PI * f / fs).cos(), -r * r);
            let (mut y1, mut y2) = (0.0, 0.0);
            for v in x.iter_mut() {
                let y = *v + c1 * y1 + c2 * y2;
                y2 = y1;
                y1 = y;
                *v = y;
            }
        }
        let a = burg(&x, 4);
        let f = formants(&a, fs, 1000.0);
        assert_eq!(f.len(), 2, "{f:?}");
        assert!(
            (f[0].freq - 700.0).abs() < 20.0 && (f[1].freq - 1800.0).abs() < 30.0,
            "{f:?}"
        );
    }
}

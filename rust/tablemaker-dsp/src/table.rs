//! Mashing cycles from every clip into one wavetable: pick the frames,
//! order them so neighbours morph smoothly, line up their phases, fill
//! between keyframes, and normalise.

use crate::cycles::Cycle;
use crate::signal::Fft;
use rustfft::num_complex::Complex64;
use std::f64::consts::PI;

/// Which cycles become keyframes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Select {
    /// Evenly through every clip in order: the table follows the clips' timeline.
    Spread,
    /// The most periodic, cleanest-looping cycles.
    Best,
    /// The most different-sounding cycles (farthest-point sampling of their spectra).
    Diverse,
}

/// How keyframes are ordered across the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    /// Clip order, then time within each clip.
    Time,
    /// Darkest to brightest (spectral centroid).
    Brightness,
    /// The shortest path through spectral space, from the darkest frame:
    /// each frame next to the ones it sounds most like.
    Path,
}

/// How frames between keyframes are made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Morph {
    /// Sample-by-sample crossfade of the phase-aligned keyframes.
    Crossfade,
    /// Harmonic amplitudes and phases interpolated separately: no
    /// cancellation dip midway between keyframes whose harmonics disagree in phase.
    Spectral,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Normalize {
    /// One gain for the whole table: keeps the level differences between frames.
    Table,
    /// Every frame to full scale.
    Frame,
    None,
}

#[derive(Clone, Debug)]
pub struct TableSettings {
    pub frames: usize,
    /// Distinct cycles used; the rest are morphed between them. Defaults to `frames`.
    pub keyframes: Option<usize>,
    pub select: Select,
    pub order: Order,
    pub morph: Morph,
    /// Rotate each frame to line up with its neighbour.
    pub align: bool,
    /// Moving average over this many frames each side.
    pub smooth: usize,
    pub normalize: Normalize,
    /// Peak level after normalising.
    pub peak: f64,
}

impl Default for TableSettings {
    fn default() -> Self {
        Self {
            frames: 64,
            keyframes: None,
            select: Select::Spread,
            order: Order::Time,
            morph: Morph::Crossfade,
            align: true,
            smooth: 0,
            normalize: Normalize::Table,
            peak: 0.99,
        }
    }
}

/// A finished table, with the cycle behind each keyframe.
#[derive(Clone, Debug)]
pub struct Table {
    pub frames: Vec<Vec<f64>>,
    /// The keyframes' cycles, in table order, with where each landed.
    pub keyframes: Vec<(usize, Cycle)>,
}

const FEATURE_HARMONICS: usize = 64;

/// Log-magnitude spectrum of the first harmonics, level-independent.
fn features(fft: &mut Fft, frame: &[f64]) -> Vec<f64> {
    let spec = fft.spectrum(frame);
    let mags: Vec<f64> = (1..=FEATURE_HARMONICS.min(frame.len() / 2 - 1))
        .map(|h| spec[h].norm())
        .collect();
    let total = mags.iter().map(|m| m * m).sum::<f64>().sqrt().max(1e-12);
    mags.iter().map(|m| (m / total + 1e-4).log10()).collect()
}

fn centroid(fft: &mut Fft, frame: &[f64]) -> f64 {
    let spec = fft.spectrum(frame);
    let (mut num, mut den) = (0.0, 0.0);
    for (h, c) in spec.iter().enumerate().take(frame.len() / 2).skip(1) {
        let p = c.norm_sqr();
        num += h as f64 * p;
        den += p;
    }
    if den > 0.0 {
        num / den
    } else {
        0.0
    }
}

fn distance(a: &[f64], b: &[f64]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).powi(2))
        .sum::<f64>()
        .sqrt()
}

/// Pick `n` of the cycles (indices into `cycles`, which are in clip and time order).
fn select(fft: &mut Fft, cycles: &[Cycle], n: usize, how: Select) -> Vec<usize> {
    let len = cycles.len();
    if n >= len {
        return (0..len).collect();
    }
    match how {
        Select::Spread => {
            if n == 1 {
                return vec![len / 2];
            }
            (0..n)
                .map(|i| (i * (len - 1) + (n - 1) / 2) / (n - 1))
                .collect()
        }
        Select::Best => {
            let mut idx: Vec<usize> = (0..len).collect();
            idx.sort_by(|&a, &b| cycles[b].score.total_cmp(&cycles[a].score));
            idx.truncate(n);
            idx.sort();
            idx
        }
        Select::Diverse => {
            let feats: Vec<Vec<f64>> = cycles.iter().map(|c| features(fft, &c.frame)).collect();
            let first = (0..len)
                .max_by(|&a, &b| cycles[a].score.total_cmp(&cycles[b].score))
                .unwrap();
            let mut chosen = vec![first];
            let mut near: Vec<f64> = feats.iter().map(|f| distance(f, &feats[first])).collect();
            while chosen.len() < n {
                let next = (0..len)
                    .max_by(|&a, &b| near[a].total_cmp(&near[b]))
                    .unwrap();
                chosen.push(next);
                for i in 0..len {
                    near[i] = near[i].min(distance(&feats[i], &feats[next]));
                }
            }
            chosen.sort();
            chosen
        }
    }
}

fn order(fft: &mut Fft, cycles: &[Cycle], mut idx: Vec<usize>, how: Order) -> Vec<usize> {
    match how {
        Order::Time => idx,
        Order::Brightness => {
            let mut c: Vec<(usize, f64)> = idx
                .iter()
                .map(|&i| (i, centroid(fft, &cycles[i].frame)))
                .collect();
            c.sort_by(|a, b| a.1.total_cmp(&b.1));
            c.into_iter().map(|(i, _)| i).collect()
        }
        Order::Path => {
            if idx.len() < 3 {
                return order(fft, cycles, idx, Order::Brightness);
            }
            let feats: Vec<Vec<f64>> = idx
                .iter()
                .map(|&i| features(fft, &cycles[i].frame))
                .collect();
            let cents: Vec<f64> = idx
                .iter()
                .map(|&i| centroid(fft, &cycles[i].frame))
                .collect();
            let n = idx.len();
            let start = (0..n)
                .min_by(|&a, &b| cents[a].total_cmp(&cents[b]))
                .unwrap();
            // Greedy nearest neighbour, then 2-opt to untangle crossings.
            let mut path = vec![start];
            let mut used = vec![false; n];
            used[start] = true;
            while path.len() < n {
                let last = *path.last().unwrap();
                let next = (0..n)
                    .filter(|&j| !used[j])
                    .min_by(|&a, &b| {
                        distance(&feats[last], &feats[a])
                            .total_cmp(&distance(&feats[last], &feats[b]))
                    })
                    .unwrap();
                used[next] = true;
                path.push(next);
            }
            let mut improved = true;
            let mut rounds = 0;
            while improved && rounds < 50 {
                improved = false;
                rounds += 1;
                for i in 0..n - 2 {
                    for j in i + 2..n {
                        // Reverse path[i+1..=j]: edges (i,i+1),(j,j+1) become (i,j),(i+1,j+1).
                        let dist = |p: usize, q: usize| distance(&feats[path[p]], &feats[path[q]]);
                        let before = dist(i, i + 1) + if j + 1 < n { dist(j, j + 1) } else { 0.0 };
                        let after = dist(i, j) + if j + 1 < n { dist(i + 1, j + 1) } else { 0.0 };
                        if after + 1e-12 < before {
                            path[i + 1..=j].reverse();
                            improved = true;
                        }
                    }
                }
            }
            idx = path.into_iter().map(|p| idx[p]).collect();
            idx
        }
    }
}

/// Circularly shift a frame by a fractional number of samples (positive = later).
fn rotate(fft: &mut Fft, frame: &[f64], shift: f64) -> Vec<f64> {
    let n = frame.len();
    let mut spec = fft.spectrum(frame);
    for (k, c) in spec.iter_mut().enumerate() {
        let h = if k <= n / 2 {
            k as f64
        } else {
            k as f64 - n as f64
        };
        if 2 * k == n {
            continue; // Nyquist: leave as is (it is real)
        }
        *c *= Complex64::from_polar(1.0, -2.0 * PI * h * shift / n as f64);
    }
    fft.real(spec)
}

/// The circular shift (fractional samples) that best lines `b` up with `a`.
fn best_lag(fft: &mut Fft, a: &[f64], b: &[f64]) -> f64 {
    let n = a.len();
    let sa = fft.spectrum(a);
    let sb = fft.spectrum(b);
    let cross: Vec<Complex64> = sa.iter().zip(&sb).map(|(p, q)| p * q.conj()).collect();
    // Coarse: the peak of the circular cross-correlation.
    let x = fft.real(cross.clone());
    let k = (0..n).max_by(|&i, &j| x[i].total_cmp(&x[j])).unwrap();
    // Fine: Newton steps on the correlation as a sum of harmonics,
    // r(tau) = sum_h |P_h| cos(arg P_h + 2 pi h tau / n), exact for a pure rotation.
    let w = 2.0 * PI / n as f64;
    let mut tau = k as f64;
    for _ in 0..8 {
        let (mut d1, mut d2) = (0.0, 0.0);
        for (h, p) in cross.iter().enumerate().take(n.div_ceil(2)).skip(1) {
            let hw = h as f64 * w;
            let ph = p.arg() + hw * tau;
            d1 -= p.norm() * hw * ph.sin();
            d2 -= p.norm() * hw * hw * ph.cos();
        }
        if d2 >= 0.0 {
            break; // not at a maximum: keep the coarse peak
        }
        let step = (-d1 / d2).clamp(-1.0, 1.0);
        tau += step;
        if step.abs() < 1e-10 {
            break;
        }
    }
    if tau > n as f64 / 2.0 {
        tau - n as f64
    } else {
        tau
    }
}

fn morph(fft: &mut Fft, a: &[f64], b: &[f64], t: f64, how: Morph) -> Vec<f64> {
    match how {
        Morph::Crossfade => a.iter().zip(b).map(|(x, y)| x + (y - x) * t).collect(),
        Morph::Spectral => {
            let sa = fft.spectrum(a);
            let sb = fft.spectrum(b);
            let spec = sa
                .iter()
                .zip(&sb)
                .map(|(p, q)| {
                    let mag = p.norm() + (q.norm() - p.norm()) * t;
                    let dir = p.unscale(p.norm().max(1e-300)) * (1.0 - t)
                        + q.unscale(q.norm().max(1e-300)) * t;
                    if dir.norm() < 1e-12 {
                        // Opposite phases at the midpoint: take the nearer keyframe's.
                        Complex64::from_polar(mag, if t < 0.5 { p.arg() } else { q.arg() })
                    } else {
                        Complex64::from_polar(mag, dir.arg())
                    }
                })
                .collect();
            fft.real(spec)
        }
    }
}

/// Build the table from every clip's cycles (in clip, then time, order).
pub fn build(cycles: &[Cycle], s: &TableSettings) -> Option<Table> {
    if cycles.is_empty() || s.frames == 0 {
        return None;
    }
    let mut fft = Fft::new();
    let want = s.keyframes.unwrap_or(s.frames).clamp(1, s.frames);
    let picked = select(&mut fft, cycles, want, s.select);
    let picked = order(&mut fft, cycles, picked, s.order);

    let mut keys: Vec<Vec<f64>> = picked.iter().map(|&i| cycles[i].frame.clone()).collect();
    if s.align {
        for k in 1..keys.len() {
            let lag = best_lag(&mut fft, &keys[k - 1], &keys[k]);
            keys[k] = rotate(&mut fft, &keys[k], lag);
        }
    }

    let nk = keys.len();
    let mut frames: Vec<Vec<f64>> = if nk == s.frames {
        keys.clone()
    } else if nk == 1 {
        vec![keys[0].clone(); s.frames]
    } else {
        (0..s.frames)
            .map(|i| {
                let pos = i as f64 * (nk - 1) as f64 / (s.frames - 1).max(1) as f64;
                let k = (pos.floor() as usize).min(nk - 2);
                let t = pos - k as f64;
                if t < 1e-9 {
                    keys[k].clone()
                } else if t > 1.0 - 1e-9 {
                    keys[k + 1].clone()
                } else {
                    morph(&mut fft, &keys[k], &keys[k + 1], t, s.morph)
                }
            })
            .collect()
    };
    let positions: Vec<usize> = (0..nk)
        .map(|k| {
            if nk == 1 {
                0
            } else {
                (k as f64 * (s.frames - 1) as f64 / (nk - 1) as f64).round() as usize
            }
        })
        .collect();

    if s.smooth > 0 {
        let orig = frames.clone();
        for (i, f) in frames.iter_mut().enumerate() {
            let lo = i.saturating_sub(s.smooth);
            let hi = (i + s.smooth + 1).min(orig.len());
            for (j, v) in f.iter_mut().enumerate() {
                *v = orig[lo..hi].iter().map(|o| o[j]).sum::<f64>() / (hi - lo) as f64;
            }
        }
    }

    let peak_of = |f: &[f64]| f.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    match s.normalize {
        Normalize::Table => {
            let p = frames.iter().map(|f| peak_of(f)).fold(0.0, f64::max);
            if p > 0.0 {
                frames.iter_mut().flatten().for_each(|v| *v *= s.peak / p);
            }
        }
        Normalize::Frame => {
            for f in frames.iter_mut() {
                let p = peak_of(f);
                if p > 0.0 {
                    f.iter_mut().for_each(|v| *v *= s.peak / p);
                }
            }
        }
        Normalize::None => {}
    }

    let keyframes = positions
        .into_iter()
        .zip(picked.iter().map(|&i| cycles[i].clone()))
        .collect();
    Some(Table { frames, keyframes })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cycle(clip: usize, time: f64, frame: Vec<f64>) -> Cycle {
        Cycle {
            clip,
            time,
            f0: 100.0,
            score: 0.99,
            periodicity: 0.99,
            rms: 0.5,
            band_limit: 22050.0,
            frame,
        }
    }

    fn harmonics(n: usize, amps: &[f64], phase: f64) -> Vec<f64> {
        (0..n)
            .map(|i| {
                let t = 2.0 * PI * i as f64 / n as f64 + phase;
                amps.iter()
                    .enumerate()
                    .map(|(h, a)| a * ((h + 1) as f64 * t).sin())
                    .sum()
            })
            .collect()
    }

    #[test]
    fn alignment_undoes_rotation() {
        let a = harmonics(256, &[1.0, 0.5, 0.3], 0.0);
        let b = harmonics(256, &[1.0, 0.5, 0.3], 1.1);
        let cycles = vec![cycle(0, 0.0, a.clone()), cycle(0, 1.0, b)];
        let s = TableSettings {
            frames: 2,
            normalize: Normalize::None,
            ..Default::default()
        };
        let t = build(&cycles, &s).unwrap();
        for (x, y) in t.frames[1].iter().zip(&a) {
            assert!((x - y).abs() < 1e-6);
        }
    }

    #[test]
    fn fills_between_keyframes_and_normalises() {
        let cycles: Vec<Cycle> = (0..3)
            .map(|i| cycle(0, i as f64, harmonics(128, &[1.0, i as f64 * 0.4], 0.0)))
            .collect();
        for morph in [Morph::Crossfade, Morph::Spectral] {
            let s = TableSettings {
                frames: 9,
                morph,
                ..Default::default()
            };
            let t = build(&cycles, &s).unwrap();
            assert_eq!(t.frames.len(), 9);
            assert_eq!(
                t.keyframes.iter().map(|k| k.0).collect::<Vec<_>>(),
                vec![0, 4, 8]
            );
            let peak = t
                .frames
                .iter()
                .flatten()
                .fold(0.0f64, |m, v| m.max(v.abs()));
            assert!((peak - 0.99).abs() < 1e-9);
            // Midway frame has half the second harmonic of keyframe 1 and 2.
            let spec = Fft::new().spectrum(&t.frames[6]);
            let ratio = spec[2].norm() / spec[1].norm();
            assert!((ratio - 0.6).abs() < 0.02, "{morph:?}: {ratio}");
        }
    }

    #[test]
    fn path_order_keeps_similar_frames_together() {
        // Brightness steps that arrive shuffled in time.
        let steps = [0.0, 0.8, 0.2, 0.6, 0.4, 1.0];
        let cycles: Vec<Cycle> = steps
            .iter()
            .enumerate()
            .map(|(i, &b)| cycle(0, i as f64, harmonics(128, &[1.0, b, b * b], 0.0)))
            .collect();
        for order in [Order::Path, Order::Brightness] {
            let s = TableSettings {
                frames: 6,
                order,
                ..Default::default()
            };
            let t = build(&cycles, &s).unwrap();
            let times: Vec<f64> = t.keyframes.iter().map(|k| k.1.time).collect();
            assert_eq!(times, vec![0.0, 2.0, 4.0, 3.0, 1.0, 5.0], "{order:?}");
        }
    }

    #[test]
    fn spread_reaches_both_ends() {
        let cycles: Vec<Cycle> = (0..100)
            .map(|i| cycle(i / 50, i as f64, harmonics(64, &[1.0], 0.0)))
            .collect();
        let picked = select(&mut Fft::new(), &cycles, 5, Select::Spread);
        assert_eq!(picked.first(), Some(&0));
        assert_eq!(picked.last(), Some(&99));
        assert_eq!(picked.len(), 5);
    }
}

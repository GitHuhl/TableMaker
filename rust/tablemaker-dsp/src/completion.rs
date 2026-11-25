//! Completing what the recordings left out.
//!
//! *Formant completion*: where a vowel's higher formants were not measured
//! (weak resonances, breathy voice, or above the recording's band), predict
//! them from the speaker's other vowels: F3 and F4 depend mostly on the
//! length of the vocal tract, which is the same for every vowel. Then look
//! for a real peak in that vowel's envelope near the prediction ("found");
//! only if there is none is the prediction kept ("inferred").
//!
//! *Restoration* builds the envelopes the instrument plays: a resonance for
//! every inferred formant, and above the recording's band edge (where a
//! low-rate recording holds nothing) the speaker's own high-frequency slope
//! continued, with the vocal tract's higher resonances on it.

use crate::envelope::{Envelope, FLOOR_DB, POINTS, STEP_HZ};
use crate::lpc::Formant;
use crate::speaker::{SpeakerProfile, VoiceType, VowelEntry};
use serde::{Deserialize, Serialize};

/// Formant slots completed: F1 to F4.
pub const SLOTS: usize = 4;
/// Speed of sound in the vocal tract (cm/s).
const SOUND: f64 = 35000.0;

/// How a formant was obtained.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FormantKind {
    /// Tracked in most of the vowel's cycles.
    #[default]
    Measured,
    /// A peak in the vowel's envelope where the speaker's other vowels put it.
    Found,
    /// Predicted from the speaker's other vowels; not in the recording.
    Inferred,
}

/// Vocal tract length (cm) of a uniform tube with these formants
/// (slot k at (2k + 1) c / 4L).
fn tube_length(slot: usize, freq: f64) -> f64 {
    (2 * slot + 1) as f64 * SOUND / (4.0 * freq)
}

/// Formant `slot` (0 = F1) of a uniform tube `length` cm long.
pub fn tube_formant(slot: usize, length: f64) -> f64 {
    (2 * slot + 1) as f64 * SOUND / (4.0 * length)
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    Some(v[v.len() / 2])
}

/// The speaker's vocal tract length: from measured F3/F4 if any (they say
/// most about it), else from every measured formant, else the voice type's.
pub fn speaker_tract(vowels: &[VowelEntry], voice: VoiceType) -> f64 {
    let measured = |min_slot: usize| -> Vec<f64> {
        vowels
            .iter()
            .flat_map(|v| {
                v.formants
                    .iter()
                    .zip(&v.formant_kinds)
                    .enumerate()
                    .filter(move |(k, (_, kind))| *k >= min_slot && **kind == FormantKind::Measured)
                    .map(|(k, (f, _))| tube_length(k, f.freq))
            })
            .collect()
    };
    median(measured(2))
        .or_else(|| median(measured(0)))
        .unwrap_or(match voice {
            VoiceType::Man => 17.5,
            VoiceType::Woman => 14.5,
            VoiceType::Child => 11.5,
            VoiceType::Auto => 16.0,
        })
}

/// Least squares y = b0 + b1 x1 + ... (ridge-stabilised); None if too few points.
fn regress(xs: &[Vec<f64>], ys: &[f64]) -> Option<Vec<f64>> {
    let p = xs.first()?.len() + 1;
    if ys.len() < p + 2 {
        return None;
    }
    // Normal equations A b = c.
    let mut a = vec![vec![0.0; p]; p];
    let mut c = vec![0.0; p];
    for (x, &y) in xs.iter().zip(ys) {
        let row: Vec<f64> = std::iter::once(1.0).chain(x.iter().copied()).collect();
        for i in 0..p {
            c[i] += row[i] * y;
            for j in 0..p {
                a[i][j] += row[i] * row[j];
            }
        }
    }
    for (i, r) in a.iter_mut().enumerate().skip(1) {
        r[i] += 1e-3;
    }
    // Gaussian elimination with partial pivoting.
    for col in 0..p {
        let piv = (col..p).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        c.swap(col, piv);
        for r in col + 1..p {
            let f = a[r][col] / a[col][col];
            let pivot_row = a[col].clone();
            for (x, y) in a[r].iter_mut().zip(&pivot_row).skip(col) {
                *x -= f * y;
            }
            c[r] -= f * c[col];
        }
    }
    let mut b = vec![0.0; p];
    for i in (0..p).rev() {
        b[i] = (c[i] - (i + 1..p).map(|k| a[i][k] * b[k]).sum::<f64>()) / a[i][i];
    }
    Some(b)
}

/// Where slot `slot` should be for a vowel with these lower formants.
fn predict(vowels: &[VowelEntry], lower: &[Formant], slot: usize, tract: f64) -> f64 {
    let preds = slot.min(2); // F1 and F2 say which vowel it is
    let complete = |v: &&VowelEntry| {
        v.formants.len() > slot
            && v.formant_kinds[..=slot]
                .iter()
                .all(|k| *k == FormantKind::Measured)
    };
    let training: Vec<&VowelEntry> = vowels.iter().filter(complete).collect();
    let xs: Vec<Vec<f64>> = training
        .iter()
        .map(|v| (0..preds).map(|k| v.formants[k].freq.ln()).collect())
        .collect();
    let ys: Vec<f64> = training
        .iter()
        .map(|v| v.formants[slot].freq.ln())
        .collect();
    let guess = if let Some(b) = regress(&xs, &ys) {
        (b[0]
            + (0..preds)
                .map(|k| b[k + 1] * lower[k].freq.ln())
                .sum::<f64>())
        .exp()
    } else if let Some(m) = median(ys.clone()) {
        m.exp()
    } else {
        tube_formant(slot, tract)
    };
    // Formants keep their order with some room between them.
    let floor = lower.last().map_or(0.0, |f| f.freq + 250.0);
    guess.max(floor)
}

/// The highest frequency the envelope holds (Hz).
pub fn band_edge(env: &Envelope) -> f64 {
    let i = env
        .db
        .iter()
        .rposition(|&v| v as f64 > FLOOR_DB + 3.0)
        .unwrap_or(0);
    i as f64 * STEP_HZ
}

/// Peaks of the envelope within [lo, hi]: frequency, -3 dB bandwidth and
/// prominence (dB). The local slope is taken out first, so a formant that
/// only shows as a shoulder on the falling spectrum counts too.
fn peaks(env: &Envelope, lo: f64, hi: f64) -> Vec<(Formant, f64)> {
    let (a, b) = (
        (lo / STEP_HZ).ceil().max(1.0) as usize,
        ((hi / STEP_HZ).floor() as usize).min(POINTS - 2),
    );
    if a + 2 > b {
        return Vec::new();
    }
    let span = (400.0 / STEP_HZ) as usize;
    let (fa, fb) = (a.saturating_sub(span).max(1), (b + span).min(POINTS - 2));
    // Least-squares line over the window and its margins.
    let n = (fb - fa + 1) as f64;
    let mx = (fa..=fb).map(|i| i as f64).sum::<f64>() / n;
    let my = (fa..=fb).map(|i| env.db[i] as f64).sum::<f64>() / n;
    let sxx: f64 = (fa..=fb).map(|i| (i as f64 - mx).powi(2)).sum();
    let sxy: f64 = (fa..=fb)
        .map(|i| (i as f64 - mx) * (env.db[i] as f64 - my))
        .sum();
    let slope = if sxx > 0.0 { sxy / sxx } else { 0.0 };
    let db = |i: usize| env.db[i] as f64 - (my + slope * (i as f64 - mx));
    let mut out = Vec::new();
    for i in a..=b {
        if !(db(i) > db(i - 1) && db(i) >= db(i + 1)) {
            continue;
        }
        let left = (i.saturating_sub(span).max(fa)..i)
            .map(db)
            .fold(f64::INFINITY, f64::min);
        let right = (i + 1..=(i + span).min(fb))
            .map(db)
            .fold(f64::INFINITY, f64::min);
        let prom = db(i) - left.max(right);
        if prom < 1.5 {
            continue;
        }
        let (l, m, r) = (db(i - 1), db(i), db(i + 1));
        let den = l - 2.0 * m + r;
        let shift = if den.abs() > 1e-9 {
            (0.5 * (l - r) / den).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        let half = |dir: isize| {
            let mut k = i as isize;
            while k > fa as isize
                && (k as usize) < fb
                && db(k as usize) > m - 3.0
                && (k - i as isize).abs() < 30
            {
                k += dir;
            }
            (k - i as isize).unsigned_abs() as f64 * STEP_HZ
        };
        out.push((
            Formant {
                freq: (i as f64 + shift) * STEP_HZ,
                bw: (half(-1) + half(1)).clamp(40.0, 500.0),
            },
            prom,
        ));
    }
    out
}

/// Where each formant can be at all, for an adult male tract (Hz); scaled by tract length.
const RANGE: [(f64, f64); SLOTS] = [
    (200.0, 1100.0),
    (500.0, 2900.0),
    (1600.0, 3800.0),
    (2800.0, 5000.0),
];

/// The most prominent peak in the slot's plausible range, preferring ones
/// near the prediction.
fn search(env: &Envelope, below: f64, slot: usize, pred: f64, tract: f64) -> Option<Formant> {
    let scale = 17.5 / tract;
    let lo = (RANGE[slot].0 * scale).max(below + 150.0);
    let hi = (RANGE[slot].1 * scale).min(band_edge(env) * 0.97);
    if lo >= hi {
        return None;
    }
    peaks(env, lo, hi)
        .into_iter()
        .map(|(f, prom)| (prom - 8.0 * (f.freq / pred).ln().abs(), f))
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, f)| f)
}

/// Complete every vowel's formants up to F4 (see the module comment).
pub fn complete(vowels: &mut [VowelEntry], voice: VoiceType) {
    for v in vowels.iter_mut() {
        v.formant_kinds
            .resize(v.formants.len(), FormantKind::Measured);
    }
    let tract = speaker_tract(vowels, voice);
    for slot in 1..SLOTS {
        let typical_bw = median(
            vowels
                .iter()
                .filter(|v| v.formant_kinds.get(slot) == Some(&FormantKind::Measured))
                .map(|v| v.formants[slot].bw)
                .collect(),
        );
        let snapshot: Vec<VowelEntry> = vowels.to_vec();
        for v in vowels.iter_mut() {
            if v.formants.len() != slot {
                continue; // complete already, or not even F1 to go on
            }
            let pred = predict(&snapshot, &v.formants, slot, tract);
            let (f, kind) = match search(&v.envelope, v.formants[slot - 1].freq, slot, pred, tract)
            {
                Some(f) => (f, FormantKind::Found),
                None => (
                    Formant {
                        freq: pred,
                        bw: typical_bw.unwrap_or(80.0 + 0.06 * pred),
                    },
                    FormantKind::Inferred,
                ),
            };
            v.formants.push(f);
            v.formant_kinds.push(kind);
        }
    }
}

/// How far a formant stands above its surroundings (dB).
fn prominence(env: &Envelope, f: f64) -> f64 {
    env.at(f) - 0.5 * (env.at((f - 450.0).max(STEP_HZ)) + env.at(f + 450.0))
}

/// Lorentzian resonance shape, 1 at the centre, 0.5 at ± bw/2.
fn bump(f: f64, centre: f64, bw: f64) -> f64 {
    let x = (f - centre) / (bw / 2.0).max(1.0);
    1.0 / (1.0 + x * x)
}

/// The envelopes to play, one per vowel: as recorded plus a resonance for
/// every inferred formant, and continued above the recording's band edge.
pub fn restored_envelopes(p: &SpeakerProfile) -> Vec<Envelope> {
    // The speaker's typical formant prominence, per slot.
    let prom: Vec<f64> = (0..SLOTS)
        .map(|k| {
            median(
                p.vowels
                    .iter()
                    .filter(|v| v.formant_kinds.get(k) == Some(&FormantKind::Measured))
                    .map(|v| prominence(&v.envelope, v.formants[k].freq))
                    .collect(),
            )
            .unwrap_or(8.0)
            .clamp(3.0, 15.0)
        })
        .collect();
    let tract = speaker_tract(&p.vowels, p.voice);
    p.vowels.iter().map(|v| restore(v, &prom, tract)).collect()
}

fn restore(v: &VowelEntry, prom: &[f64], tract: f64) -> Envelope {
    let top = (POINTS - 1) as f64 * STEP_HZ;
    let edge = band_edge(&v.envelope);
    let mut db: Vec<f64> = v.envelope.db.iter().map(|&x| x as f64).collect();
    let freq = |i: usize| i as f64 * STEP_HZ;
    // Above the band (if the recording stops short): the slope of the top octave, continued.
    let x0 = edge * 0.9;
    let extend = edge < top * 0.97 && edge > 1000.0;
    if extend {
        let fit: Vec<(f64, f64)> = (0..POINTS)
            .filter(|&i| freq(i) >= edge * 0.45 && freq(i) <= x0)
            .map(|i| ((freq(i) / x0).log2(), db[i]))
            .collect();
        let n = fit.len() as f64;
        let (mx, my) = (
            fit.iter().map(|p| p.0).sum::<f64>() / n,
            fit.iter().map(|p| p.1).sum::<f64>() / n,
        );
        let sxx: f64 = fit.iter().map(|p| (p.0 - mx).powi(2)).sum();
        let sxy: f64 = fit.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
        let slope = if sxx > 0.0 {
            (sxy / sxx).clamp(-18.0, -3.0)
        } else {
            -9.0
        };
        let base = my - slope * mx; // the fitted level at x0
        for (i, d) in db.iter_mut().enumerate() {
            let f = freq(i);
            if f > x0 {
                // Blend over the first tenth of an octave so there is no step.
                let t = ((f / x0).log2() / 0.15).min(1.0);
                let ext = base + slope * (f / x0).log2();
                *d = *d * (1.0 - t) + ext * t;
            }
        }
        // The vocal tract's higher resonances, gentler than the measured ones.
        let mut slot = SLOTS;
        loop {
            let f = tube_formant(slot, tract);
            if f >= top {
                break;
            }
            if f > x0 {
                let bw = 150.0 + 0.05 * f;
                for (i, d) in db.iter_mut().enumerate() {
                    if freq(i) > x0 {
                        *d += 4.0 * bump(freq(i), f, bw);
                    }
                }
            }
            slot += 1;
        }
    }
    // A resonance for every formant the recording does not show.
    for (k, (f, kind)) in v.formants.iter().zip(&v.formant_kinds).enumerate() {
        if *kind == FormantKind::Inferred {
            let p = prom.get(k).copied().unwrap_or(8.0);
            for (i, d) in db.iter_mut().enumerate() {
                *d += p * bump(freq(i), f.freq, f.bw);
            }
        }
    }
    let peak = db.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    Envelope {
        db: db.iter().map(|d| (d - peak).max(FLOOR_DB) as f32).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speaker::{analyze, SpeakerSettings};
    use crate::testsignal::Voice;
    use crate::wav::Audio;

    const AH: [(f64, f64); 4] = [
        (770.0, 90.0),
        (1330.0, 110.0),
        (2440.0, 160.0),
        (3400.0, 250.0),
    ];
    const EE: [(f64, f64); 4] = [
        (300.0, 60.0),
        (2250.0, 120.0),
        (2950.0, 170.0),
        (3600.0, 250.0),
    ];
    const OO: [(f64, f64); 4] = [
        (320.0, 60.0),
        (870.0, 90.0),
        (2250.0, 150.0),
        (3300.0, 250.0),
    ];

    fn clip(name: &str, f: &[(f64, f64)], sr: u32, seed: u64) -> (String, Audio) {
        let v = Voice {
            f0: 120.0,
            formants: f.to_vec(),
            jitter: 0.003,
            seed,
            ..Default::default()
        };
        (
            name.to_string(),
            Audio {
                sample_rate: sr,
                samples: v.render(sr, 1.5),
            },
        )
    }

    fn by_file<'a>(p: &'a SpeakerProfile, file: &str) -> &'a VowelEntry {
        p.vowels.iter().find(|v| v.sources[0].file == file).unwrap()
    }

    #[test]
    fn finds_formants_that_were_dropped() {
        let clips = vec![
            clip("ah", &AH, 44100, 1),
            clip("ee", &EE, 44100, 2),
            clip("oo", &OO, 44100, 3),
        ];
        let mut p = analyze(&clips, &SpeakerSettings::default(), &|_, _| {}).unwrap();
        let i = p
            .vowels
            .iter()
            .position(|v| v.sources[0].file == "ah")
            .unwrap();
        assert_eq!(p.vowels[i].formants.len(), 4);
        assert!(p.vowels[i]
            .formant_kinds
            .iter()
            .all(|k| *k == FormantKind::Measured));
        let truth = p.vowels[i].formants.clone();

        // Lose F3 and F4, as a weak or breathy vowel would.
        p.vowels[i].formants.truncate(2);
        p.vowels[i].formant_kinds.truncate(2);
        complete(&mut p.vowels, p.voice);
        let v = &p.vowels[i];
        assert_eq!(
            v.formant_kinds[2..],
            [FormantKind::Found, FormantKind::Found],
            "{:?}",
            v.formants
        );
        #[allow(clippy::needless_range_loop)]
        for k in 2..4 {
            assert!(
                (v.formants[k].freq / truth[k].freq - 1.0).abs() < 0.05,
                "F{}: {:?} vs {:?}",
                k + 1,
                v.formants[k],
                truth[k]
            );
        }
    }

    #[test]
    fn infers_formants_above_a_low_rate_recording_and_restores_them() {
        // "ah" recorded at 6 kHz holds nothing above 3 kHz: its F4 (3400 Hz) is not there.
        let clips = vec![
            clip("ah", &AH, 6000, 1),
            clip("ee", &EE, 44100, 2),
            clip("oo", &OO, 44100, 3),
        ];
        let p = analyze(&clips, &SpeakerSettings::default(), &|_, _| {}).unwrap();
        let ah = by_file(&p, "ah");
        assert_eq!(ah.formants.len(), 4, "{:?}", ah.formants);
        assert_eq!(
            ah.formant_kinds[3],
            FormantKind::Inferred,
            "{:?} {:?}",
            ah.formants,
            ah.formant_kinds
        );
        assert!(
            (ah.formants[3].freq / 3400.0 - 1.0).abs() < 0.15,
            "inferred F4 {:?}",
            ah.formants[3]
        );
        assert!(band_edge(&ah.envelope) < 3100.0);

        let restored = restored_envelopes(&p);
        let i = p
            .vowels
            .iter()
            .position(|v| v.sources[0].file == "ah")
            .unwrap();
        let r = &restored[i];
        // Continued above the band, with the inferred F4 standing out of the slope.
        assert!(band_edge(r) > 15000.0, "edge {}", band_edge(r));
        let f4 = ah.formants[3].freq;
        assert!(
            r.at(f4) > r.at(f4 - 400.0) + 2.0 && r.at(f4) > r.at(f4 + 400.0) + 2.0,
            "no F4 peak"
        );
        // Below the band nothing changes but the overall level.
        let shift = r.at(770.0) - ah.envelope.at(770.0);
        assert!((r.at(1330.0) - ah.envelope.at(1330.0) - shift).abs() < 1.0);
        // Full-band vowels are left (nearly) as they were.
        let j = p
            .vowels
            .iter()
            .position(|v| v.sources[0].file == "ee")
            .unwrap();
        let d = (0..200)
            .map(|k| (restored[j].db[k] - p.vowels[j].envelope.db[k]).abs())
            .fold(0.0f32, f32::max);
        assert!(d < 1.0, "full-band vowel changed by {d} dB");
    }

    #[test]
    fn falls_back_to_the_tube_with_nothing_to_learn_from() {
        let v = VowelEntry {
            label: "ɑ".into(),
            formants: vec![
                Formant {
                    freq: 750.0,
                    bw: 90.0,
                },
                Formant {
                    freq: 1300.0,
                    bw: 110.0,
                },
            ],
            formant_kinds: vec![],
            f0: 120.0,
            count: 10,
            share: 1.0,
            hnr: 20.0,
            h1_h2: 0.0,
            envelope: Envelope {
                db: vec![FLOOR_DB as f32; POINTS],
            },
            sources: vec![],
        };
        let mut vs = vec![v];
        complete(&mut vs, VoiceType::Man);
        let f = &vs[0].formants;
        assert_eq!(f.len(), 4);
        assert!(f[1].freq < f[2].freq && f[2].freq < f[3].freq, "{f:?}");
        assert_eq!(
            vs[0].formant_kinds[2..],
            [FormantKind::Inferred, FormantKind::Inferred]
        );
    }

    #[test]
    fn regression_recovers_a_linear_law() {
        let xs: Vec<Vec<f64>> = (0..8)
            .map(|i| vec![i as f64, (i * i) as f64 * 0.1])
            .collect();
        let ys: Vec<f64> = xs.iter().map(|x| 2.0 + 0.5 * x[0] - 3.0 * x[1]).collect();
        let b = regress(&xs, &ys).unwrap();
        assert!(
            (b[0] - 2.0).abs() < 0.01 && (b[1] - 0.5).abs() < 0.01 && (b[2] + 3.0).abs() < 0.01,
            "{b:?}"
        );
    }
}

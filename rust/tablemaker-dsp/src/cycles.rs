//! Single-cycle extraction: oversample a clip, find its rising zero
//! crossings to a fraction of a sample, pair crossings one period apart,
//! score each pair on how periodic it is and how cleanly it loops, keep the
//! best non-overlapping ones, and render each to a fixed frame size.

use crate::pitch::{self, PitchSettings, PitchTrack};
use crate::signal::{self, Crossing, Fft};
use crate::wav::Audio;

#[derive(Clone, Debug)]
pub struct ExtractSettings {
    pub oversample: usize,
    pub frame_size: usize,
    /// Allowed deviation of a crossing pair from the tracked period (fraction).
    pub tolerance: f64,
    /// Lowest score kept (0..1).
    pub min_score: f64,
    /// Cycles quieter than this (dB below the clip's loudest cycle) are dropped.
    pub gate_db: f64,
    pub pitch: PitchSettings,
    /// Skip tracking and use this pitch everywhere.
    pub fixed_f0: Option<f64>,
}

impl Default for ExtractSettings {
    fn default() -> Self {
        Self {
            oversample: 16,
            frame_size: 2048,
            tolerance: 0.03,
            min_score: 0.9,
            gate_db: 40.0,
            pitch: PitchSettings::default(),
            fixed_f0: None,
        }
    }
}

/// One extracted cycle, rendered to the frame size.
#[derive(Clone, Debug)]
pub struct Cycle {
    pub clip: usize,
    /// Start of the cycle in the clip, in seconds.
    pub time: f64,
    /// The cycle's own fundamental, from its exact length.
    pub f0: f64,
    /// Periodicity (correlation with the following period) weighted by how
    /// well the slopes at its two ends match.
    pub score: f64,
    /// Correlation with the following period alone (1 = perfectly periodic);
    /// what is missing is noise or change, e.g. breath.
    pub periodicity: f64,
    pub rms: f64,
    /// Highest frequency the recording holds (half its sample rate), in Hz.
    pub band_limit: f64,
    /// The cycle at the frame size: bin h of its spectrum is harmonic h.
    pub frame: Vec<f64>,
}

/// What the extractor found in one clip.
#[derive(Clone, Debug)]
pub struct ClipAnalysis {
    pub sample_rate: u32,
    pub duration: f64,
    pub median_f0: Option<f64>,
    pub voiced_fraction: f64,
    pub crossings: usize,
    pub candidates: usize,
    pub cycles: Vec<Cycle>,
    pub pitch: PitchTrack,
}

struct Candidate {
    start: f64,
    len: f64,
    score: f64,
    periodicity: f64,
    rms: f64,
}

const SCORE_POINTS: usize = 256;
const DC_CUTOFF_HZ: f64 = 10.0;

/// Extract cycles from one clip.
pub fn extract(audio: &Audio, clip: usize, s: &ExtractSettings) -> ClipAnalysis {
    let sr = audio.sample_rate;
    let x = signal::dc_block(&audio.samples, sr, DC_CUTOFF_HZ);
    let track = match s.fixed_f0 {
        Some(f) => PitchTrack::fixed(f, sr, x.len()),
        None => pitch::track(&x, sr, &s.pitch),
    };
    let f = s.oversample.max(1);
    let xo = signal::oversample(&x, f);
    let crossings = signal::rising_crossings(&xo);
    let osr = sr as f64 * f as f64;

    // Near the clip's ends the DC blocker is still settling (three time constants) and the
    // oversampling filter runs off the edge: crossings there are slightly misplaced.
    let settle = 3.0 * sr as f64 / (2.0 * std::f64::consts::PI * DC_CUTOFF_HZ);
    let edge = settle.max(signal::OVERSAMPLE_ZEROS as f64) * f as f64;
    let mut cands = Vec::new();
    for (i, c) in crossings.iter().enumerate() {
        if c.pos < edge {
            continue;
        }
        let Some(f0) = track.at(c.pos / f as f64) else {
            continue;
        };
        let period = osr / f0;
        let Some(j) = nearest(&crossings[i + 1..], c.pos + period).map(|j| j + i + 1) else {
            continue;
        };
        let end = crossings[j];
        let len = end.pos - c.pos;
        if (len / period - 1.0).abs() > s.tolerance || end.pos + len + edge >= xo.len() as f64 {
            continue;
        }
        let (score, periodicity, rms) = score(&xo, c, &end, len);
        cands.push(Candidate {
            start: c.pos,
            len,
            score,
            periodicity,
            rms,
        });
    }
    let candidates = cands.len();

    // Best first, never overlapping an accepted cycle.
    cands.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut taken: Vec<(f64, f64)> = Vec::new(); // sorted by start
    let mut keep = Vec::new();
    for c in cands {
        if c.score < s.min_score {
            break;
        }
        let at = taken.partition_point(|t| t.0 < c.start);
        let clear_before = at == 0 || taken[at - 1].1 <= c.start;
        let clear_after = at == taken.len() || taken[at].0 >= c.start + c.len;
        if clear_before && clear_after {
            taken.insert(at, (c.start, c.start + c.len));
            keep.push(c);
        }
    }
    keep.sort_by(|a, b| a.start.total_cmp(&b.start));
    let loudest = keep.iter().map(|c| c.rms).fold(0.0, f64::max);
    let floor = loudest * 10f64.powf(-s.gate_db / 20.0);

    let mut fft = Fft::new();
    let cycles = keep
        .into_iter()
        .filter(|c| c.rms >= floor && c.rms > 0.0)
        .map(|c| Cycle {
            clip,
            time: c.start / osr,
            f0: osr / c.len,
            score: c.score,
            periodicity: c.periodicity,
            rms: c.rms,
            band_limit: sr as f64 / 2.0,
            frame: render(&mut fft, &xo, c.start, c.len, f, s.frame_size),
        })
        .collect();

    ClipAnalysis {
        sample_rate: sr,
        duration: audio.samples.len() as f64 / sr as f64,
        median_f0: track.median(),
        voiced_fraction: track.voiced_fraction(),
        crossings: crossings.len(),
        candidates,
        cycles,
        pitch: track,
    }
}

/// Index of the crossing nearest `target` (crossings are sorted).
fn nearest(c: &[Crossing], target: f64) -> Option<usize> {
    let k = c.partition_point(|x| x.pos < target);
    [k.checked_sub(1), (k < c.len()).then_some(k)]
        .into_iter()
        .flatten()
        .min_by(|&a, &b| {
            (c[a].pos - target)
                .abs()
                .total_cmp(&(c[b].pos - target).abs())
        })
}

/// Correlation of the cycle with the period that follows it, weighted by
/// the match of the slopes at its two ends (a mismatch clicks on loop).
fn score(xo: &[f64], start: &Crossing, end: &Crossing, len: f64) -> (f64, f64, f64) {
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for k in 0..SCORE_POINTS {
        let t = len * k as f64 / SCORE_POINTS as f64;
        let a = signal::linear(xo, start.pos + t);
        let b = signal::linear(xo, end.pos + t);
        ab += a * b;
        aa += a * a;
        bb += b * b;
    }
    let corr = if aa > 0.0 && bb > 0.0 {
        (ab / (aa * bb).sqrt()).max(0.0)
    } else {
        0.0
    };
    let (s0, s1) = (start.slope.abs(), end.slope.abs());
    let slope = if s0.max(s1) > 0.0 {
        s0.min(s1) / s0.max(s1)
    } else {
        0.0
    };
    (
        corr * slope.powf(0.25),
        corr,
        (aa / SCORE_POINTS as f64).sqrt(),
    )
}

/// Render the cycle [start, start + len) of the oversampled signal to
/// `size` samples: sampled at its native resolution with cubic
/// interpolation, then moved to the frame size in the frequency domain.
fn render(fft: &mut Fft, xo: &[f64], start: f64, len: f64, factor: usize, size: usize) -> Vec<f64> {
    let n = (len.ceil() as usize).max(8);
    let native: Vec<f64> = (0..n)
        .map(|k| signal::cubic(xo, start + len * k as f64 / n as f64))
        .collect();
    // Harmonics up to the original Nyquist frequency: len / factor samples per period at the
    // original rate hold len / (2 factor) harmonics.
    let max_h = (len / (2.0 * factor as f64)).floor() as usize;
    signal::resample_periodic(fft, &native, size, max_h.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn tone(f: f64, sr: u32, secs: f64, shape: impl Fn(f64) -> f64) -> Audio {
        let n = (secs * sr as f64) as usize;
        Audio {
            sample_rate: sr,
            samples: (0..n)
                .map(|i| shape(2.0 * PI * f * i as f64 / sr as f64))
                .collect(),
        }
    }

    #[test]
    fn extracts_whole_cycles_at_the_right_pitch() {
        // A non-integer period in samples, so the cycle boundaries fall between samples.
        let f = 217.3;
        let shape = |t: f64| 0.6 * t.sin() + 0.3 * (2.0 * t + 0.4).sin() + 0.1 * (5.0 * t).sin();
        let a = tone(f, 44100, 1.0, shape);
        let s = ExtractSettings {
            frame_size: 512,
            ..Default::default()
        };
        let r = extract(&a, 0, &s);
        assert!(r.cycles.len() > 150, "only {} cycles", r.cycles.len());
        for c in &r.cycles {
            assert!(
                (c.f0 / f - 1.0).abs() < 1e-4,
                "cycle f0 {} at {}",
                c.f0,
                c.time
            );
            assert!(c.score > 0.99);
        }
        // Every frame is the same waveform, rotated to start at a rising crossing.
        let first = &r.cycles[0].frame;
        for c in &r.cycles {
            let err: f64 = c
                .frame
                .iter()
                .zip(first)
                .map(|(a, b)| (a - b).powi(2))
                .sum::<f64>()
                / 512.0;
            assert!(err.sqrt() < 2e-3, "rms difference {}", err.sqrt());
        }
        // And it is that waveform: compare with the shape started at its own crossing.
        let frame = &r.cycles[0].frame;
        let best = (0..512)
            .map(|rot| {
                frame
                    .iter()
                    .enumerate()
                    .map(|(k, v)| (v - shape(2.0 * PI * ((k + rot) % 512) as f64 / 512.0)).powi(2))
                    .sum::<f64>()
            })
            .fold(f64::INFINITY, f64::min);
        // (Whole-sample rotations only, so up to half a sample off.)
        assert!((best / 512.0).sqrt() < 1.5e-2);
    }

    #[test]
    fn noise_gives_no_cycles() {
        let mut seed = 1u64;
        let samples = (0..44100)
            .map(|_| {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5
            })
            .collect();
        let r = extract(
            &Audio {
                sample_rate: 44100,
                samples,
            },
            0,
            &ExtractSettings::default(),
        );
        assert!(r.cycles.len() < 5, "{} cycles from noise", r.cycles.len());
    }
}

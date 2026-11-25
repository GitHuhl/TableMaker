//! Automatic analysis of one speaker across any number of recordings: their
//! pitch, the vowels and formant states they produce (the vowel table),
//! and the qualities of their voice (jitter, shimmer, breathiness, spectral
//! tilt, vibrato). The result is what the synthesizer is built from.

use crate::completion::{self, FormantKind};
use crate::cycles::{self, ExtractSettings};
use crate::envelope::Envelope;
use crate::formant::{self, FormantSettings};
use crate::lpc::Formant;
use crate::pitch::{PitchSettings, PitchTrack};
use crate::signal::Fft;
use crate::wav::Audio;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// Who is speaking: narrows the pitch search, sets how high formants are
/// looked for, and picks the reference vowels entries are named after.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VoiceType {
    /// Decide from the recordings.
    #[default]
    Auto,
    Man,
    Woman,
    Child,
}

impl VoiceType {
    /// Pitch search range (Hz), wide enough for singing as well as speech.
    pub fn pitch_range(self) -> (f64, f64) {
        match self {
            VoiceType::Auto => (50.0, 800.0),
            VoiceType::Man => (60.0, 500.0),
            VoiceType::Woman => (120.0, 1000.0),
            VoiceType::Child => (180.0, 1100.0),
        }
    }

    /// Highest formant to search for (Hz); for `Auto`, chosen from the median pitch.
    pub fn max_formant(self, median_f0: f64) -> f64 {
        match self {
            VoiceType::Man => 5000.0,
            VoiceType::Woman => 5500.0,
            VoiceType::Child => 8000.0,
            VoiceType::Auto if median_f0 < 165.0 => 5000.0,
            VoiceType::Auto if median_f0 < 300.0 => 5500.0,
            VoiceType::Auto => 6500.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SpeakerSettings {
    pub voice: VoiceType,
    /// Most vowel entries to keep (similar ones are merged, rare ones dropped).
    pub vowels: usize,
    /// Pitch search range (Hz); `None` takes the voice type's.
    pub fmin: Option<f64>,
    pub fmax: Option<f64>,
    /// Lowest cycle score kept: speech changes quickly, so lower than for held notes.
    pub min_score: f64,
    /// Highest formant to search for; `None` chooses from the speaker's pitch.
    pub max_formant: Option<f64>,
    /// Cycles kept per recording at most (neighbouring cycles are near copies).
    pub max_cycles_per_clip: usize,
    /// Worker threads (0 = one per core).
    pub threads: usize,
}

impl Default for SpeakerSettings {
    fn default() -> Self {
        Self {
            voice: VoiceType::Auto,
            vowels: 12,
            fmin: None,
            fmax: None,
            min_score: 0.8,
            max_formant: None,
            max_cycles_per_clip: 3000,
            threads: 0,
        }
    }
}

/// A recording position.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub file: String,
    pub time: f64,
}

/// One entry of the vowel table: a vowel or other voiced formant state the
/// speaker produced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VowelEntry {
    /// The nearest reference vowel (IPA), a guess.
    pub label: String,
    /// F1 to F4: measured (median over the entry's cycles), or completed
    /// (see `completion`).
    pub formants: Vec<Formant>,
    /// How each formant was obtained.
    #[serde(default)]
    pub formant_kinds: Vec<FormantKind>,
    /// Median pitch it was produced at (Hz).
    pub f0: f64,
    /// Cycles in this entry, and their share of all cycles.
    pub count: usize,
    pub share: f64,
    /// Harmonics-to-noise ratio (dB): lower is breathier.
    pub hnr: f64,
    /// H1-H2 (dB): spectral tilt at the bottom of the spectrum.
    pub h1_h2: f64,
    /// The timbre: mean spectral envelope of its cycles.
    pub envelope: Envelope,
    /// Where it is heard most typically.
    pub sources: Vec<Source>,
}

/// Pitch statistics (Hz).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PitchStats {
    pub median: f64,
    /// 5th and 95th percentiles: the comfortable range.
    pub low: f64,
    pub high: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Vibrato {
    pub rate: f64,
    /// Peak deviation (cents).
    pub extent: f64,
    /// Share of sustained voiced time it is present in.
    pub presence: f64,
}

/// What was found in one recording.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecordingSummary {
    pub file: String,
    pub duration: f64,
    pub voiced: f64,
    pub cycles: usize,
    pub median_f0: Option<f64>,
    /// Highest frequency the recording holds (half its sample rate), Hz.
    #[serde(default)]
    pub band: f64,
}

/// A point of the vowel space: one analysed cycle.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VowelPoint {
    pub f1: f64,
    pub f2: f64,
    pub entry: usize,
}

/// Everything known about the speaker.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpeakerProfile {
    pub version: u32,
    pub voice: VoiceType,
    pub recordings: Vec<RecordingSummary>,
    pub pitch: PitchStats,
    /// Mean period-to-period variation of length (%).
    pub jitter: f64,
    /// Mean period-to-period variation of amplitude (%).
    pub shimmer: f64,
    /// Median harmonics-to-noise ratio (dB).
    pub hnr: f64,
    /// Median H1-H2 (dB).
    pub h1_h2: f64,
    pub vibrato: Option<Vibrato>,
    /// Median level of voiced speech (dBFS).
    pub level: f64,
    /// Highest formant searched for (Hz).
    pub max_formant: f64,
    /// The vowel table, ordered so neighbours sound alike.
    pub vowels: Vec<VowelEntry>,
    /// A sample of the analysed cycles in the F1/F2 plane, for display.
    pub points: Vec<VowelPoint>,
}

impl SpeakerProfile {
    pub const VERSION: u32 = 1;

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("profile serialises")
    }

    pub fn from_json(s: &str) -> Result<Self, String> {
        let p: Self =
            serde_json::from_str(s).map_err(|e| format!("not a TableMaker voice: {e}"))?;
        if p.version != Self::VERSION {
            return Err(format!(
                "voice file version {} (this TableMaker reads {})",
                p.version,
                Self::VERSION
            ));
        }
        // Files from before formant completion: complete them now.
        let mut p = p;
        if p.vowels
            .iter()
            .any(|v| v.formant_kinds.len() != v.formants.len())
        {
            completion::complete(&mut p.vowels, p.voice);
        }
        Ok(p)
    }
}

/// One analysed cycle, compact.
struct CycleInfo {
    clip: usize,
    time: f64,
    f0: f64,
    periodicity: f64,
    rms: f64,
    formants: Vec<Formant>,
    envelope: Envelope,
    features: Vec<f64>,
}

struct ClipResult {
    summary: RecordingSummary,
    cycles: Vec<CycleInfo>,
    jitter: (f64, usize),
    shimmer: (f64, usize),
    vibrato: Vec<(f64, f64, f64, bool)>, // (seconds, rate, extent, present)
}

/// Analyse one speaker from named recordings. `progress` receives the
/// fraction done and what is happening.
pub fn analyze(
    clips: &[(String, Audio)],
    s: &SpeakerSettings,
    progress: &(dyn Fn(f64, &str) + Sync),
) -> Result<SpeakerProfile, String> {
    if clips.is_empty() {
        return Err("no recordings".into());
    }
    let threads = if s.threads == 0 {
        std::thread::available_parallelism().map_or(4, |n| n.get())
    } else {
        s.threads
    }
    .min(clips.len())
    .max(1);
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<ClipResult>>> =
        Mutex::new((0..clips.len()).map(|_| None).collect());
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= clips.len() {
                    break;
                }
                let r = analyze_clip(i, &clips[i].0, &clips[i].1, s);
                results.lock().unwrap()[i] = Some(r);
                let d = done.fetch_add(1, Ordering::SeqCst) + 1;
                progress(
                    0.9 * d as f64 / clips.len() as f64,
                    &format!("analysed {}", clips[i].0),
                );
            });
        }
    });
    let results: Vec<ClipResult> = results
        .into_inner()
        .unwrap()
        .into_iter()
        .map(|r| r.unwrap())
        .collect();
    progress(0.92, "building the vowel table");
    let profile = assemble(results, s)?;
    progress(1.0, "done");
    Ok(profile)
}

fn analyze_clip(index: usize, name: &str, audio: &Audio, s: &SpeakerSettings) -> ClipResult {
    let ex = ExtractSettings {
        frame_size: 1024,
        min_score: s.min_score,
        tolerance: 0.04,
        pitch: PitchSettings {
            fmin: s.fmin.unwrap_or(s.voice.pitch_range().0),
            fmax: s.fmax.unwrap_or(s.voice.pitch_range().1),
            ..Default::default()
        },
        ..Default::default()
    };
    let a = cycles::extract(audio, index, &ex);
    let max_formant = s
        .max_formant
        .unwrap_or_else(|| s.voice.max_formant(a.median_f0.unwrap_or(150.0)))
        // A low-rate recording has nothing above its band: do not look for formants there.
        .min(audio.sample_rate as f64 / 2.0 * 0.95);
    let ftrack = formant::track(
        &audio.samples,
        audio.sample_rate,
        &FormantSettings {
            max_formant,
            // About one formant per 1.1 kHz: asking for more than the band holds invents some.
            formants: ((max_formant / 1100.0).round() as usize).clamp(2, 5),
            ..Default::default()
        },
    );

    // Jitter and shimmer over runs of consecutive cycles.
    let (mut jit, mut jn, mut shim, mut sn) = (0.0, 0, 0.0, 0);
    for w in a.cycles.windows(2) {
        let (p, q) = (&w[0], &w[1]);
        let (tp, tq) = (1.0 / p.f0, 1.0 / q.f0);
        if ((q.time - p.time) / tp - 1.0).abs() < 0.1 {
            jit += (tp - tq).abs() / (0.5 * (tp + tq));
            jn += 1;
            shim += (p.rms - q.rms).abs() / (0.5 * (p.rms + q.rms));
            sn += 1;
        }
    }

    let stride = a.cycles.len().div_ceil(s.max_cycles_per_clip.max(1)).max(1);
    let mut fft = Fft::new();
    let cycles = a
        .cycles
        .iter()
        .step_by(stride)
        .map(|c| {
            let envelope = Envelope::from_cycle(&mut fft, &c.frame, c.f0, c.band_limit);
            let mut formants = ftrack.at(c.time + 0.5 / c.f0).to_vec();
            formants.truncate(4);
            CycleInfo {
                clip: index,
                time: c.time,
                f0: c.f0,
                periodicity: c.periodicity,
                rms: c.rms,
                formants,
                features: envelope.features(),
                envelope,
            }
        })
        .collect();

    ClipResult {
        summary: RecordingSummary {
            file: name.to_string(),
            duration: a.duration,
            voiced: a.voiced_fraction,
            cycles: a.cycles.len(),
            median_f0: a.median_f0,
            band: audio.sample_rate as f64 / 2.0,
        },
        cycles,
        jitter: (jit, jn),
        shimmer: (shim, sn),
        vibrato: vibrato_runs(&a.pitch),
    }
}

/// Vibrato in each sustained voiced stretch of a pitch track:
/// (length s, rate Hz, extent cents, present).
fn vibrato_runs(track: &PitchTrack) -> Vec<(f64, f64, f64, bool)> {
    let dt = track.hop as f64 / track.sample_rate as f64;
    let mut out = Vec::new();
    let mut run: Vec<f64> = Vec::new();
    let flush = |run: &mut Vec<f64>, out: &mut Vec<(f64, f64, f64, bool)>| {
        let n = run.len();
        let secs = n as f64 * dt;
        if secs >= 0.6 {
            let cents: Vec<f64> = run.iter().map(|f| 1200.0 * f.log2()).collect();
            // Remove the slow melody/intonation: subtract a 0.3 s moving average.
            let half = ((0.15 / dt) as usize).max(1);
            let resid: Vec<f64> = (0..n)
                .map(|i| {
                    let (lo, hi) = (i.saturating_sub(half), (i + half + 1).min(n));
                    cents[i] - cents[lo..hi].iter().sum::<f64>() / (hi - lo) as f64
                })
                .collect();
            let e0: f64 = resid.iter().map(|v| v * v).sum();
            let (lag_lo, lag_hi) = (
                ((1.0 / 9.0) / dt) as usize,
                ((1.0 / 3.5) / dt).ceil() as usize,
            );
            let mut best = (0.0, 0);
            for lag in lag_lo.max(1)..=lag_hi.min(n / 2) {
                let r: f64 =
                    (0..n - lag).map(|i| resid[i] * resid[i + lag]).sum::<f64>() / e0.max(1e-12);
                if r > best.0 {
                    best = (r, lag);
                }
            }
            let rms = (e0 / n as f64).sqrt();
            let present = best.0 > 0.35 && rms > 5.0;
            let rate = if best.1 > 0 {
                1.0 / (best.1 as f64 * dt)
            } else {
                0.0
            };
            out.push((secs, rate, rms * std::f64::consts::SQRT_2, present));
        }
        run.clear();
    };
    for f in &track.f0 {
        match f {
            Some(f) => run.push(*f),
            None => flush(&mut run, &mut out),
        }
    }
    flush(&mut run, &mut out);
    out
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn dist(a: &[f64], b: &[f64]) -> f64 {
    (a.iter().zip(b).map(|(x, y)| (x - y).powi(2)).sum::<f64>() / a.len() as f64).sqrt()
}

/// k-means++ (deterministic seeding) on feature vectors; returns assignments.
fn kmeans(feats: &[&[f64]], k: usize) -> Vec<usize> {
    let n = feats.len();
    let dim = feats[0].len();
    let mut seed = 0x2545F4914F6CDD1Du64;
    let mut rand = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut cents: Vec<Vec<f64>> = vec![feats[(rand() * n as f64) as usize % n].to_vec()];
    let mut d2: Vec<f64> = feats.iter().map(|f| dist(f, &cents[0]).powi(2)).collect();
    while cents.len() < k {
        let total: f64 = d2.iter().sum();
        if total <= 0.0 {
            break;
        }
        let mut r = rand() * total;
        let mut pick = n - 1;
        for (i, d) in d2.iter().enumerate() {
            r -= d;
            if r <= 0.0 {
                pick = i;
                break;
            }
        }
        cents.push(feats[pick].to_vec());
        for (i, f) in feats.iter().enumerate() {
            d2[i] = d2[i].min(dist(f, cents.last().unwrap()).powi(2));
        }
    }
    let mut assign = vec![0; n];
    for _ in 0..40 {
        let mut changed = false;
        for (i, f) in feats.iter().enumerate() {
            let best = (0..cents.len())
                .min_by(|&a, &b| dist(f, &cents[a]).total_cmp(&dist(f, &cents[b])))
                .unwrap();
            if best != assign[i] {
                assign[i] = best;
                changed = true;
            }
        }
        let mut sums = vec![vec![0.0; dim]; cents.len()];
        let mut counts = vec![0usize; cents.len()];
        for (i, f) in feats.iter().enumerate() {
            counts[assign[i]] += 1;
            for (s, v) in sums[assign[i]].iter_mut().zip(f.iter()) {
                *s += v;
            }
        }
        for (c, (s, &m)) in cents.iter_mut().zip(sums.iter().zip(&counts)) {
            if m > 0 {
                *c = s.iter().map(|v| v / m as f64).collect();
            }
        }
        if !changed {
            break;
        }
    }
    assign
}

/// Average vowel formants (F1, F2 in Hz) of men, women and children, for
/// naming entries.
const REFERENCE: [(&str, [(f64, f64); 3]); 12] = [
    ("i", [(342.0, 2322.0), (437.0, 2761.0), (452.0, 3081.0)]),
    ("ɪ", [(427.0, 2034.0), (483.0, 2365.0), (511.0, 2552.0)]),
    ("e", [(476.0, 2089.0), (536.0, 2530.0), (564.0, 2656.0)]),
    ("ɛ", [(580.0, 1799.0), (731.0, 2058.0), (749.0, 2267.0)]),
    ("æ", [(588.0, 1952.0), (669.0, 2349.0), (717.0, 2501.0)]),
    ("ɑ", [(768.0, 1333.0), (936.0, 1551.0), (1002.0, 1688.0)]),
    ("ɔ", [(652.0, 997.0), (781.0, 1136.0), (803.0, 1210.0)]),
    ("o", [(497.0, 910.0), (555.0, 1035.0), (597.0, 1137.0)]),
    ("ʊ", [(469.0, 1122.0), (519.0, 1225.0), (568.0, 1490.0)]),
    ("u", [(378.0, 997.0), (459.0, 1105.0), (494.0, 1345.0)]),
    ("ʌ", [(623.0, 1200.0), (753.0, 1426.0), (749.0, 1546.0)]),
    ("ɝ", [(474.0, 1379.0), (523.0, 1588.0), (586.0, 1719.0)]),
];

/// The nearest reference vowel to F1/F2, among the given voice type's
/// references (all of them for `Auto`).
pub fn label(f1: f64, f2: f64, voice: VoiceType) -> &'static str {
    let groups: &[usize] = match voice {
        VoiceType::Auto => &[0, 1, 2],
        VoiceType::Man => &[0],
        VoiceType::Woman => &[1],
        VoiceType::Child => &[2],
    };
    let d = |(a, b): (f64, f64)| (f1.ln() - a.ln()).powi(2) + (f2.ln() - b.ln()).powi(2);
    let best = |r: &(&str, [(f64, f64); 3])| {
        groups
            .iter()
            .map(|&g| d(r.1[g]))
            .fold(f64::INFINITY, f64::min)
    };
    REFERENCE
        .iter()
        .min_by(|x, y| best(x).total_cmp(&best(y)))
        .map_or("?", |r| r.0)
}

/// Smallest share of all cycles a vowel entry may have.
const MIN_SHARE: f64 = 0.02;

fn hnr_db(periodicity: f64) -> f64 {
    let r = periodicity.clamp(1e-4, 0.9999);
    10.0 * (r / (1.0 - r)).log10()
}

fn assemble(results: Vec<ClipResult>, s: &SpeakerSettings) -> Result<SpeakerProfile, String> {
    let recordings: Vec<RecordingSummary> = results.iter().map(|r| r.summary.clone()).collect();
    let (jsum, jn) = results
        .iter()
        .fold((0.0, 0), |a, r| (a.0 + r.jitter.0, a.1 + r.jitter.1));
    let (ssum, sn) = results
        .iter()
        .fold((0.0, 0), |a, r| (a.0 + r.shimmer.0, a.1 + r.shimmer.1));
    let runs: Vec<(f64, f64, f64, bool)> = results
        .iter()
        .flat_map(|r| r.vibrato.iter().copied())
        .collect();
    let cycles: Vec<CycleInfo> = results.into_iter().flat_map(|r| r.cycles).collect();
    if cycles.is_empty() {
        return Err(
            "no voiced speech found: is the pitch range (--fmin/--fmax) right for this voice?"
                .into(),
        );
    }

    let mut f0s: Vec<f64> = cycles.iter().map(|c| c.f0).collect();
    let pitch = PitchStats {
        median: percentile(&mut f0s, 0.5),
        low: percentile(&mut f0s, 0.05),
        high: percentile(&mut f0s, 0.95),
    };
    let total_run: f64 = runs.iter().map(|r| r.0).sum();
    let vib_runs: Vec<&(f64, f64, f64, bool)> = runs.iter().filter(|r| r.3).collect();
    let vib_time: f64 = vib_runs.iter().map(|r| r.0).sum();
    let vibrato = (total_run > 0.0 && vib_time / total_run > 0.3).then(|| Vibrato {
        rate: median(vib_runs.iter().map(|r| r.1).collect()),
        extent: median(vib_runs.iter().map(|r| r.2).collect()),
        presence: vib_time / total_run,
    });

    // The vowel table.
    let feats: Vec<&[f64]> = cycles.iter().map(|c| c.features.as_slice()).collect();
    let k = s.vowels.max(1).min(cycles.len());
    let mut assign = kmeans(&feats, k);
    // Merge entries that sound alike. The same vowel varies by about 3 dB (on average over the
    // bands) with breath and loudness; different vowels are 7 dB or more apart.
    const MERGE_DB: f64 = 4.0;
    loop {
        let groups = group(&assign, &feats);
        let mut merged = false;
        'outer: for a in 0..groups.len() {
            for b in a + 1..groups.len() {
                if groups[a].1 > 0 && groups[b].1 > 0 && dist(&groups[a].0, &groups[b].0) < MERGE_DB
                {
                    for x in assign.iter_mut().filter(|x| **x == b) {
                        *x = a;
                    }
                    merged = true;
                    break 'outer;
                }
            }
        }
        if !merged {
            break;
        }
    }
    // Clusters too small to be a real vowel (stray or transitional cycles) join their nearest entry.
    let min_members = ((cycles.len() as f64 * MIN_SHARE).ceil() as usize)
        .max(5)
        .min(cycles.len());
    let groups = group(&assign, &feats);
    let kept: Vec<usize> = (0..groups.len())
        .filter(|&g| groups[g].1 >= min_members)
        .collect();
    if kept.is_empty() {
        return Err("too few voiced cycles to build a vowel table".into());
    }
    for (i, a) in assign.iter_mut().enumerate() {
        if groups[*a].1 < min_members {
            *a = *kept
                .iter()
                .min_by(|&&x, &&y| {
                    dist(feats[i], &groups[x].0).total_cmp(&dist(feats[i], &groups[y].0))
                })
                .unwrap();
        }
    }
    let groups = group(&assign, &feats);
    let file = |c: &CycleInfo| recordings[c.clip].file.clone();
    let mut entries: Vec<(VowelEntry, Vec<f64>, usize)> = Vec::new(); // entry, centroid, group id
    for (g, (centroid, count)) in groups.iter().enumerate() {
        if *count == 0 {
            continue;
        }
        let members: Vec<&CycleInfo> = cycles
            .iter()
            .zip(&assign)
            .filter(|(_, &a)| a == g)
            .map(|(c, _)| c)
            .collect();
        let envelope = Envelope::mean(members.iter().map(|c| &c.envelope)).unwrap();
        let formants: Vec<Formant> = (0..4)
            .map_while(|slot| {
                let fs: Vec<&Formant> = members
                    .iter()
                    .filter_map(|c| c.formants.get(slot))
                    .collect();
                (fs.len() * 2 >= members.len()).then(|| Formant {
                    freq: median(fs.iter().map(|f| f.freq).collect()),
                    bw: median(fs.iter().map(|f| f.bw).collect()),
                })
            })
            .collect();
        let f0 = median(members.iter().map(|c| c.f0).collect());
        let mut typical: Vec<&&CycleInfo> = members.iter().collect();
        typical.sort_by(|a, b| dist(&a.features, centroid).total_cmp(&dist(&b.features, centroid)));
        let sources = typical
            .iter()
            .take(3)
            .map(|c| Source {
                file: file(c),
                time: c.time,
            })
            .collect();
        let lab = match (formants.first(), formants.get(1)) {
            (Some(a), Some(b)) => label(a.freq, b.freq, s.voice).to_string(),
            _ => "?".into(),
        };
        entries.push((
            VowelEntry {
                label: lab,
                formant_kinds: vec![FormantKind::Measured; formants.len()],
                formants,
                f0,
                count: *count,
                share: *count as f64 / cycles.len() as f64,
                hnr: median(members.iter().map(|c| hnr_db(c.periodicity)).collect()),
                h1_h2: median(members.iter().map(|c| c.envelope.h1_h2(c.f0)).collect()),
                envelope,
                sources,
            },
            centroid.clone(),
            g,
        ));
    }
    if entries.is_empty() {
        return Err("too few voiced cycles to build a vowel table".into());
    }
    // Fill in formants the recordings did not show, then name entries that lacked F2.
    let mut vowels: Vec<VowelEntry> = entries.iter().map(|e| e.0.clone()).collect();
    completion::complete(&mut vowels, s.voice);
    for ((e, _, _), v) in entries.iter_mut().zip(vowels) {
        if e.label == "?" && v.formants.len() >= 2 {
            e.label = label(v.formants[0].freq, v.formants[1].freq, s.voice).to_string();
        }
        *e = v;
    }
    let order = path_order(&entries);
    let entries: Vec<(VowelEntry, Vec<f64>, usize)> =
        order.into_iter().map(|i| entries[i].clone()).collect();

    // Display points: cycles with two formants, mapped to their entry's new index.
    let entry_of = |g: usize| entries.iter().position(|e| e.2 == g);
    let with_f: Vec<(&CycleInfo, usize)> = cycles
        .iter()
        .zip(&assign)
        .filter(|(c, _)| c.formants.len() >= 2)
        .filter_map(|(c, &g)| entry_of(g).map(|e| (c, e)))
        .collect();
    let stride = with_f.len().div_ceil(3000).max(1);
    let points = with_f
        .iter()
        .step_by(stride)
        .map(|(c, e)| VowelPoint {
            f1: c.formants[0].freq,
            f2: c.formants[1].freq,
            entry: *e,
        })
        .collect();

    let max_formant = s
        .max_formant
        .unwrap_or_else(|| s.voice.max_formant(pitch.median));
    Ok(SpeakerProfile {
        version: SpeakerProfile::VERSION,
        voice: s.voice,
        recordings,
        pitch,
        jitter: if jn > 0 {
            100.0 * jsum / jn as f64
        } else {
            0.0
        },
        shimmer: if sn > 0 {
            100.0 * ssum / sn as f64
        } else {
            0.0
        },
        hnr: median(cycles.iter().map(|c| hnr_db(c.periodicity)).collect()),
        h1_h2: median(cycles.iter().map(|c| c.envelope.h1_h2(c.f0)).collect()),
        vibrato,
        level: median(
            cycles
                .iter()
                .map(|c| 20.0 * c.rms.max(1e-9).log10())
                .collect(),
        ),
        max_formant,
        vowels: entries.into_iter().map(|e| e.0).collect(),
        points,
    })
}

/// Centroid and size of each group id.
fn group(assign: &[usize], feats: &[&[f64]]) -> Vec<(Vec<f64>, usize)> {
    let k = assign.iter().copied().max().unwrap_or(0) + 1;
    let dim = feats[0].len();
    let mut out = vec![(vec![0.0; dim], 0usize); k];
    for (i, f) in feats.iter().enumerate() {
        let g = &mut out[assign[i]];
        g.1 += 1;
        for (s, v) in g.0.iter_mut().zip(f.iter()) {
            *s += v;
        }
    }
    for g in out.iter_mut() {
        if g.1 > 0 {
            g.0.iter_mut().for_each(|v| *v /= g.1 as f64);
        }
    }
    out
}

/// Order entries so each sits next to the ones that sound most like it:
/// from the most front vowel (highest F2), nearest neighbour then 2-opt.
fn path_order(entries: &[(VowelEntry, Vec<f64>, usize)]) -> Vec<usize> {
    let n = entries.len();
    let f2 = |i: usize| entries[i].0.formants.get(1).map_or(0.0, |f| f.freq);
    let start = (0..n).max_by(|&a, &b| f2(a).total_cmp(&f2(b))).unwrap();
    let d = |a: usize, b: usize| dist(&entries[a].1, &entries[b].1);
    let mut path = vec![start];
    let mut used = vec![false; n];
    used[start] = true;
    while path.len() < n {
        let last = *path.last().unwrap();
        let next = (0..n)
            .filter(|&j| !used[j])
            .min_by(|&a, &b| d(last, a).total_cmp(&d(last, b)))
            .unwrap();
        used[next] = true;
        path.push(next);
    }
    for _ in 0..50 {
        let mut improved = false;
        for i in 0..n.saturating_sub(2) {
            for j in i + 2..n {
                let before = d(path[i], path[i + 1])
                    + if j + 1 < n {
                        d(path[j], path[j + 1])
                    } else {
                        0.0
                    };
                let after = d(path[i], path[j])
                    + if j + 1 < n {
                        d(path[i + 1], path[j + 1])
                    } else {
                        0.0
                    };
                if after + 1e-12 < before {
                    path[i + 1..=j].reverse();
                    improved = true;
                }
            }
        }
        if !improved {
            break;
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsignal::Voice;

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

    fn clip(name: &str, v: Voice, secs: f64) -> (String, Audio) {
        (
            name.to_string(),
            Audio {
                sample_rate: 44100,
                samples: v.render(44100, secs),
            },
        )
    }

    #[test]
    fn finds_each_vowel_and_the_voice_qualities() {
        let clips = vec![
            clip(
                "ah.wav",
                Voice {
                    f0: 120.0,
                    formants: AH.to_vec(),
                    jitter: 0.004,
                    seed: 1,
                    ..Default::default()
                },
                1.5,
            ),
            clip(
                "ee.wav",
                Voice {
                    f0: 125.0,
                    formants: EE.to_vec(),
                    jitter: 0.004,
                    seed: 2,
                    ..Default::default()
                },
                1.5,
            ),
            clip(
                "oo.wav",
                Voice {
                    f0: 115.0,
                    formants: OO.to_vec(),
                    jitter: 0.004,
                    seed: 3,
                    ..Default::default()
                },
                1.5,
            ),
        ];
        let p = analyze(&clips, &SpeakerSettings::default(), &|_, _| {}).unwrap();
        assert_eq!(
            p.vowels.len(),
            3,
            "{:?}",
            p.vowels.iter().map(|v| &v.label).collect::<Vec<_>>()
        );
        let find = |file: &str| p.vowels.iter().find(|v| v.sources[0].file == file).unwrap();
        for (file, want, label) in [
            ("ah.wav", AH, "ɑ"),
            ("ee.wav", EE, "i"),
            ("oo.wav", OO, "u"),
        ] {
            let v = find(file);
            assert!(
                v.sources.iter().all(|s| s.file == file),
                "{file} mixed with others"
            );
            assert_eq!(v.label, label, "{file}: {:?}", v.formants);
            for (k, w) in want.iter().take(2).enumerate() {
                assert!(
                    (v.formants[k].freq / w.0 - 1.0).abs() < 0.1,
                    "{file} F{}: {:?}",
                    k + 1,
                    v.formants
                );
            }
        }
        // Ordered as a path: "ee" (front) first.
        assert_eq!(p.vowels[0].label, "i");
        assert!((p.pitch.median - 120.0).abs() < 6.0, "{:?}", p.pitch);
        assert!(p.jitter > 0.2 && p.jitter < 1.5, "jitter {}", p.jitter);
        assert!(p.vibrato.is_none(), "{:?}", p.vibrato);
        assert!(!p.points.is_empty());
        // Round trip.
        assert_eq!(SpeakerProfile::from_json(&p.to_json()).unwrap(), p);
    }

    #[test]
    fn voice_type_shapes_the_search_and_the_names() {
        assert_eq!(VoiceType::Child.max_formant(400.0), 8000.0);
        assert_eq!(VoiceType::Man.pitch_range(), (60.0, 500.0));
        // 750/2270 Hz is a child's "ɛ", but not a man's.
        assert_eq!(label(749.0, 2267.0, VoiceType::Child), "ɛ");
        assert_ne!(label(749.0, 2267.0, VoiceType::Man), "ɛ");
        // A child's voice is analysed with the child's pitch range: a 300 Hz voice
        // gets one entry, at the right pitch.
        let v = Voice {
            f0: 300.0,
            formants: vec![
                (1000.0, 120.0),
                (1690.0, 140.0),
                (3200.0, 200.0),
                (4300.0, 300.0),
            ],
            ..Default::default()
        };
        let s = SpeakerSettings {
            voice: VoiceType::Child,
            ..Default::default()
        };
        let p = analyze(&[clip("kid.wav", v, 1.0)], &s, &|_, _| {}).unwrap();
        assert_eq!(p.voice, VoiceType::Child);
        assert_eq!(p.max_formant, 8000.0);
        assert!((p.pitch.median - 300.0).abs() < 5.0);
        assert_eq!(p.vowels[0].label, "ɑ", "{:?}", p.vowels[0].formants);
    }

    #[test]
    fn measures_vibrato() {
        let v = Voice {
            f0: 220.0,
            formants: AH.to_vec(),
            vibrato: Some((5.5, 50.0)),
            ..Default::default()
        };
        let p = analyze(
            &[clip("sung.wav", v, 3.0)],
            &SpeakerSettings::default(),
            &|_, _| {},
        )
        .unwrap();
        let vib = p.vibrato.expect("vibrato found");
        assert!((vib.rate - 5.5).abs() < 0.5, "{vib:?}");
        assert!((vib.extent - 50.0).abs() < 15.0, "{vib:?}");
    }

    #[test]
    fn breath_lowers_the_harmonics_to_noise_ratio() {
        let clean = Voice {
            f0: 150.0,
            formants: AH.to_vec(),
            ..Default::default()
        };
        let breathy = Voice {
            breath: 0.05,
            ..clean.clone()
        };
        let a = analyze(
            &[clip("a.wav", clean, 1.0)],
            &SpeakerSettings::default(),
            &|_, _| {},
        )
        .unwrap();
        let b = analyze(
            &[clip("b.wav", breathy, 1.0)],
            &SpeakerSettings::default(),
            &|_, _| {},
        )
        .unwrap();
        assert!(a.hnr > b.hnr + 3.0, "clean {} breathy {}", a.hnr, b.hnr);
    }
}

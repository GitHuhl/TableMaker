//! The pre-rendered material the voices play. For every vowel entry:
//!
//! * *recorded* tables, one single cycle per semitone, with harmonic
//!   amplitudes read from the vowel's spectral envelope at that pitch (so
//!   the formants stay where they were recorded), restored where the
//!   recording left gaps (see `tablemaker_dsp::completion`);
//! * *pulse* tables: a synthetic glottal pulse put through the vowel's
//!   formants as a cascade of resonators, for comparison;
//! * a breath-noise loop shaped by the vowel's envelope.

use rustfft::num_complex::Complex64;
use std::f64::consts::PI;
use tablemaker_dsp::completion::{self, FormantKind};
use tablemaker_dsp::envelope::{Envelope, POINTS, STEP_HZ};
use tablemaker_dsp::lpc::Formant;
use tablemaker_dsp::signal::Fft;
use tablemaker_dsp::speaker::{SpeakerProfile, VowelEntry};

/// Samples per table.
pub const TABLE: usize = 2048;
/// MIDI note of the lowest zone; one zone per semitone up to `ZONE_TOP`.
pub const ZONE_LOW: u8 = 21; // A0
pub const ZONE_TOP: u8 = 108; // C8
pub const ZONES: usize = (ZONE_TOP - ZONE_LOW) as usize + 1;
/// Breath-noise loop length (samples).
const NOISE: usize = 1 << 16;
/// RMS level of every table and noise loop.
const LEVEL: f64 = 0.25;
/// Points per period the glottal pulse is sampled at to find its harmonics.
const PULSE_POINTS: usize = 4096;

pub fn note_hz(note: f64) -> f64 {
    440.0 * 2f64.powf((note - 69.0) / 12.0)
}

/// The glottal pulse (Rosenberg model): the glottis opens over `open *
/// skew` of the period, closes over the rest of `open`, and stays shut for
/// the remainder.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PulseShape {
    /// Open quotient: the fraction of each period the glottis is open (0.3-0.95).
    /// Higher is softer and breathier, lower is pressed and brighter.
    pub open: f64,
    /// Speed quotient as a fraction: opening time over open time (0.5-0.9).
    /// Higher closes more abruptly, which is brighter.
    pub skew: f64,
}

impl Default for PulseShape {
    fn default() -> Self {
        Self {
            open: 0.6,
            skew: 0.7,
        }
    }
}

impl PulseShape {
    /// Complex harmonics of the flow derivative (lip radiation included),
    /// index h = harmonic h.
    fn harmonics(&self, fft: &mut Fft) -> Vec<Complex64> {
        let open = self.open.clamp(0.1, 0.99);
        let tp = open * self.skew.clamp(0.05, 0.95);
        let tn = open - tp;
        let x: Vec<f64> = (0..PULSE_POINTS)
            .map(|i| {
                let t = i as f64 / PULSE_POINTS as f64;
                if t < tp {
                    0.5 * PI / tp * (PI * t / tp).sin()
                } else if t < tp + tn {
                    -PI / (2.0 * tn) * (PI * (t - tp) / (2.0 * tn)).sin()
                } else {
                    0.0
                }
            })
            .collect();
        fft.spectrum(&x)
    }

    /// The shape whose pulse, through the vowels' formants, best matches
    /// the speaker's measured H1-H2 (low-harmonic spectral tilt).
    pub fn fit(profile: &SpeakerProfile, sample_rate: f64) -> Self {
        let mut fft = Fft::new();
        let tract = completion::speaker_tract(&profile.vowels, profile.voice);
        let filters: Vec<Vec<Formant>> = profile
            .vowels
            .iter()
            .map(|v| filter_formants(v, true, tract, sample_rate))
            .collect();
        let mut best = (f64::INFINITY, Self::default());
        for k in 0..=26 {
            let shape = Self {
                open: 0.3 + 0.025 * k as f64,
                ..Self::default()
            };
            let g = shape.harmonics(&mut fft);
            let err: f64 = profile
                .vowels
                .iter()
                .zip(&filters)
                .map(|(v, f)| {
                    let h1 = (g[1] * cascade(f, v.f0, sample_rate)).norm();
                    let h2 = (g[2] * cascade(f, 2.0 * v.f0, sample_rate)).norm();
                    (20.0 * (h1 / h2.max(1e-12)).log10() - v.h1_h2).abs()
                })
                .sum();
            if err < best.0 {
                best = (err, shape);
            }
        }
        best.1
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BankOptions {
    /// Play the restored envelopes and filter with inferred formants too.
    pub restore: bool,
    /// The pulse for the pulse tables; `None` fits it to the speaker.
    pub pulse: Option<PulseShape>,
}

impl Default for BankOptions {
    fn default() -> Self {
        Self {
            restore: true,
            pulse: None,
        }
    }
}

/// One vowel's material.
pub struct Vowel {
    /// `ZONES` tables of `TABLE` samples, back to back: the recorded timbre.
    pub tables: Vec<f32>,
    /// The same for the glottal pulse through the vowel's formants.
    pub pulse_tables: Vec<f32>,
    pub noise: Vec<f32>,
    /// Breath noise level for this vowel relative to the voiced sound, from its HNR.
    pub breath: f32,
}

impl Vowel {
    #[inline]
    pub fn table(&self, zone: usize) -> &[f32] {
        &self.tables[zone * TABLE..(zone + 1) * TABLE]
    }

    #[inline]
    pub fn pulse_table(&self, zone: usize) -> &[f32] {
        &self.pulse_tables[zone * TABLE..(zone + 1) * TABLE]
    }
}

pub struct Bank {
    pub sample_rate: f64,
    pub vowels: Vec<Vowel>,
    /// Breath noise level giving the speaker's own harmonics-to-noise ratio.
    pub noise_gain: f32,
    pub options: BankOptions,
    /// The pulse the pulse tables were made with.
    pub pulse: PulseShape,
}

/// The minimum-phase frequency response with the envelope's magnitude, on
/// the envelope's grid (0 .. (POINTS - 1) * STEP_HZ). Minimum phase gives a
/// compact, voice-like pulse, and the same phase behaviour for every vowel,
/// so morphing between vowels never cancels.
fn minimum_phase(fft: &mut Fft, env: &Envelope) -> Vec<Complex64> {
    let m = POINTS - 1;
    let n = 2 * m;
    let ln10 = std::f64::consts::LN_10;
    let mut logmag = vec![Complex64::new(0.0, 0.0); n];
    for k in 0..=m {
        let v = env.db[k] as f64 * ln10 / 20.0;
        logmag[k].re = v;
        if k > 0 && k < m {
            logmag[n - k].re = v;
        }
    }
    // Real cepstrum, folded onto positive quefrencies.
    fft.inverse(&mut logmag);
    let mut c = vec![Complex64::new(0.0, 0.0); n];
    c[0] = logmag[0];
    for q in 1..m {
        c[q] = logmag[q] * 2.0;
    }
    c[m] = logmag[m];
    fft.forward(&mut c);
    c.iter().take(m + 1).map(|z| z.exp()).collect()
}

fn response_at(h: &[Complex64], freq: f64) -> Complex64 {
    let pos = freq / STEP_HZ;
    let i = pos.floor() as usize;
    if i + 1 >= h.len() {
        return Complex64::new(0.0, 0.0);
    }
    let t = pos - i as f64;
    h[i] * (1.0 - t) + h[i + 1] * t
}

/// The formants the pulse is filtered with: the vowel's (inferred ones
/// only when restoring), then the vocal tract's higher resonances up to
/// the Nyquist frequency.
fn filter_formants(v: &VowelEntry, restore: bool, tract: f64, sample_rate: f64) -> Vec<Formant> {
    let mut f: Vec<Formant> = v
        .formants
        .iter()
        .zip(
            v.formant_kinds
                .iter()
                .chain(std::iter::repeat(&FormantKind::Measured)),
        )
        .filter(|(_, k)| restore || **k != FormantKind::Inferred)
        .map(|(f, _)| *f)
        .collect();
    let top = f.last().map_or(0.0, |x| x.freq);
    let mut slot = completion::SLOTS;
    loop {
        let freq = completion::tube_formant(slot, tract);
        if freq > sample_rate * 0.45 {
            break;
        }
        if freq > top + 300.0 {
            f.push(Formant {
                freq,
                bw: 150.0 + 0.05 * freq,
            });
        }
        slot += 1;
    }
    f
}

/// Response of a cascade of digital resonators (unity gain at 0 Hz) at `freq`.
fn cascade(formants: &[Formant], freq: f64, sample_rate: f64) -> Complex64 {
    let w = 2.0 * PI * freq / sample_rate;
    let z1 = Complex64::from_polar(1.0, -w);
    let z2 = z1 * z1;
    formants.iter().fold(Complex64::new(1.0, 0.0), |acc, f| {
        let r = (-PI * f.bw / sample_rate).exp();
        let b = 2.0 * r * (2.0 * PI * f.freq / sample_rate).cos();
        let c = -r * r;
        let a = 1.0 - b - c;
        acc * (a / (Complex64::new(1.0, 0.0) - z1 * b - z2 * c))
    })
}

fn rms_normalise(x: &mut [f64]) {
    let rms = (x.iter().map(|v| v * v).sum::<f64>() / x.len() as f64).sqrt();
    if rms > 0.0 {
        x.iter_mut().for_each(|v| *v *= LEVEL / rms);
    }
}

/// Render a table from harmonic values `h(k, f)` for each harmonic k at `f`.
fn render_table(
    fft: &mut Fft,
    f: f64,
    harmonics: usize,
    value: impl Fn(usize, f64) -> Complex64,
) -> Vec<f64> {
    let mut spec = vec![Complex64::new(0.0, 0.0); TABLE];
    for k in 1..=harmonics {
        let r = value(k, k as f64 * f);
        spec[k] = r;
        spec[TABLE - k] = r.conj();
    }
    let mut x = fft.real(spec);
    rms_normalise(&mut x);
    x
}

impl Bank {
    pub fn new(profile: &SpeakerProfile, sample_rate: f64) -> Self {
        Self::with_options(profile, sample_rate, BankOptions::default())
    }

    pub fn with_options(profile: &SpeakerProfile, sample_rate: f64, options: BankOptions) -> Self {
        let mut fft = Fft::new();
        let nyquist = sample_rate / 2.0;
        let mean_hnr =
            profile.vowels.iter().map(|v| v.hnr).sum::<f64>() / profile.vowels.len().max(1) as f64;
        let envelopes: Vec<Envelope> = if options.restore {
            completion::restored_envelopes(profile)
        } else {
            profile.vowels.iter().map(|v| v.envelope.clone()).collect()
        };
        let pulse = options
            .pulse
            .unwrap_or_else(|| PulseShape::fit(profile, sample_rate));
        let glottal = pulse.harmonics(&mut fft);
        let tract = completion::speaker_tract(&profile.vowels, profile.voice);
        let mut seed = 0x853C49E6748FEA9Bu64;
        let vowels = profile
            .vowels
            .iter()
            .zip(&envelopes)
            .map(|(v, env)| {
                let h = minimum_phase(&mut fft, env);
                let formants = filter_formants(v, options.restore, tract, sample_rate);
                let mut tables = Vec::with_capacity(ZONES * TABLE);
                let mut pulse_tables = Vec::with_capacity(ZONES * TABLE);
                for z in 0..ZONES {
                    let f = note_hz((ZONE_LOW as usize + z) as f64);
                    // Band-limited for the top of the zone's playing range (it is
                    // crossfaded with the zone above, up to that zone's pitch).
                    let top = note_hz((ZONE_LOW as usize + z + 1) as f64);
                    let harmonics =
                        ((nyquist * 0.95 / top).floor() as usize).clamp(1, TABLE / 2 - 1);
                    let rec = render_table(&mut fft, f, harmonics, |_, freq| response_at(&h, freq));
                    let pul = render_table(&mut fft, f, harmonics, |k, freq| {
                        glottal.get(k).copied().unwrap_or_default()
                            * cascade(&formants, freq, sample_rate)
                    });
                    tables.extend(rec.iter().map(|&x| x as f32));
                    pulse_tables.extend(pul.iter().map(|&x| x as f32));
                }
                // Noise with the vowel's envelope: random phases under its magnitude.
                let mut spec = vec![Complex64::new(0.0, 0.0); NOISE];
                for k in 1..NOISE / 2 {
                    let f = k as f64 * sample_rate / NOISE as f64;
                    seed = seed
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    let phase = (seed >> 11) as f64 / (1u64 << 53) as f64 * std::f64::consts::TAU;
                    let r = Complex64::from_polar(response_at(&h, f).norm(), phase);
                    spec[k] = r;
                    spec[NOISE - k] = r.conj();
                }
                let mut noise = fft.real(spec);
                rms_normalise(&mut noise);
                // Breathier vowels (lower HNR than the speaker's average) get more noise.
                let breath = 10f64.powf((mean_hnr - v.hnr) / 20.0).clamp(0.25, 4.0) as f32;
                Vowel {
                    tables,
                    pulse_tables,
                    noise: noise.iter().map(|&x| x as f32).collect(),
                    breath,
                }
            })
            .collect();
        Bank {
            sample_rate,
            vowels,
            noise_gain: 10f64.powf(-mean_hnr / 20.0).clamp(0.005, 0.7) as f32,
            options,
            pulse,
        }
    }

    /// Zone index and crossfade position for a frequency.
    #[inline]
    pub fn zone(freq: f64) -> (usize, f32) {
        let note = 69.0 + 12.0 * (freq / 440.0).log2() - ZONE_LOW as f64;
        let note = note.clamp(0.0, (ZONES - 1) as f64 - 1e-6);
        (note as usize, (note - note.floor()) as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resonator_cascade_peaks_at_its_formants() {
        let f = [
            Formant {
                freq: 700.0,
                bw: 80.0,
            },
            Formant {
                freq: 1200.0,
                bw: 100.0,
            },
        ];
        let at = |x: f64| cascade(&f, x, 48000.0).norm();
        assert!((at(0.0) - 1.0).abs() < 1e-9);
        assert!(at(700.0) > 5.0 * at(450.0) && at(700.0) > 3.0 * at(950.0));
        assert!(at(1200.0) > 2.0 * at(950.0) && at(1200.0) > 3.0 * at(1700.0));
    }

    #[test]
    fn a_more_open_pulse_is_darker() {
        let mut fft = Fft::new();
        let mut tilt = |open: f64| {
            let g = PulseShape { open, skew: 0.7 }.harmonics(&mut fft);
            20.0 * (g[1].norm() / g[2].norm()).log10()
        };
        assert!(
            tilt(0.9) > tilt(0.4) + 3.0,
            "H1-H2 {} vs {}",
            tilt(0.9),
            tilt(0.4)
        );
    }
}

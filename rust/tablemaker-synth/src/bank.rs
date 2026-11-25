//! The pre-rendered material the voices play: for every vowel entry, one
//! single-cycle table per semitone (harmonic amplitudes read from the
//! vowel's envelope at that pitch, so the formants stay where they were
//! recorded), and a breath-noise loop shaped by the same envelope.

use rustfft::num_complex::Complex64;
use tablemaker_dsp::envelope::{Envelope, POINTS, STEP_HZ};
use tablemaker_dsp::signal::Fft;
use tablemaker_dsp::speaker::SpeakerProfile;

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

pub fn note_hz(note: f64) -> f64 {
    440.0 * 2f64.powf((note - 69.0) / 12.0)
}

/// One vowel's material.
pub struct Vowel {
    /// `ZONES` tables of `TABLE` samples, back to back.
    pub tables: Vec<f32>,
    pub noise: Vec<f32>,
    /// Breath noise level for this vowel relative to the voiced sound, from its HNR.
    pub breath: f32,
}

impl Vowel {
    #[inline]
    pub fn table(&self, zone: usize) -> &[f32] {
        &self.tables[zone * TABLE..(zone + 1) * TABLE]
    }
}

pub struct Bank {
    pub sample_rate: f64,
    pub vowels: Vec<Vowel>,
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

fn rms_normalise(x: &mut [f64]) {
    let rms = (x.iter().map(|v| v * v).sum::<f64>() / x.len() as f64).sqrt();
    if rms > 0.0 {
        x.iter_mut().for_each(|v| *v *= LEVEL / rms);
    }
}

impl Bank {
    pub fn new(profile: &SpeakerProfile, sample_rate: f64) -> Self {
        let mut fft = Fft::new();
        let nyquist = sample_rate / 2.0;
        let mean_hnr =
            profile.vowels.iter().map(|v| v.hnr).sum::<f64>() / profile.vowels.len().max(1) as f64;
        let mut seed = 0x853C49E6748FEA9Bu64;
        let vowels = profile
            .vowels
            .iter()
            .map(|v| {
                let h = minimum_phase(&mut fft, &v.envelope);
                let mut tables = Vec::with_capacity(ZONES * TABLE);
                for z in 0..ZONES {
                    let f = note_hz((ZONE_LOW as usize + z) as f64);
                    // Band-limited for the top of the zone's playing range (it is
                    // crossfaded with the zone above, up to that zone's pitch).
                    let top = note_hz((ZONE_LOW as usize + z + 1) as f64);
                    let harmonics =
                        ((nyquist * 0.95 / top).floor() as usize).clamp(1, TABLE / 2 - 1);
                    let mut spec = vec![Complex64::new(0.0, 0.0); TABLE];
                    for k in 1..=harmonics {
                        let r = response_at(&h, k as f64 * f);
                        spec[k] = r;
                        spec[TABLE - k] = r.conj();
                    }
                    let mut x = fft.real(spec);
                    rms_normalise(&mut x);
                    tables.extend(x.iter().map(|&v| v as f32));
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
                    noise: noise.iter().map(|&v| v as f32).collect(),
                    breath,
                }
            })
            .collect();
        Bank {
            sample_rate,
            vowels,
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

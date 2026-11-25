//! The real-time voice engine. Nothing in `process` allocates or locks.

use crate::bank::{note_hz, Bank, TABLE};
use std::sync::Arc;
use tablemaker_dsp::speaker::SpeakerProfile;

const VOICES: usize = 16;
const BLOCK: usize = 32;

/// Performance settings. Defaults come from the speaker's own measurements.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    pub vibrato_rate: f32,
    /// Peak vibrato (cents), reached `vibrato_delay` + 0.4 s into a note.
    pub vibrato_depth: f32,
    pub vibrato_delay: f32,
    /// Random pitch wander (fraction of the frequency, standard deviation).
    pub jitter: f32,
    /// Random level wander (fraction, standard deviation).
    pub shimmer: f32,
    /// Breath noise, 1 = as much as the speaker has.
    pub breath: f32,
    /// One voice with legato glides instead of chords.
    pub mono: bool,
    /// Glide time between legato notes (s).
    pub glide: f32,
    pub bend_range: f32,
    /// Time to move between vowels (s).
    pub vowel_glide: f32,
    /// Keys from this note upwards choose vowels instead of sounding.
    pub vowel_keys: Option<u8>,
    pub gain: f32,
    /// What sounds: 0 = the recorded timbre, 1 = a glottal pulse through the
    /// vowel's formant filters; in between, a blend.
    pub source: f32,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            attack: 0.06,
            decay: 0.15,
            sustain: 0.85,
            release: 0.25,
            vibrato_rate: 5.5,
            vibrato_depth: 0.0,
            vibrato_delay: 0.3,
            jitter: 0.004,
            shimmer: 0.03,
            breath: 1.0,
            mono: false,
            glide: 0.08,
            bend_range: 2.0,
            vowel_glide: 0.08,
            vowel_keys: None,
            gain: 0.5,
            source: 0.0,
        }
    }
}

impl Params {
    /// The speaker's own vibrato, wander and breathiness.
    pub fn from_profile(p: &SpeakerProfile) -> Self {
        let mut s = Self::default();
        if let Some(v) = p.vibrato {
            s.vibrato_rate = v.rate as f32;
            s.vibrato_depth = v.extent as f32;
        }
        // Measured as mean change between neighbouring periods: about 1.13 standard deviations.
        s.jitter = ((p.jitter / 100.0) / 1.13).clamp(0.0, 0.03) as f32;
        s.shimmer = ((p.shimmer / 100.0) / 1.13).clamp(0.0, 0.2) as f32;
        s
    }
}

/// What the engine responds to.
#[derive(Clone)]
pub enum Event {
    NoteOn {
        note: u8,
        velocity: u8,
    },
    NoteOff {
        note: u8,
    },
    /// MIDI controller: 1 vowel, 2 breath, 11 expression, 64 sustain, 123 all notes off.
    Control {
        cc: u8,
        value: u8,
    },
    /// -1..1
    PitchBend(f32),
    /// Channel pressure 0..1: adds vibrato.
    Pressure(f32),
    /// Vowel position 0..1 along the table.
    Vowel(f32),
    Params(Params),
    Bank(Arc<Bank>),
    AllOff,
}

impl Event {
    /// From a raw MIDI message (any channel).
    pub fn from_midi(m: &[u8]) -> Option<Event> {
        let status = *m.first()?;
        let d = |i: usize| m.get(i).copied().unwrap_or(0) & 0x7F;
        match status & 0xF0 {
            0x90 if d(2) > 0 => Some(Event::NoteOn {
                note: d(1),
                velocity: d(2),
            }),
            0x80 | 0x90 => Some(Event::NoteOff { note: d(1) }),
            0xB0 => Some(Event::Control {
                cc: d(1),
                value: d(2),
            }),
            0xD0 => Some(Event::Pressure(d(1) as f32 / 127.0)),
            0xE0 => Some(Event::PitchBend(
                ((d(2) as i32) << 7 | d(1) as i32) as f32 / 8192.0 - 1.0,
            )),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Off,
    Attack,
    Decay,
    Sustain,
    Release,
}

#[derive(Clone, Copy)]
struct Voice {
    stage: Stage,
    note: u8,
    held: bool,
    velocity: f32,
    freq: f64,
    target: f64,
    phase: f64,
    noise_pos: usize,
    env: f32,
    age: u64,
    time: f32,
    vib_phase: f32,
    jitter: f32,
    shimmer: f32,
}

impl Voice {
    const OFF: Voice = Voice {
        stage: Stage::Off,
        note: 0,
        held: false,
        velocity: 0.0,
        freq: 440.0,
        target: 440.0,
        phase: 0.0,
        noise_pos: 0,
        env: 0.0,
        age: 0,
        time: 0.0,
        vib_phase: 0.0,
        jitter: 0.0,
        shimmer: 0.0,
    };
}

pub struct Engine {
    bank: Arc<Bank>,
    params: Params,
    sr: f32,
    voices: [Voice; VOICES],
    counter: u64,
    rng: u64,
    bend: f32,
    pressure: f32,
    vowel_target: f32,
    vowel: f32,
    expression: f32,
    breath_cc: f32,
    sustain_pedal: bool,
    /// Held notes in mono mode, latest last.
    stack: [u8; 128],
    stack_len: usize,
}

#[inline]
fn soft_clip(x: f32) -> f32 {
    let x = x.clamp(-3.0, 3.0);
    x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
}

impl Engine {
    pub fn new(bank: Arc<Bank>, params: Params) -> Self {
        Self {
            sr: bank.sample_rate as f32,
            bank,
            params,
            voices: [Voice::OFF; VOICES],
            counter: 0,
            rng: 0x9E3779B97F4A7C15,
            bend: 0.0,
            pressure: 0.0,
            vowel_target: 0.0,
            vowel: 0.0,
            expression: 1.0,
            breath_cc: 1.0,
            sustain_pedal: false,
            stack: [0; 128],
            stack_len: 0,
        }
    }

    /// An engine for a speaker at a sample rate, set up from their profile.
    pub fn for_profile(p: &SpeakerProfile, sample_rate: f64) -> Self {
        Self::new(Arc::new(Bank::new(p, sample_rate)), Params::from_profile(p))
    }

    /// The same with chosen bank options (restoration, pulse shape).
    pub fn with_options(
        p: &SpeakerProfile,
        sample_rate: f64,
        options: crate::bank::BankOptions,
    ) -> Self {
        Self::new(
            Arc::new(Bank::with_options(p, sample_rate, options)),
            Params::from_profile(p),
        )
    }

    pub fn params(&self) -> Params {
        self.params
    }

    /// Current vowel position, 0..1.
    pub fn vowel(&self) -> f32 {
        self.vowel
    }

    /// Notes sounding (bit per MIDI note).
    pub fn sounding(&self) -> u128 {
        self.voices
            .iter()
            .filter(|v| v.stage != Stage::Off)
            .fold(0, |m, v| m | 1u128 << v.note)
    }

    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 40) as f32 / (1u64 << 24) as f32
    }

    fn vowel_count(&self) -> usize {
        self.bank.vowels.len()
    }

    pub fn handle(&mut self, e: Event) {
        match e {
            Event::NoteOn { note, velocity } => {
                if let Some(low) = self.params.vowel_keys {
                    let n = self.vowel_count();
                    if note >= low && (note - low) < n as u8 {
                        self.vowel_target = if n > 1 {
                            (note - low) as f32 / (n - 1) as f32
                        } else {
                            0.0
                        };
                        return;
                    }
                }
                self.note_on(note, velocity);
            }
            Event::NoteOff { note } => self.note_off(note),
            Event::Control { cc, value } => {
                let v = value as f32 / 127.0;
                match cc {
                    1 => self.vowel_target = v,
                    2 => self.breath_cc = v * 2.0,
                    11 => self.expression = v,
                    64 => {
                        self.sustain_pedal = value >= 64;
                        if !self.sustain_pedal {
                            for i in 0..VOICES {
                                if !self.voices[i].held && self.voices[i].stage != Stage::Off {
                                    self.voices[i].stage = Stage::Release;
                                }
                            }
                        }
                    }
                    120 | 123 => self.all_off(),
                    _ => {}
                }
            }
            Event::PitchBend(b) => self.bend = b,
            Event::Pressure(p) => self.pressure = p,
            Event::Vowel(v) => self.vowel_target = v.clamp(0.0, 1.0),
            Event::Params(p) => self.params = p,
            Event::Bank(b) => {
                self.all_off();
                self.bank = b;
            }
            Event::AllOff => self.all_off(),
        }
    }

    fn all_off(&mut self) {
        for v in self.voices.iter_mut() {
            if v.stage != Stage::Off {
                v.stage = Stage::Release;
                v.held = false;
            }
        }
        self.stack_len = 0;
    }

    fn start(&mut self, i: usize, note: u8, velocity: u8, glide_from: Option<f64>) {
        self.counter += 1;
        let f = note_hz(note as f64);
        let (r1, r2) = (self.rand(), self.rand());
        let v = &mut self.voices[i];
        v.note = note;
        v.held = true;
        v.velocity = (velocity as f32 / 127.0).powf(1.5);
        v.target = f;
        v.freq = glide_from.unwrap_or(f);
        v.age = self.counter;
        if glide_from.is_none() {
            v.stage = Stage::Attack;
            v.time = 0.0;
            v.vib_phase = r1;
            v.noise_pos = (r2 * 60000.0) as usize;
            if v.env == 0.0 {
                v.phase = 0.0;
            }
        }
    }

    fn note_on(&mut self, note: u8, velocity: u8) {
        if self.params.mono {
            self.stack_remove(note);
            if self.stack_len < 128 {
                self.stack[self.stack_len] = note;
                self.stack_len += 1;
            }
            let v = self.voices[0];
            let legato = v.stage != Stage::Off && v.stage != Stage::Release && v.held;
            self.start(0, note, velocity, legato.then_some(v.freq));
            return;
        }
        let i = self
            .voices
            .iter()
            .position(|v| v.stage == Stage::Off)
            .or_else(|| {
                (0..VOICES)
                    .filter(|&i| self.voices[i].stage == Stage::Release)
                    .min_by_key(|&i| self.voices[i].age)
            })
            .unwrap_or_else(|| (0..VOICES).min_by_key(|&i| self.voices[i].age).unwrap());
        self.start(i, note, velocity, None);
    }

    fn stack_remove(&mut self, note: u8) {
        if let Some(p) = self.stack[..self.stack_len].iter().position(|&n| n == note) {
            self.stack.copy_within(p + 1..self.stack_len, p);
            self.stack_len -= 1;
        }
    }

    fn note_off(&mut self, note: u8) {
        if self.params.mono {
            self.stack_remove(note);
            let v = self.voices[0];
            if v.note == note && v.stage != Stage::Off {
                if self.stack_len > 0 {
                    // Back to the previous held note, legato.
                    let prev = self.stack[self.stack_len - 1];
                    let vel = (v.velocity.powf(1.0 / 1.5) * 127.0) as u8;
                    self.start(0, prev, vel, Some(v.freq));
                } else {
                    self.release(0);
                }
            }
            return;
        }
        for i in 0..VOICES {
            if self.voices[i].note == note
                && self.voices[i].held
                && self.voices[i].stage != Stage::Off
            {
                self.release(i);
            }
        }
    }

    fn release(&mut self, i: usize) {
        self.voices[i].held = false;
        if !self.sustain_pedal {
            self.voices[i].stage = Stage::Release;
        }
    }

    /// Render mono audio, adding nothing to what is there (overwrites).
    pub fn process(&mut self, out: &mut [f32]) {
        let sr = self.sr;
        let p = self.params;
        let n_vowels = self.vowel_count();
        if n_vowels == 0 {
            out.iter_mut().for_each(|s| *s = 0.0);
            return;
        }
        for chunk in out.chunks_mut(BLOCK) {
            chunk.iter_mut().for_each(|s| *s = 0.0);
            let dt = chunk.len() as f32 / sr;
            // Vowel position glides to its target.
            let k = 1.0 - (-dt / p.vowel_glide.max(0.001)).exp();
            self.vowel += (self.vowel_target - self.vowel) * k;
            let pos = self.vowel * (n_vowels.max(1) - 1) as f32;
            let e0 = (pos.floor() as usize).min(n_vowels.saturating_sub(1));
            let e1 = (e0 + 1).min(n_vowels.saturating_sub(1));
            let ef = pos - e0 as f32;
            let bank = Arc::clone(&self.bank);
            let (v0, v1) = (&bank.vowels[e0], &bank.vowels[e1]);
            let breath = p.breath
                * self.breath_cc
                * bank.noise_gain
                * (v0.breath + (v1.breath - v0.breath) * ef);
            let bend = 2f64.powf((self.bend * p.bend_range) as f64 / 12.0);
            let attack_step = dt / p.attack.max(0.001);
            let decay_k = (-dt / (p.decay.max(0.001) / 4.0)).exp();
            let release_k = (-dt / (p.release.max(0.001) / 4.0)).exp();
            let glide_k = 1.0 - (-(dt as f64) / (p.glide.max(0.001) as f64 / 3.0)).exp();
            for vi in 0..VOICES {
                if self.voices[vi].stage == Stage::Off {
                    continue;
                }
                let (g1, g2) = (self.rand() * 2.0 - 1.0, self.rand() * 2.0 - 1.0);
                let v = &mut self.voices[vi];
                // Envelope, per block (smooth enough at 32 samples).
                let env_start = v.env;
                match v.stage {
                    Stage::Attack => {
                        v.env += attack_step;
                        if v.env >= 1.0 {
                            v.env = 1.0;
                            v.stage = Stage::Decay;
                        }
                    }
                    Stage::Decay => {
                        v.env = p.sustain + (v.env - p.sustain) * decay_k;
                        if (v.env - p.sustain).abs() < 1e-3 {
                            v.stage = Stage::Sustain;
                        }
                    }
                    Stage::Sustain => v.env = p.sustain,
                    Stage::Release => {
                        v.env *= release_k;
                        if v.env < 1e-4 {
                            v.env = 0.0;
                            v.stage = Stage::Off;
                        }
                    }
                    Stage::Off => {}
                }
                v.time += dt;
                // Pitch: glide, bend, vibrato and wander.
                v.freq += (v.target - v.freq) * glide_k;
                v.vib_phase = (v.vib_phase + p.vibrato_rate * dt).fract();
                let ramp = ((v.time - p.vibrato_delay) / 0.4).clamp(0.0, 1.0);
                let depth = p.vibrato_depth * ramp + self.pressure * 60.0;
                let vib = depth * (v.vib_phase * std::f32::consts::TAU).sin();
                // Low-passed noise; 2.4 scales its spread back to the requested deviation.
                v.jitter = 0.7 * v.jitter + 0.3 * 2.4 * 1.73 * g1 * p.jitter;
                v.shimmer = 0.7 * v.shimmer + 0.3 * 2.4 * 1.73 * g2 * p.shimmer;
                let f = v.freq * bend * 2f64.powf(vib as f64 / 1200.0) * (1.0 + v.jitter as f64);
                let (z, zf) = Bank::zone(f);
                let (a0, a1, b0, b1) = (v0.table(z), v0.table(z + 1), v1.table(z), v1.table(z + 1));
                let (p0, p1, q0, q1) = (
                    v0.pulse_table(z),
                    v0.pulse_table(z + 1),
                    v1.pulse_table(z),
                    v1.pulse_table(z + 1),
                );
                let mix = p.source.clamp(0.0, 1.0);
                let step = f / sr as f64;
                let amp = v.velocity * (1.0 + v.shimmer);
                let n = chunk.len() as f32;
                for (j, s) in chunk.iter_mut().enumerate() {
                    let env = env_start + (v.env - env_start) * (j as f32 + 1.0) / n;
                    let x = v.phase * TABLE as f64;
                    let i = x as usize & (TABLE - 1);
                    let i2 = (i + 1) & (TABLE - 1);
                    let t = (x - x.floor()) as f32;
                    let rd = |tb: &[f32]| tb[i] + (tb[i2] - tb[i]) * t;
                    let play = |t0: &[f32], t1: &[f32], u0: &[f32], u1: &[f32]| {
                        let a = rd(t0) + (rd(t1) - rd(t0)) * zf;
                        let b = rd(u0) + (rd(u1) - rd(u0)) * zf;
                        a + (b - a) * ef
                    };
                    // Only the sources that are heard are read.
                    let voiced = if mix <= 0.0 {
                        play(a0, a1, b0, b1)
                    } else if mix >= 1.0 {
                        play(p0, p1, q0, q1)
                    } else {
                        let r = play(a0, a1, b0, b1);
                        r + (play(p0, p1, q0, q1) - r) * mix
                    };
                    let np = v.noise_pos & (v0.noise.len() - 1);
                    let noise = v0.noise[np] + (v1.noise[np] - v0.noise[np]) * ef;
                    // Breath noise pulses with the glottal cycle, as it does in a real voice.
                    let pulse = 0.6 + 0.4 * (v.phase as f32 * std::f32::consts::TAU).cos();
                    *s += (voiced + noise * breath * pulse) * env * amp;
                    v.phase += step;
                    if v.phase >= 1.0 {
                        v.phase -= 1.0;
                    }
                    v.noise_pos += 1;
                }
            }
            let g = p.gain * self.expression;
            chunk.iter_mut().for_each(|s| *s = soft_clip(*s * g));
        }
    }

    /// Render a timed list of events offline (seconds, event).
    pub fn render(&mut self, score: &[(f64, Event)], seconds: f64) -> Vec<f32> {
        let n = (seconds * self.sr as f64) as usize;
        let mut out = vec![0.0f32; n];
        let mut events: Vec<&(f64, Event)> = score.iter().collect();
        events.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut at = 0;
        let mut ei = 0;
        while at < n {
            while ei < events.len() && (events[ei].0 * self.sr as f64) as usize <= at {
                self.handle(events[ei].1.clone());
                ei += 1;
            }
            let next = events
                .get(ei)
                .map_or(n, |e| ((e.0 * self.sr as f64) as usize).clamp(at + 1, n));
            let end = next.min(at + BLOCK).max(at + 1);
            self.process(&mut out[at..end]);
            at = end;
        }
        out
    }
}

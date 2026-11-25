//! Live playing: the synth on the sound card, driven by MIDI keyboards and
//! by the app itself (through the same lock-free queue).

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tablemaker_dsp::speaker::SpeakerProfile;
use tablemaker_synth::{Bank, BankOptions, Engine, Event, Params};

/// What the audio thread reports back.
#[derive(Default)]
pub struct Status {
    vowel: AtomicU32,
    sounding_lo: AtomicU64,
    sounding_hi: AtomicU64,
    peak: AtomicU32,
}

impl Status {
    /// Current vowel position, 0..1.
    pub fn vowel(&self) -> f32 {
        f32::from_bits(self.vowel.load(Ordering::Relaxed))
    }

    pub fn sounding(&self, note: u8) -> bool {
        let (word, bit) = if note < 64 {
            (&self.sounding_lo, note)
        } else {
            (&self.sounding_hi, note - 64)
        };
        word.load(Ordering::Relaxed) >> bit & 1 == 1
    }

    /// Output peak since last asked, 0..1.
    pub fn take_peak(&self) -> f32 {
        f32::from_bits(self.peak.swap(0, Ordering::Relaxed))
    }
}

/// Sends events to the audio thread from any thread.
#[derive(Clone)]
pub struct Sender(Arc<Mutex<rtrb::Producer<Event>>>);

impl Sender {
    pub fn send(&self, e: Event) {
        if let Ok(mut p) = self.0.lock() {
            let _ = p.push(e); // a full queue drops the event rather than block
        }
    }
}

pub struct Live {
    _stream: cpal::Stream,
    _midi: Vec<midir::MidiInputConnection<()>>,
    pub sender: Sender,
    pub status: Arc<Status>,
    pub device: String,
    pub sample_rate: u32,
    pub midi_ports: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct LiveOptions {
    /// Output device (part of its name); default device if unset.
    pub device: Option<String>,
    /// MIDI inputs to connect (part of their name); all if unset, none if "none".
    pub midi: Option<String>,
}

pub fn output_devices() -> Vec<String> {
    cpal::default_host()
        .output_devices()
        .map(|d| d.map(|d| d.to_string()).collect())
        .unwrap_or_default()
}

pub fn midi_inputs() -> Vec<String> {
    let Ok(m) = midir::MidiInput::new("tablemaker") else {
        return Vec::new();
    };
    m.ports()
        .iter()
        .filter_map(|p| m.port_name(p).ok())
        .collect()
}

impl Live {
    pub fn start(
        profile: &SpeakerProfile,
        params: Option<Params>,
        bank: BankOptions,
        o: &LiveOptions,
    ) -> Result<Live, String> {
        let host = cpal::default_host();
        let device = match &o.device {
            None => host
                .default_output_device()
                .ok_or("no audio output device")?,
            Some(want) => host
                .output_devices()
                .map_err(|e| e.to_string())?
                .find(|d| d.to_string().to_lowercase().contains(&want.to_lowercase()))
                .ok_or_else(|| format!("no output device matching \"{want}\""))?,
        };
        let supported = device
            .default_output_config()
            .map_err(|e| format!("{device}: {e}"))?;
        let config = supported.config();
        let sample_rate = config.sample_rate;
        let mut engine = Engine::with_options(profile, sample_rate as f64, bank);
        if let Some(p) = params {
            engine.handle(Event::Params(p));
        }
        let (producer, consumer) = rtrb::RingBuffer::new(4096);
        let status = Arc::new(Status::default());
        let st = status.clone();
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => run::<f32>(&device, config, engine, consumer, st),
            cpal::SampleFormat::I16 => run::<i16>(&device, config, engine, consumer, st),
            cpal::SampleFormat::I32 => run::<i32>(&device, config, engine, consumer, st),
            cpal::SampleFormat::U16 => run::<u16>(&device, config, engine, consumer, st),
            f => return Err(format!("{device}: unsupported sample format {f}")),
        }?;
        stream.play().map_err(|e| e.to_string())?;
        let sender = Sender(Arc::new(Mutex::new(producer)));
        let (midi, midi_ports) = connect_midi(o.midi.as_deref(), &sender);
        Ok(Live {
            _stream: stream,
            _midi: midi,
            sender,
            status,
            device: device.to_string(),
            sample_rate,
            midi_ports,
        })
    }

    /// Switch to another voice, or rebuild this one's bank with other
    /// options. The bank is built on a background thread, never the audio thread.
    pub fn load(&self, profile: Arc<SpeakerProfile>, params: Params, options: BankOptions) {
        let (sender, rate) = (self.sender.clone(), self.sample_rate as f64);
        std::thread::spawn(move || {
            let bank = Arc::new(Bank::with_options(&profile, rate, options));
            sender.send(Event::Bank(bank));
            sender.send(Event::Params(params));
        });
    }
}

fn connect_midi(
    filter: Option<&str>,
    sender: &Sender,
) -> (Vec<midir::MidiInputConnection<()>>, Vec<String>) {
    let mut conns = Vec::new();
    let mut names = Vec::new();
    if filter == Some("none") {
        return (conns, names);
    }
    let Ok(probe) = midir::MidiInput::new("tablemaker") else {
        return (conns, names);
    };
    for port in probe.ports() {
        let Ok(name) = probe.port_name(&port) else {
            continue;
        };
        let wanted = match filter {
            Some(f) => name.to_lowercase().contains(&f.to_lowercase()),
            // Skip the system's loopback ports and our own.
            None => !name.contains("Through") && !name.contains("tablemaker"),
        };
        if !wanted {
            continue;
        }
        let Ok(input) = midir::MidiInput::new("tablemaker") else {
            continue;
        };
        let s = sender.clone();
        if let Ok(c) = input.connect(
            &port,
            "tablemaker-in",
            move |_, msg, _| {
                if let Some(e) = Event::from_midi(msg) {
                    s.send(e);
                }
            },
            (),
        ) {
            conns.push(c);
            names.push(name);
        }
    }
    (conns, names)
}

fn run<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut engine: Engine,
    mut events: rtrb::Consumer<Event>,
    status: Arc<Status>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let mut mono = vec![0.0f32; 8192];
    device
        .build_output_stream(
            config,
            move |out: &mut [T], _| {
                while let Ok(e) = events.pop() {
                    engine.handle(e);
                }
                let frames = out.len() / channels;
                if mono.len() < frames {
                    mono.resize(frames, 0.0); // only if the host asks for an unusually large buffer
                }
                engine.process(&mut mono[..frames]);
                let mut peak = 0.0f32;
                for (frame, &v) in out.chunks_mut(channels).zip(&mono[..frames]) {
                    peak = peak.max(v.abs());
                    frame.iter_mut().for_each(|s| *s = T::from_sample(v));
                }
                status
                    .vowel
                    .store(engine.vowel().to_bits(), Ordering::Relaxed);
                let s = engine.sounding();
                status.sounding_lo.store(s as u64, Ordering::Relaxed);
                status
                    .sounding_hi
                    .store((s >> 64) as u64, Ordering::Relaxed);
                let old = f32::from_bits(status.peak.load(Ordering::Relaxed));
                status
                    .peak
                    .store(old.max(peak).to_bits(), Ordering::Relaxed);
            },
            |e| eprintln!("audio: {e}"),
            None,
        )
        .map_err(|e| format!("{device}: {e}"))
}

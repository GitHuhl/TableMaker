//! `tablemaker play` and `tablemaker audition`.

use crate::live::{self, Live, LiveOptions};
use crate::voice;
use std::path::{Path, PathBuf};
use tablemaker_dsp::speaker::{SpeakerProfile, SpeakerSettings, VoiceType};
use tablemaker_dsp::wav;
use tablemaker_synth::{Engine, Event, Params};

/// A voice file, or recordings to analyse now.
pub fn profile_from(inputs: &[PathBuf], speaker: VoiceType) -> Result<SpeakerProfile, String> {
    let is_json = |p: &Path| {
        p.extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"))
    };
    if inputs.len() == 1 && is_json(&inputs[0]) {
        let text = std::fs::read_to_string(&inputs[0])
            .map_err(|e| format!("{}: {e}", inputs[0].display()))?;
        return SpeakerProfile::from_json(&text)
            .map_err(|e| format!("{}: {e}", inputs[0].display()));
    }
    voice::analyze(
        inputs,
        &SpeakerSettings {
            voice: speaker,
            ..Default::default()
        },
    )
}

/// A short phrase through every vowel near the speaker's own pitch, then a chord.
pub fn demo_score(p: &SpeakerProfile, params: Params) -> (Vec<(f64, Event)>, f64) {
    let base = (69.0 + 12.0 * (p.pitch.median / 440.0).log2()).round() as i32;
    let tune = [0, 2, 4, 5, 7, 5, 4, 2];
    let n = p.vowels.len().max(1);
    let steps = (2 * n).max(8);
    let mut score = vec![(
        0.0,
        Event::Params(Params {
            mono: true,
            ..params
        }),
    )];
    let dur = 0.45;
    let mut t = 0.1;
    for i in 0..steps {
        let note = (base + tune[i % tune.len()]).clamp(24, 100) as u8;
        let pos = if n > 1 {
            (i % (2 * n - 2).max(1)) as f32
        } else {
            0.0
        };
        let pos = if pos > (n - 1) as f32 {
            (2 * (n - 1)) as f32 - pos
        } else {
            pos
        };
        score.push((t, Event::Vowel(pos / (n - 1).max(1) as f32)));
        score.push((t, Event::NoteOn { note, velocity: 96 }));
        score.push((t + dur * 1.05, Event::NoteOff { note }));
        t += dur;
    }
    t += 0.4;
    score.push((
        t,
        Event::Params(Params {
            mono: false,
            ..params
        }),
    ));
    score.push((t, Event::Vowel(0.5)));
    for (k, iv) in [0, 4, 7, 12].iter().enumerate() {
        let note = (base - 12 + iv).clamp(24, 100) as u8;
        score.push((t + 0.05 * k as f64, Event::NoteOn { note, velocity: 80 }));
        score.push((t + 2.2, Event::NoteOff { note }));
    }
    (score, t + 3.0)
}

pub fn audition(profile: &SpeakerProfile, out: &Path, params: Params) -> Result<(), String> {
    let mut engine = Engine::for_profile(profile, 48000.0);
    let (score, secs) = demo_score(profile, params);
    let audio: Vec<f64> = engine
        .render(&score, secs)
        .iter()
        .map(|&v| v as f64)
        .collect();
    wav::write(out, &audio, 48000, wav::Encoding::Pcm24)
        .map_err(|e| format!("{}: {e}", out.display()))?;
    eprintln!("wrote {} ({secs:.1} s)", out.display());
    Ok(())
}

pub fn play(profile: &SpeakerProfile, params: Params, o: &LiveOptions) -> Result<(), String> {
    let live = Live::start(profile, Some(params), o)?;
    eprintln!("playing on {} at {} Hz", live.device, live.sample_rate);
    if live.midi_ports.is_empty() {
        eprintln!(
            "no MIDI input connected (available: {:?})",
            live::midi_inputs()
        );
    } else {
        eprintln!("MIDI from {}", live.midi_ports.join(", "));
    }
    eprintln!(
        "vowels on the mod wheel: {}",
        profile
            .vowels
            .iter()
            .map(|v| v.label.as_str())
            .collect::<Vec<_>>()
            .join(" → ")
    );
    eprintln!("press Enter to stop");
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    live.sender.send(Event::AllOff);
    std::thread::sleep(std::time::Duration::from_millis(300));
    Ok(())
}

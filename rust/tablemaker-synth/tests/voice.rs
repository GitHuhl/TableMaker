//! The synthesizer sings with the recorded vowels' formants at any pitch.

use tablemaker_dsp::formant::{self, FormantSettings};
use tablemaker_dsp::pitch::{self, PitchSettings};
use tablemaker_dsp::speaker::{self, SpeakerProfile, SpeakerSettings};
use tablemaker_dsp::testsignal::Voice;
use tablemaker_dsp::wav::Audio;
use tablemaker_synth::{Engine, Event};

const SR: f64 = 44100.0;
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

fn profile(vowels: &[(&str, &[(f64, f64)])]) -> SpeakerProfile {
    let clips: Vec<(String, Audio)> = vowels
        .iter()
        .enumerate()
        .map(|(i, (name, f))| {
            let v = Voice {
                f0: 120.0,
                formants: f.to_vec(),
                jitter: 0.003,
                seed: i as u64,
                ..Default::default()
            };
            (
                name.to_string(),
                Audio {
                    sample_rate: SR as u32,
                    samples: v.render(SR as u32, 1.5),
                },
            )
        })
        .collect();
    speaker::analyze(&clips, &SpeakerSettings::default(), &|_, _| {}).unwrap()
}

fn sing(engine: &mut Engine, note: u8, extra: Vec<(f64, Event)>) -> Vec<f64> {
    let mut score = extra;
    score.push((
        0.0,
        Event::NoteOn {
            note,
            velocity: 100,
        },
    ));
    score.push((1.2, Event::NoteOff { note }));
    engine
        .render(&score, 1.6)
        .iter()
        .map(|&v| v as f64)
        .collect()
}

fn median_formants(x: &[f64]) -> (f64, f64) {
    let t = formant::track(
        &x[(0.3 * SR) as usize..(1.1 * SR) as usize],
        SR as u32,
        &FormantSettings {
            max_formant: 5000.0,
            ..Default::default()
        },
    );
    let med = |k: usize| {
        let mut v: Vec<f64> = t
            .frames
            .iter()
            .filter_map(|f| f.get(k))
            .map(|f| f.freq)
            .collect();
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    (med(0), med(1))
}

#[test]
fn plays_in_tune() {
    let p = profile(&[("ah", &AH)]);
    let mut e = Engine::for_profile(&p, SR);
    for note in [45u8, 57, 69] {
        let x = sing(&mut e, note, vec![]);
        let tr = pitch::track(
            &x[(0.2 * SR) as usize..(1.1 * SR) as usize],
            SR as u32,
            &PitchSettings::default(),
        );
        let want = 440.0 * 2f64.powf((note as f64 - 69.0) / 12.0);
        let got = tr.median().unwrap();
        assert!(
            (got / want - 1.0).abs() < 0.01,
            "note {note}: {got} Hz, want {want}"
        );
    }
}

#[test]
fn formants_stay_put_across_pitch() {
    // Recorded at 120 Hz; sung from an octave below the recording's pitch region to well above it.
    let p = profile(&[("ah", &AH)]);
    let rec = (p.vowels[0].formants[0].freq, p.vowels[0].formants[1].freq);
    let mut e = Engine::for_profile(&p, SR);
    for note in [45u8, 52, 60, 64] {
        let (f1, f2) = median_formants(&sing(&mut e, note, vec![]));
        assert!(
            (f1 / rec.0 - 1.0).abs() < 0.12,
            "note {note}: F1 {f1} vs recorded {}",
            rec.0
        );
        assert!(
            (f2 / rec.1 - 1.0).abs() < 0.12,
            "note {note}: F2 {f2} vs recorded {}",
            rec.1
        );
    }
}

#[test]
fn the_mod_wheel_moves_between_vowels() {
    let p = profile(&[("ah", &AH), ("ee", &EE)]);
    let ee_first = p.vowels[0].label == "i";
    let mut e = Engine::for_profile(&p, SR);
    let (_, f2_start) = median_formants(&sing(
        &mut e,
        50,
        vec![(0.0, Event::Control { cc: 1, value: 0 })],
    ));
    let (_, f2_end) = median_formants(&sing(
        &mut e,
        50,
        vec![(0.0, Event::Control { cc: 1, value: 127 })],
    ));
    let (ee, ah) = if ee_first {
        (f2_start, f2_end)
    } else {
        (f2_end, f2_start)
    };
    assert!(ee > 2000.0 && ah < 1600.0, "ee F2 {ee}, ah F2 {ah}");
}

#[test]
fn mono_glides_and_releases_to_silence() {
    let p = profile(&[("ah", &AH)]);
    let mut e = Engine::for_profile(&p, SR);
    let mut params = e.params();
    params.mono = true;
    e.handle(Event::Params(params));
    e.handle(Event::NoteOn {
        note: 60,
        velocity: 100,
    });
    e.handle(Event::NoteOn {
        note: 64,
        velocity: 100,
    });
    let mut buf = vec![0.0f32; 4410];
    e.process(&mut buf);
    assert_eq!(e.sounding().count_ones(), 1);
    e.handle(Event::NoteOff { note: 64 });
    e.handle(Event::NoteOff { note: 60 });
    let mut tail = vec![0.0f32; 44100];
    e.process(&mut tail);
    assert_eq!(e.sounding(), 0);
    assert!(tail[40000..].iter().all(|v| v.abs() < 1e-6));
    assert!(buf.iter().any(|v| v.abs() > 0.01));
}

#[test]
fn reads_midi() {
    assert!(matches!(
        Event::from_midi(&[0x91, 60, 100]),
        Some(Event::NoteOn {
            note: 60,
            velocity: 100
        })
    ));
    assert!(matches!(
        Event::from_midi(&[0x90, 60, 0]),
        Some(Event::NoteOff { note: 60 })
    ));
    assert!(matches!(
        Event::from_midi(&[0xB0, 1, 64]),
        Some(Event::Control { cc: 1, value: 64 })
    ));
    match Event::from_midi(&[0xE0, 0, 64]) {
        Some(Event::PitchBend(b)) => assert!(b.abs() < 1e-6),
        _ => panic!(),
    }
}

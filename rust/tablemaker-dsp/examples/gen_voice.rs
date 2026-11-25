//! Writes a few synthetic vowels for trying the voice analysis by hand.
use tablemaker_dsp::{testsignal::Voice, wav};
fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("output directory"));
    let vowels = [
        (
            "ah",
            vec![
                (770.0, 90.0),
                (1330.0, 110.0),
                (2440.0, 160.0),
                (3400.0, 250.0),
            ],
        ),
        (
            "ee",
            vec![
                (300.0, 60.0),
                (2250.0, 120.0),
                (2950.0, 170.0),
                (3600.0, 250.0),
            ],
        ),
        (
            "oo",
            vec![
                (320.0, 60.0),
                (870.0, 90.0),
                (2250.0, 150.0),
                (3300.0, 250.0),
            ],
        ),
        (
            "eh",
            vec![
                (580.0, 80.0),
                (1800.0, 110.0),
                (2500.0, 160.0),
                (3500.0, 250.0),
            ],
        ),
    ];
    for (i, (name, f)) in vowels.iter().enumerate() {
        let v = Voice {
            f0: 110.0 + 8.0 * i as f64,
            formants: f.clone(),
            jitter: 0.005,
            breath: 0.01,
            vibrato: Some((5.2, 25.0)),
            seed: i as u64,
        };
        let sub = if i % 2 == 0 {
            dir.clone()
        } else {
            dir.join("sub")
        };
        std::fs::create_dir_all(&sub).unwrap();
        wav::write(
            &sub.join(format!("{name}.wav")),
            &v.render(44100, 2.0),
            44100,
            wav::Encoding::Pcm24,
        )
        .unwrap();
    }
}

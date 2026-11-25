//! Prints level, pitch and formants over time of a WAV (for checking renders by hand).
use tablemaker_dsp::{formant, pitch, wav};
fn main() {
    let path = std::env::args().nth(1).expect("wav file");
    let a = wav::read(std::path::Path::new(&path)).unwrap();
    let sr = a.sample_rate;
    let pt = pitch::track(&a.samples, sr, &Default::default());
    let ft = formant::track(
        &a.samples,
        sr,
        &formant::FormantSettings {
            max_formant: 5000.0,
            ..Default::default()
        },
    );
    let peak = a.samples.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    println!(
        "{:.1} s, peak {:.2}",
        a.samples.len() as f64 / sr as f64,
        peak
    );
    let mut t = 0.2;
    while t < a.samples.len() as f64 / sr as f64 {
        let f0 = pt
            .at(t * sr as f64)
            .map_or("   -".into(), |f| format!("{f:4.0}"));
        let f: Vec<String> = ft
            .at(t)
            .iter()
            .take(3)
            .map(|f| format!("{:5.0}", f.freq))
            .collect();
        println!("{t:5.2}s  f0 {f0}  F {}", f.join(" "));
        t += 0.45;
    }
}

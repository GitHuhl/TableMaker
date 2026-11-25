//! End to end: two clips of one "instrument" in, a wavetable out.

use std::f64::consts::PI;
use std::path::Path;
use std::process::Command;
use tablemaker_dsp::wav;

/// A tone whose brightness rises over the clip, with a little vibrato.
fn clip(path: &Path, f0: f64, sr: u32, secs: f64, bright: (f64, f64)) {
    let n = (secs * sr as f64) as usize;
    let mut phase = 0.0;
    let samples: Vec<f64> = (0..n)
        .map(|i| {
            let t = i as f64 / sr as f64;
            let b = bright.0 + (bright.1 - bright.0) * t / secs;
            phase += 2.0 * PI * f0 * (1.0 + 0.003 * (2.0 * PI * 5.0 * t).sin()) / sr as f64;
            0.5 * (1..12)
                .map(|h| b.powi(h - 1) * (h as f64 * phase).sin() / h as f64)
                .sum::<f64>()
        })
        .collect();
    wav::write(path, &samples, sr, wav::Encoding::Pcm24).unwrap();
}

#[test]
fn builds_a_table_from_two_clips() {
    let dir = std::env::temp_dir().join(format!("tablemaker-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.wav");
    let b = dir.join("b.wav");
    clip(&a, 146.83, 44100, 0.8, (0.2, 0.6));
    clip(&b, 147.5, 48000, 0.8, (0.6, 0.95));
    let out = dir.join("table.wav");
    let preview = dir.join("preview.wav");
    let report = dir.join("report.json");
    let cycles = dir.join("cycles");
    let status = Command::new(env!("CARGO_BIN_EXE_tablemaker"))
        .args([
            "build",
            "-f",
            "16",
            "--frame-size",
            "1024",
            "--order",
            "path",
            "-o",
        ])
        .arg(&out)
        .arg("--preview")
        .arg(&preview)
        .arg("--report")
        .arg(&report)
        .arg("--cycles-dir")
        .arg(&cycles)
        .arg(&a)
        .arg(&b)
        .status()
        .unwrap();
    assert!(status.success());

    let bytes = std::fs::read(&out).unwrap();
    assert_eq!(wav::clm_frame_size(&bytes), Some(1024));
    let table = wav::parse(&bytes).unwrap();
    assert_eq!(table.samples.len(), 16 * 1024);
    let peak = table.samples.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    assert!((peak - 0.99).abs() < 1e-3);
    // Frames loop cleanly: the jump from each frame's end to its start is like any other step.
    for f in table.samples.chunks(1024) {
        let wrap = (f[0] - f[1023]).abs();
        let typical = f
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f64::max);
        assert!(wrap <= typical * 1.05, "wrap {wrap} vs max step {typical}");
    }
    // The path ordering runs dark to bright: the last frame has the most upper harmonics.
    let upper = |f: &[f64]| -> f64 {
        let d: f64 = f.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
        d / f.iter().map(|v| v * v).sum::<f64>()
    };
    let frames: Vec<&[f64]> = table.samples.chunks(1024).collect();
    assert!(upper(frames[15]) > 2.0 * upper(frames[0]));

    assert_eq!(wav::read(&preview).unwrap().samples.len(), 4 * 48000);
    let json = std::fs::read_to_string(&report).unwrap();
    assert!(json.contains("\"a.wav\"") && json.contains("\"b.wav\""));
    assert_eq!(std::fs::read_dir(&cycles).unwrap().count(), 16);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn fails_cleanly_without_cycles() {
    let dir = std::env::temp_dir().join(format!("tablemaker-silent-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("silence.wav");
    wav::write(&a, &vec![0.0; 44100], 44100, wav::Encoding::Pcm16).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_tablemaker"))
        .args(["build", "-o"])
        .arg(dir.join("t.wav"))
        .arg(&a)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no usable cycles"));
    std::fs::remove_dir_all(&dir).unwrap();
}

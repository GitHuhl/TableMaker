//! `tablemaker voice`: analyse a speaker and write their voice file.

use std::io::Write;
use std::path::{Path, PathBuf};
use tablemaker_dsp::completion::FormantKind;
use tablemaker_dsp::speaker::{self, SpeakerProfile, SpeakerSettings};
use tablemaker_dsp::{inputs, wav};

/// Read every recording named or inside the named directories.
pub fn load(paths: &[PathBuf]) -> Result<Vec<(String, wav::Audio)>, String> {
    let files = inputs::collect(paths).map_err(|e| e.to_string())?;
    if files.is_empty() {
        return Err("no WAV files found".into());
    }
    let common = common_parent(&files);
    files
        .iter()
        .map(|f| {
            let audio = wav::read(f).map_err(|e| format!("{}: {e}", f.display()))?;
            let name = f.strip_prefix(&common).unwrap_or(f).display().to_string();
            Ok((name, audio))
        })
        .collect()
}

fn common_parent(files: &[PathBuf]) -> PathBuf {
    let mut common: PathBuf = files[0].parent().map(Path::to_path_buf).unwrap_or_default();
    for f in &files[1..] {
        while !f.starts_with(&common) {
            if !common.pop() {
                return PathBuf::new();
            }
        }
    }
    common
}

/// Analyse with a progress line on stderr.
pub fn analyze(paths: &[PathBuf], s: &SpeakerSettings) -> Result<SpeakerProfile, String> {
    let clips = load(paths)?;
    let secs: f64 = clips
        .iter()
        .map(|(_, a)| a.samples.len() as f64 / a.sample_rate as f64)
        .sum();
    eprintln!("analysing {} recordings ({secs:.1} s)", clips.len());
    let p = speaker::analyze(&clips, s, &|f, what| {
        eprint!("\r\x1b[K{:3.0}%  {what}", f * 100.0);
        let _ = std::io::stderr().flush();
    });
    eprintln!();
    p
}

/// A readable summary of a profile.
pub fn summary(p: &SpeakerProfile) -> String {
    let mut o = String::new();
    let secs: f64 = p.recordings.iter().map(|r| r.duration).sum();
    let voiced: f64 = p.recordings.iter().map(|r| r.duration * r.voiced).sum();
    let who = match p.voice {
        speaker::VoiceType::Auto => "",
        speaker::VoiceType::Man => " (man)",
        speaker::VoiceType::Woman => " (woman)",
        speaker::VoiceType::Child => " (child)",
    };
    o += &format!(
        "Speaker{who}   {} recordings, {secs:.1} s, {:.0}% voiced\n",
        p.recordings.len(),
        100.0 * voiced / secs.max(1e-9)
    );
    o += &format!(
        "Pitch     {:.0} Hz median, range {:.0}-{:.0} Hz\n",
        p.pitch.median, p.pitch.low, p.pitch.high
    );
    o += &format!(
        "Voice     jitter {:.2}%, shimmer {:.1}%, HNR {:.1} dB, H1-H2 {:.1} dB, level {:.0} dBFS\n",
        p.jitter, p.shimmer, p.hnr, p.h1_h2, p.level
    );
    o += &match p.vibrato {
        Some(v) => format!(
            "Vibrato   {:.1} Hz, ±{:.0} cents, in {:.0}% of sustained notes\n",
            v.rate,
            v.extent,
            v.presence * 100.0
        ),
        None => "Vibrato   none\n".into(),
    };
    o += &format!(
        "\nVowel table ({} entries, in morph order)\n",
        p.vowels.len()
    );
    o += "   #  vowel     F1     F2     F3     F4  share  pitch   HNR  heard in\n";
    for (i, v) in p.vowels.iter().enumerate() {
        let f = |k: usize| {
            v.formants.get(k).map_or("     -".to_string(), |f| {
                let mark = match v.formant_kinds.get(k) {
                    Some(FormantKind::Found) => "+",
                    Some(FormantKind::Inferred) => "~",
                    _ => " ",
                };
                format!("{mark}{:5.0}", f.freq)
            })
        };
        let src = v
            .sources
            .first()
            .map_or(String::new(), |s| format!("{} at {:.2} s", s.file, s.time));
        o += &format!(
            "{:4}  {:<5} {} {} {} {} {:5.1}% {:4.0}Hz {:4.0}  {src}\n",
            i + 1,
            v.label,
            f(0),
            f(1),
            f(2),
            f(3),
            v.share * 100.0,
            v.f0,
            v.hnr
        );
    }
    let kinds = || p.vowels.iter().flat_map(|v| &v.formant_kinds);
    if kinds().any(|k| *k == FormantKind::Found) {
        o += "  + found in the vowel's spectrum where the speaker's other vowels put it\n";
    }
    if kinds().any(|k| *k == FormantKind::Inferred) {
        o +=
            "  ~ inferred: not in the recording (restored in the instrument unless --no-restore)\n";
    }
    let low: Vec<String> = p
        .recordings
        .iter()
        .filter(|r| r.band > 0.0 && r.band < 18000.0)
        .map(|r| format!("{} ({:.1} kHz)", r.file, r.band / 1000.0))
        .collect();
    if !low.is_empty() {
        o += &format!(
            "\nBelow full band, restored above it in the instrument: {}\n",
            low.join(", ")
        );
    }
    o
}

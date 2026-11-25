//! `tablemaker`: turn recordings of a voice into an instrument.

mod voice;

use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::process::ExitCode;
use tablemaker_dsp::cycles::{self, ClipAnalysis, ExtractSettings};
use tablemaker_dsp::pitch::PitchSettings;
use tablemaker_dsp::speaker::{SpeakerSettings, VoiceType};
use tablemaker_dsp::table::{self, Morph, Normalize, Order, Select, TableSettings};
use tablemaker_dsp::{preview, report, wav};

#[derive(Parser)]
#[command(name = "tablemaker", version, about = "Turn recordings of a voice into an instrument", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Analyse a speaker (files and/or directories) and write their voice file:
    /// pitch, voice quality and the vowel table the instrument plays.
    Voice(VoiceArgs),
    /// Extract cycles from the clips and mash them into one wavetable.
    Build(BuildArgs),
    /// Report pitch and how many usable cycles each clip has, without writing a table.
    Analyze {
        /// WAV files and/or directories of them (any channel count, PCM or float).
        #[arg(required = true)]
        clips: Vec<PathBuf>,
        #[command(flatten)]
        extract: ExtractArgs,
    },
}

#[derive(Args)]
struct VoiceArgs {
    /// WAV files and/or directories of them (searched recursively).
    #[arg(required = true)]
    inputs: Vec<PathBuf>,
    /// The voice file to write.
    #[arg(short, long, default_value = "voice.json")]
    output: PathBuf,
    /// Who is speaking: sets the pitch search range, how high formants are looked
    /// for, and the reference vowels entries are named after.
    #[arg(short, long, value_enum, default_value_t = SpeakerArg::Auto)]
    speaker: SpeakerArg,
    /// Most vowel entries in the table (similar ones are merged).
    #[arg(long, default_value_t = 12)]
    vowels: usize,
    /// Lowest pitch of the voice (Hz). Default: from --speaker.
    #[arg(long)]
    fmin: Option<f64>,
    /// Highest pitch of the voice (Hz). Default: from --speaker.
    #[arg(long)]
    fmax: Option<f64>,
    /// Highest formant to look for (Hz). Default: from --speaker (5000 man, 5500 woman,
    /// 8000 child; for auto, chosen from the voice's pitch).
    #[arg(long)]
    max_formant: Option<f64>,
}

#[derive(Args)]
struct ExtractArgs {
    /// Oversampling factor for locating zero crossings.
    #[arg(long, default_value_t = 16, value_parser = clap::value_parser!(u32).range(1..=64))]
    oversample: u32,
    /// Lowest pitch to track (Hz).
    #[arg(long, default_value_t = 30.0)]
    fmin: f64,
    /// Highest pitch to track (Hz).
    #[arg(long, default_value_t = 2000.0)]
    fmax: f64,
    /// Skip pitch tracking: the clips are at this pitch (Hz).
    #[arg(long)]
    f0: Option<f64>,
    /// How far (fraction of a period) a crossing pair may stray from the tracked period.
    #[arg(long, default_value_t = 0.03)]
    tolerance: f64,
    /// Lowest cycle score kept, 0..1: periodicity weighted by loop-point slope match.
    #[arg(long, default_value_t = 0.9)]
    min_score: f64,
    /// Drop cycles this many dB below a clip's loudest cycle.
    #[arg(long, default_value_t = 40.0)]
    gate: f64,
}

#[derive(Args)]
struct BuildArgs {
    /// WAV files and/or directories of them, all of one source.
    #[arg(required = true)]
    clips: Vec<PathBuf>,
    /// The wavetable to write: `.wt` for Surge XT's own format, otherwise a WAV with the
    /// frames back to back and a `clm ` chunk (Vital, Surge XT, Bitwig and most others).
    #[arg(short, long)]
    output: PathBuf,
    /// Frames in the table.
    #[arg(short, long, default_value_t = 64)]
    frames: usize,
    /// Samples per frame (2048 is the common choice; `.wt` takes powers of two up to 4096).
    #[arg(long, default_value_t = 2048)]
    frame_size: usize,
    /// Distinct cycles to use; the frames between them are morphed. Default: one per frame.
    #[arg(short, long)]
    keyframes: Option<usize>,
    /// Which cycles become keyframes.
    #[arg(long, value_enum, default_value_t = SelectArg::Spread)]
    select: SelectArg,
    /// How keyframes are ordered across the table.
    #[arg(long, value_enum, default_value_t = OrderArg::Time)]
    order: OrderArg,
    /// How the frames between keyframes are made.
    #[arg(long, value_enum, default_value_t = MorphArg::Crossfade)]
    morph: MorphArg,
    /// Do not rotate frames to line up with their neighbours.
    #[arg(long)]
    no_align: bool,
    /// Average each frame with this many neighbours on each side.
    #[arg(long, default_value_t = 0)]
    smooth: usize,
    #[arg(long, value_enum, default_value_t = NormalizeArg::Table)]
    normalize: NormalizeArg,
    /// Sample format of the written files.
    #[arg(long, value_enum, default_value_t = FormatArg::Float32)]
    format: FormatArg,
    /// Also render an audition: the table played at --preview-note, sweeping every frame.
    #[arg(long)]
    preview: Option<PathBuf>,
    /// MIDI note of the preview.
    #[arg(long, default_value_t = 48)]
    preview_note: u8,
    /// Length of the preview (seconds).
    #[arg(long, default_value_t = 4.0)]
    preview_seconds: f64,
    /// Also write every frame as a single-cycle WAV into this directory.
    #[arg(long)]
    cycles_dir: Option<PathBuf>,
    /// Also write a JSON report: what each clip held and where every keyframe came from.
    #[arg(long)]
    report: Option<PathBuf>,
    #[command(flatten)]
    extract: ExtractArgs,
}

#[derive(Clone, Copy, ValueEnum)]
enum SpeakerArg {
    /// Decide from the recordings.
    Auto,
    Man,
    Woman,
    Child,
}

#[derive(Clone, Copy, ValueEnum)]
enum SelectArg {
    /// Evenly through the clips in order (the table follows their timeline).
    Spread,
    /// The cleanest, most periodic cycles.
    Best,
    /// The most different-sounding cycles.
    Diverse,
}

#[derive(Clone, Copy, ValueEnum)]
enum OrderArg {
    /// Clip order, then time.
    Time,
    /// Darkest to brightest.
    Brightness,
    /// Shortest path through spectral space: each frame beside its closest relatives.
    Path,
}

#[derive(Clone, Copy, ValueEnum)]
enum MorphArg {
    /// Crossfade the aligned waveforms.
    Crossfade,
    /// Interpolate harmonic amplitudes and phases (no dips between keyframes).
    Spectral,
}

#[derive(Clone, Copy, ValueEnum)]
enum NormalizeArg {
    Table,
    Frame,
    None,
}

#[derive(Clone, Copy, ValueEnum)]
enum FormatArg {
    Float32,
    Pcm16,
    Pcm24,
}

impl ExtractArgs {
    fn settings(&self, frame_size: usize) -> ExtractSettings {
        ExtractSettings {
            oversample: self.oversample as usize,
            frame_size,
            tolerance: self.tolerance,
            min_score: self.min_score,
            gate_db: self.gate,
            pitch: PitchSettings {
                fmin: self.fmin,
                fmax: self.fmax,
                ..Default::default()
            },
            fixed_f0: self.f0,
        }
    }
}

fn analyze(paths: &[PathBuf], s: &ExtractSettings) -> Result<Vec<(String, ClipAnalysis)>, String> {
    let mut out = Vec::new();
    for (i, (name, audio)) in voice::load(paths)?.into_iter().enumerate() {
        let a = cycles::extract(&audio, i, s);
        eprintln!(
            "{}: {:.2} s at {} Hz, pitch {}, voiced {:.0}%, {} rising crossings, {} candidate cycles, {} kept",
            name,
            a.duration,
            a.sample_rate,
            a.median_f0.map_or("none".into(), |f| format!("{f:.2} Hz")),
            a.voiced_fraction * 100.0,
            a.crossings,
            a.candidates,
            a.cycles.len()
        );
        out.push((name, a));
    }
    Ok(out)
}

fn build(b: &BuildArgs) -> Result<(), String> {
    if b.frame_size < 16 || b.frame_size > 65536 {
        return Err("--frame-size must be between 16 and 65536".into());
    }
    if b.frames == 0 {
        return Err("--frames must be at least 1".into());
    }
    let clips = analyze(&b.clips, &b.extract.settings(b.frame_size))?;
    let all: Vec<_> = clips
        .iter()
        .flat_map(|(_, a)| a.cycles.iter().cloned())
        .collect();
    if all.is_empty() {
        return Err(
            "no usable cycles found: try --f0, a wider --fmin/--fmax, a lower --min-score or a larger --tolerance"
                .into(),
        );
    }
    let settings = TableSettings {
        frames: b.frames,
        keyframes: b.keyframes,
        select: match b.select {
            SelectArg::Spread => Select::Spread,
            SelectArg::Best => Select::Best,
            SelectArg::Diverse => Select::Diverse,
        },
        order: match b.order {
            OrderArg::Time => Order::Time,
            OrderArg::Brightness => Order::Brightness,
            OrderArg::Path => Order::Path,
        },
        morph: match b.morph {
            MorphArg::Crossfade => Morph::Crossfade,
            MorphArg::Spectral => Morph::Spectral,
        },
        align: !b.no_align,
        smooth: b.smooth,
        normalize: match b.normalize {
            NormalizeArg::Table => Normalize::Table,
            NormalizeArg::Frame => Normalize::Frame,
            NormalizeArg::None => Normalize::None,
        },
        ..Default::default()
    };
    let t = table::build(&all, &settings).ok_or("nothing to build")?;
    if t.keyframes.len() < b.keyframes.unwrap_or(b.frames).min(b.frames) {
        eprintln!(
            "only {} usable cycles: morphing between them to fill {} frames",
            t.keyframes.len(),
            b.frames
        );
    }
    let encoding = match b.format {
        FormatArg::Float32 => wav::Encoding::Float32,
        FormatArg::Pcm16 => wav::Encoding::Pcm16,
        FormatArg::Pcm24 => wav::Encoding::Pcm24,
    };
    const TABLE_RATE: u32 = 44100;
    let is_wt = b
        .output
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("wt"));
    if is_wt {
        let bytes = wav::build_wt(&t.frames, encoding)?;
        std::fs::write(&b.output, bytes).map_err(|e| format!("{}: {e}", b.output.display()))?;
    } else {
        wav::write_wavetable(&b.output, &t.frames, TABLE_RATE, encoding)
            .map_err(|e| format!("{}: {e}", b.output.display()))?;
    }
    eprintln!(
        "wrote {}: {} frames of {} samples from {} cycles",
        b.output.display(),
        t.frames.len(),
        b.frame_size,
        t.keyframes.len()
    );

    if let Some(p) = &b.preview {
        let freq = 440.0 * 2f64.powf((b.preview_note as f64 - 69.0) / 12.0);
        let audio = preview::render(&t.frames, freq, 48000, b.preview_seconds.max(0.1));
        wav::write(p, &audio, 48000, encoding).map_err(|e| format!("{}: {e}", p.display()))?;
        eprintln!("wrote {} (preview at {freq:.2} Hz)", p.display());
    }
    if let Some(dir) = &b.cycles_dir {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let stem = b
            .output
            .file_stem()
            .map_or("table".into(), |s| s.to_string_lossy().into_owned());
        for (i, f) in t.frames.iter().enumerate() {
            let p = dir.join(format!("{stem}-{i:03}.wav"));
            wav::write(&p, f, TABLE_RATE, encoding).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        eprintln!(
            "wrote {} single cycles to {}",
            t.frames.len(),
            dir.display()
        );
    }
    if let Some(p) = &b.report {
        std::fs::write(p, report::json(&clips, &t)).map_err(|e| format!("{}: {e}", p.display()))?;
        eprintln!("wrote {}", p.display());
    }
    Ok(())
}

fn make_voice(v: &VoiceArgs) -> Result<(), String> {
    let s = SpeakerSettings {
        voice: match v.speaker {
            SpeakerArg::Auto => VoiceType::Auto,
            SpeakerArg::Man => VoiceType::Man,
            SpeakerArg::Woman => VoiceType::Woman,
            SpeakerArg::Child => VoiceType::Child,
        },
        vowels: v.vowels,
        fmin: v.fmin,
        fmax: v.fmax,
        max_formant: v.max_formant,
        ..Default::default()
    };
    let p = voice::analyze(&v.inputs, &s)?;
    print!("{}", voice::summary(&p));
    std::fs::write(&v.output, p.to_json()).map_err(|e| format!("{}: {e}", v.output.display()))?;
    eprintln!("wrote {}", v.output.display());
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match &cli.command {
        Command::Voice(v) => make_voice(v),
        Command::Build(b) => build(b),
        Command::Analyze { clips, extract } => analyze(clips, &extract.settings(2048)).map(|_| ()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("tablemaker: {e}");
            ExitCode::FAILURE
        }
    }
}

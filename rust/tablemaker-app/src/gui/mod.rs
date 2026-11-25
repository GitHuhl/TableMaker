//! The TableMaker window: drop in recordings of a voice, get an instrument.

mod keys;
mod pad;
mod png;
mod theme;

use crate::live::{Live, LiveOptions};
use crate::perform;
use eframe::egui::{self, Color32, Margin, RichText};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tablemaker_dsp::speaker::{self, SpeakerProfile, SpeakerSettings, VoiceType};
use tablemaker_dsp::wav;
use tablemaker_synth::{Bank, Event, Params};

struct Job {
    progress: Arc<Mutex<(f64, String)>>,
    result: Receiver<Result<SpeakerProfile, String>>,
}

pub struct App {
    live: Option<Live>,
    audio_error: Option<String>,
    profile: Option<Arc<SpeakerProfile>>,
    params: Params,
    speaker: VoiceType,
    inputs: Vec<PathBuf>,
    job: Option<Job>,
    dialog: Option<Receiver<DialogResult>>,
    toast: Option<(String, Instant)>,
    selected: Option<usize>,
    /// Base C of the computer-keyboard octave.
    computer_base: u8,
    computer_down: Vec<(egui::Key, u8)>,
    mouse_note: Option<u8>,
    pad_note: Option<u8>,
    screenshot: Option<(PathBuf, u32)>,
}

enum DialogResult {
    Recordings(Vec<PathBuf>),
    Voice(PathBuf),
    SaveVoice(PathBuf),
    Audition(PathBuf),
    Wavetable(PathBuf),
}

/// Open the window, optionally with recordings or a voice file.
pub fn run(inputs: Vec<PathBuf>, speaker: VoiceType) -> Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("TableMaker")
            .with_app_id("tablemaker")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([760.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "TableMaker",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            let mut app = App::new(speaker);
            if !inputs.is_empty() {
                app.open(inputs, &cc.egui_ctx);
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| e.to_string())
}

impl App {
    fn new(speaker: VoiceType) -> Self {
        let screenshot = std::env::var_os("TABLEMAKER_SCREENSHOT").map(|p| (PathBuf::from(p), 0));
        App {
            live: None,
            audio_error: None,
            profile: None,
            params: Params::default(),
            speaker,
            inputs: Vec::new(),
            job: None,
            dialog: None,
            toast: None,
            selected: None,
            computer_base: 48,
            computer_down: Vec::new(),
            mouse_note: None,
            pad_note: None,
            screenshot,
        }
    }

    fn say(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    fn send(&self, e: Event) {
        if let Some(l) = &self.live {
            l.sender.send(e);
        }
    }

    /// Recordings to analyse, or a voice file to load.
    fn open(&mut self, inputs: Vec<PathBuf>, ctx: &egui::Context) {
        let is_voice = inputs.len() == 1
            && inputs[0]
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("json"));
        if is_voice {
            match std::fs::read_to_string(&inputs[0])
                .map_err(|e| e.to_string())
                .and_then(|t| SpeakerProfile::from_json(&t))
            {
                Ok(p) => {
                    self.speaker = p.voice;
                    self.inputs.clear();
                    self.set_profile(p);
                }
                Err(e) => self.say(format!("Could not open the voice: {e}")),
            }
            return;
        }
        self.inputs = inputs;
        self.analyse(ctx);
    }

    fn analyse(&mut self, ctx: &egui::Context) {
        if self.inputs.is_empty() {
            return;
        }
        let progress = Arc::new(Mutex::new((0.0, "reading recordings".to_string())));
        let (tx, rx) = mpsc::channel();
        let (inputs, voice, p2, ctx2) = (
            self.inputs.clone(),
            self.speaker,
            progress.clone(),
            ctx.clone(),
        );
        std::thread::spawn(move || {
            let result = crate::voice::load(&inputs).and_then(|clips| {
                let settings = SpeakerSettings {
                    voice,
                    ..Default::default()
                };
                speaker::analyze(&clips, &settings, &|f, what| {
                    *p2.lock().unwrap() = (f, what.to_string());
                    ctx2.request_repaint();
                })
            });
            let _ = tx.send(result);
            ctx2.request_repaint();
        });
        self.job = Some(Job {
            progress,
            result: rx,
        });
    }

    fn set_profile(&mut self, p: SpeakerProfile) {
        let note = 69.0 + 12.0 * (p.pitch.median / 440.0).log2();
        self.computer_base = ((note.round() as i32 / 12) * 12).clamp(24, 84) as u8;
        self.params = Params::from_profile(&p);
        self.selected = None;
        match &self.live {
            Some(l) => l.load(&p, self.params),
            None => match Live::start(&p, Some(self.params), &LiveOptions::default()) {
                Ok(l) => {
                    self.audio_error = None;
                    self.live = Some(l);
                }
                Err(e) => self.audio_error = Some(e),
            },
        }
        let vowels: Vec<&str> = p.vowels.iter().map(|v| v.label.as_str()).collect();
        self.say(format!(
            "{} vowels found: {}",
            vowels.len(),
            vowels.join(" ")
        ));
        self.profile = Some(Arc::new(p));
    }

    fn dialog(
        &mut self,
        f: impl FnOnce() -> Option<DialogResult> + Send + 'static,
        ctx: &egui::Context,
    ) {
        if self.dialog.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            if let Some(r) = f() {
                let _ = tx.send(r);
            }
            ctx.request_repaint();
        });
        self.dialog = Some(rx);
    }

    fn handle_dialog(&mut self, r: DialogResult, ctx: &egui::Context) {
        let profile = self.profile.clone();
        let result = match r {
            DialogResult::Recordings(paths) => {
                self.open(paths, ctx);
                Ok(None)
            }
            DialogResult::Voice(path) => {
                self.open(vec![path], ctx);
                Ok(None)
            }
            DialogResult::SaveVoice(path) => match profile {
                Some(p) => std::fs::write(&path, p.to_json())
                    .map(|_| Some(path))
                    .map_err(|e| e.to_string()),
                None => Ok(None),
            },
            DialogResult::Audition(path) => match profile {
                Some(p) => perform::audition(&p, &path, self.params).map(|_| Some(path)),
                None => Ok(None),
            },
            DialogResult::Wavetable(path) => match profile {
                Some(p) => export_wavetable(&p, &path).map(|_| Some(path)),
                None => Ok(None),
            },
        };
        match result {
            Ok(Some(path)) => self.say(format!("Saved {}", path.display())),
            Ok(None) => {}
            Err(e) => self.say(format!("Could not save: {e}")),
        }
    }

    /// Play the demo phrase through the live voice.
    fn hear_it(&self) {
        let (Some(live), Some(p)) = (&self.live, &self.profile) else {
            return;
        };
        let (score, _) = perform::demo_score(p, self.params);
        let sender = live.sender.clone();
        let params = self.params;
        std::thread::spawn(move || {
            let start = Instant::now();
            let mut score = score;
            score.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (t, e) in score {
                let due = Duration::from_secs_f64(t);
                if let Some(wait) = due.checked_sub(start.elapsed()) {
                    std::thread::sleep(wait);
                }
                sender.send(e);
            }
            std::thread::sleep(Duration::from_millis(2500));
            sender.send(Event::Params(params));
        });
    }

    fn sounding(&self, note: u8) -> bool {
        self.live.as_ref().is_some_and(|l| l.status.sounding(note))
    }

    fn computer_keyboard(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let events = ctx.input(|i| i.events.clone());
        for e in events {
            if let egui::Event::Key {
                key,
                pressed,
                repeat: false,
                ..
            } = e
            {
                if pressed && key == egui::Key::Z {
                    self.computer_base = self.computer_base.saturating_sub(12).max(12);
                } else if pressed && key == egui::Key::X {
                    self.computer_base = (self.computer_base + 12).min(96);
                } else if let Some(&(_, off)) = keys::COMPUTER_KEYS.iter().find(|(k, _)| *k == key)
                {
                    if pressed {
                        let note = self.computer_base + off;
                        self.computer_down.push((key, note));
                        self.send(Event::NoteOn {
                            note,
                            velocity: 100,
                        });
                    } else if let Some(i) = self.computer_down.iter().position(|(k, _)| *k == key) {
                        let (_, note) = self.computer_down.remove(i);
                        self.send(Event::NoteOff { note });
                    }
                }
            }
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("TableMaker")
                    .size(20.0)
                    .strong()
                    .color(theme::TEXT),
            );
            ui.add_space(18.0);
            ui.label(RichText::new("Voice").color(theme::DIM));
            let before = self.speaker;
            for (v, name) in [
                (VoiceType::Auto, "Auto"),
                (VoiceType::Man, "Man"),
                (VoiceType::Woman, "Woman"),
                (VoiceType::Child, "Child"),
            ] {
                if ui.selectable_label(self.speaker == v, name).clicked() {
                    self.speaker = v;
                }
            }
            if self.speaker != before && !self.inputs.is_empty() {
                self.analyse(&ctx);
            }
            ui.add_space(18.0);
            if ui.button("Add recordings…").clicked() {
                self.dialog(
                    || {
                        rfd::FileDialog::new()
                            .add_filter("Audio", &["wav", "wave"])
                            .pick_files()
                            .map(DialogResult::Recordings)
                    },
                    &ctx,
                );
            }
            if ui.button("Add folder…").clicked() {
                self.dialog(
                    || {
                        rfd::FileDialog::new()
                            .pick_folders()
                            .map(DialogResult::Recordings)
                    },
                    &ctx,
                );
            }
            if ui.button("Open voice…").clicked() {
                self.dialog(
                    || {
                        rfd::FileDialog::new()
                            .add_filter("TableMaker voice", &["json"])
                            .pick_file()
                            .map(DialogResult::Voice)
                    },
                    &ctx,
                );
            }
            let have = self.profile.is_some();
            ui.add_enabled_ui(have, |ui| {
                ui.menu_button("Save ▾", |ui| {
                    if ui.button("Voice…").clicked() {
                        self.dialog(
                            || {
                                rfd::FileDialog::new()
                                    .set_file_name("voice.json")
                                    .save_file()
                                    .map(DialogResult::SaveVoice)
                            },
                            &ctx,
                        );
                    }
                    if ui.button("Demo phrase (WAV)…").clicked() {
                        self.dialog(
                            || {
                                rfd::FileDialog::new()
                                    .set_file_name("audition.wav")
                                    .save_file()
                                    .map(DialogResult::Audition)
                            },
                            &ctx,
                        );
                    }
                    if ui.button("Vowels as a wavetable (WAV)…").clicked() {
                        self.dialog(
                            || {
                                rfd::FileDialog::new()
                                    .set_file_name("vowels.wav")
                                    .save_file()
                                    .map(DialogResult::Wavetable)
                            },
                            &ctx,
                        );
                    }
                });
                if ui.button("▶ Hear it").clicked() {
                    self.hear_it();
                }
            });
            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| match &self.live {
                    Some(l) => {
                        let peak = l.status.take_peak();
                        let (r, p) =
                            ui.allocate_painter(egui::vec2(60.0, 8.0), egui::Sense::hover());
                        p.rect_filled(r.rect, 3.0, theme::CARD);
                        let mut lit = r.rect;
                        lit.set_width(r.rect.width() * peak.min(1.0));
                        p.rect_filled(
                            lit,
                            3.0,
                            if peak > 0.95 {
                                Color32::from_rgb(240, 80, 80)
                            } else {
                                theme::ACCENT
                            },
                        );
                        let midi = if l.midi_ports.is_empty() {
                            "no MIDI".to_string()
                        } else {
                            l.midi_ports.join(", ")
                        };
                        ui.label(
                            RichText::new(format!("{} · {}", l.device, midi))
                                .color(theme::DIM)
                                .size(12.0),
                        );
                    }
                    None => match &self.audio_error {
                        Some(e) => {
                            ui.label(
                                RichText::new("● no audio output")
                                    .color(Color32::from_rgb(240, 120, 100))
                                    .size(12.0),
                            )
                            .on_hover_text(e);
                        }
                        None => {
                            ui.label(
                                RichText::new("audio starts when a voice is loaded")
                                    .color(theme::DIM)
                                    .size(12.0),
                            );
                        }
                    },
                },
            );
        });
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        let before = self.params;
        let speaker = self
            .profile
            .as_ref()
            .map(|p| Params::from_profile(p))
            .unwrap_or_default();
        /// One control: a small caption with its value over a slider.
        fn knob(ui: &mut egui::Ui, name: &str, value: String, slider: egui::Slider) -> bool {
            let mut changed = false;
            ui.vertical(|ui| {
                ui.set_width(104.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(name).color(theme::DIM).size(12.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(value).color(theme::TEXT).size(12.0));
                    });
                });
                ui.spacing_mut().slider_width = 104.0;
                changed = ui.add(slider.show_value(false)).changed();
            });
            changed
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            let p = &mut self.params;
            knob(
                ui,
                "Vibrato",
                format!("{:.0}¢", p.vibrato_depth),
                egui::Slider::new(&mut p.vibrato_depth, 0.0..=120.0),
            );
            knob(
                ui,
                "Rate",
                format!("{:.1} Hz", p.vibrato_rate),
                egui::Slider::new(&mut p.vibrato_rate, 3.0..=8.0),
            );
            knob(
                ui,
                "Breath",
                format!("{:.0}%", p.breath * 100.0),
                egui::Slider::new(&mut p.breath, 0.0..=4.0),
            );
            let mut wander = if speaker.jitter > 0.0 {
                p.jitter / speaker.jitter
            } else {
                1.0
            };
            if knob(
                ui,
                "Wander",
                format!("{:.0}%", wander * 100.0),
                egui::Slider::new(&mut wander, 0.0..=3.0),
            ) {
                p.jitter = speaker.jitter * wander;
                p.shimmer = speaker.shimmer * wander;
            }
            knob(
                ui,
                "Attack",
                format!("{:.0} ms", p.attack * 1000.0),
                egui::Slider::new(&mut p.attack, 0.005..=1.0).logarithmic(true),
            );
            knob(
                ui,
                "Release",
                format!("{:.2} s", p.release),
                egui::Slider::new(&mut p.release, 0.02..=3.0).logarithmic(true),
            );
            knob(
                ui,
                "Glide",
                format!("{:.0} ms", p.glide * 1000.0),
                egui::Slider::new(&mut p.glide, 0.0..=0.6),
            );
            knob(
                ui,
                "Volume",
                format!("{:.0}%", p.gain * 100.0),
                egui::Slider::new(&mut p.gain, 0.0..=1.0),
            );
            ui.vertical(|ui| {
                ui.set_min_width(170.0);
                ui.checkbox(&mut p.mono, "Legato (one voice)");
                if ui.button("Reset to the speaker").clicked() {
                    *p = Params {
                        gain: p.gain,
                        mono: p.mono,
                        ..speaker
                    };
                }
            });
        });
        if self.params != before {
            self.send(Event::Params(self.params));
        }
    }

    fn info(&mut self, ui: &mut egui::Ui, p: &SpeakerProfile) {
        let row = |ui: &mut egui::Ui, k: &str, v: String| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(k).color(theme::DIM));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(v).color(theme::TEXT))
                });
            });
        };
        let note = 69.0 + 12.0 * (p.pitch.median / 440.0).log2();
        let names = [
            "C", "C♯", "D", "E♭", "E", "F", "F♯", "G", "A♭", "A", "B♭", "B",
        ];
        let n = note.round() as i32;
        ui.label(RichText::new("Voice").size(15.0).strong());
        ui.label(
            RichText::new(format!(
                "{:.0} Hz · {}{}",
                p.pitch.median,
                names[n.rem_euclid(12) as usize],
                n / 12 - 1
            ))
            .size(26.0)
            .color(theme::TEXT),
        );
        ui.label(
            RichText::new(format!(
                "comfortable {:.0}–{:.0} Hz",
                p.pitch.low, p.pitch.high
            ))
            .color(theme::DIM),
        );
        ui.add_space(6.0);
        let secs: f64 = p.recordings.iter().map(|r| r.duration).sum();
        row(
            ui,
            "Recordings",
            format!("{} · {secs:.1} s", p.recordings.len()),
        );
        row(ui, "Breathiness", format!("HNR {:.0} dB", p.hnr));
        row(ui, "Pitch wander", format!("{:.2}%", p.jitter));
        row(ui, "Level wander", format!("{:.1}%", p.shimmer));
        row(ui, "Tilt (H1–H2)", format!("{:.1} dB", p.h1_h2));
        row(
            ui,
            "Vibrato",
            p.vibrato.map_or("none".into(), |v| {
                format!("{:.1} Hz ±{:.0}¢", v.rate, v.extent)
            }),
        );
        ui.add_space(14.0);
        let Some(i) = self.selected.filter(|&i| i < p.vowels.len()) else {
            ui.label(RichText::new("Press a vowel to hear it and see where it came from. Drag across the pad to morph; play keys (or A–L on the keyboard) to sing.").color(theme::DIM));
            return;
        };
        let v = &p.vowels[i];
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(&v.label)
                    .size(34.0)
                    .color(theme::vowel_color(i, p.vowels.len())),
            );
            ui.label(
                RichText::new(format!(
                    "vowel {} of {}\n{:.0}% of the voice",
                    i + 1,
                    p.vowels.len(),
                    v.share * 100.0
                ))
                .color(theme::DIM),
            );
        });
        for (k, f) in v.formants.iter().enumerate() {
            row(
                ui,
                &format!("F{}", k + 1),
                format!("{:.0} Hz  (±{:.0})", f.freq, f.bw / 2.0),
            );
        }
        row(ui, "Breathiness", format!("HNR {:.0} dB", v.hnr));
        ui.add_space(4.0);
        ui.label(RichText::new("Heard in").color(theme::DIM));
        for s in &v.sources {
            ui.label(RichText::new(format!("{}  {:.2} s", s.file, s.time)).size(12.0));
        }
    }

    fn empty_state(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
        let painter = ui.painter_at(rect);
        let zone = rect.shrink(40.0);
        painter.rect_filled(zone, 18.0, if hovering { theme::CARD } else { theme::BG });
        painter.rect_stroke(
            zone,
            18.0,
            egui::Stroke::new(2.0, if hovering { theme::ACCENT } else { theme::LINE }),
            egui::StrokeKind::Inside,
        );
        let c = zone.center();
        painter.text(
            c - egui::vec2(0.0, 30.0),
            egui::Align2::CENTER_CENTER,
            "Drop recordings of a voice here",
            egui::FontId::proportional(26.0),
            theme::TEXT,
        );
        painter.text(
            c + egui::vec2(0.0, 8.0),
            egui::Align2::CENTER_CENTER,
            "WAV files or whole folders, all of one speaker. Speech or singing.",
            egui::FontId::proportional(15.0),
            theme::DIM,
        );
        painter.text(
            c + egui::vec2(0.0, 32.0),
            egui::Align2::CENTER_CENTER,
            "TableMaker finds their pitch, vowels and voice, and makes an instrument that sings with them.",
            egui::FontId::proportional(15.0),
            theme::DIM,
        );
    }

    fn progress(&self, ui: &mut egui::Ui, job: &Job) {
        let (f, what) = job.progress.lock().unwrap().clone();
        ui.add_space(ui.available_height() * 0.38);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Listening…").size(24.0).color(theme::TEXT));
            ui.add_space(8.0);
            ui.add(
                egui::ProgressBar::new(f as f32)
                    .desired_width(420.0)
                    .show_percentage(),
            );
            ui.label(RichText::new(what).color(theme::DIM));
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Finished work from other threads.
        if let Some(job) = &self.job {
            if let Ok(r) = job.result.try_recv() {
                self.job = None;
                match r {
                    Ok(p) => self.set_profile(p),
                    Err(e) => self.say(e),
                }
            }
        }
        if let Some(rx) = &self.dialog {
            match rx.try_recv() {
                Ok(r) => {
                    self.dialog = None;
                    self.handle_dialog(r, &ctx);
                }
                Err(mpsc::TryRecvError::Disconnected) => self.dialog = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() {
            self.open(dropped, &ctx);
        }
        self.computer_keyboard(&ctx);

        egui::Panel::top("top")
            .frame(
                egui::Frame::NONE
                    .fill(theme::PANEL)
                    .inner_margin(Margin::symmetric(16, 10)),
            )
            .show(ui, |ui| self.top_bar(ui));

        let profile = self.profile.clone();
        if let Some(p) = &profile {
            let low = self.computer_base.saturating_sub(12).max(21);
            let high = (low + 36).min(108);
            egui::Panel::bottom("keys")
                .exact_size(118.0)
                .frame(
                    egui::Frame::NONE
                        .fill(theme::PANEL)
                        .inner_margin(Margin::symmetric(16, 10)),
                )
                .show(ui, |ui| {
                    let base = self.computer_base;
                    let note = keys::show(ui, low, high, base, |n| self.sounding(n));
                    if note != self.mouse_note {
                        if let Some(old) = self.mouse_note {
                            self.send(Event::NoteOff { note: old });
                        }
                        if let Some(n) = note {
                            self.send(Event::NoteOn {
                                note: n,
                                velocity: 100,
                            });
                        }
                        self.mouse_note = note;
                    }
                });
            egui::Panel::bottom("controls")
                .frame(
                    egui::Frame::NONE
                        .fill(theme::PANEL)
                        .inner_margin(Margin::symmetric(16, 8)),
                )
                .show(ui, |ui| self.controls(ui));
            egui::Panel::right("info")
                .exact_size(290.0)
                .frame(
                    egui::Frame::NONE
                        .fill(theme::PANEL)
                        .inner_margin(Margin::symmetric(16, 14)),
                )
                .show(ui, |ui| self.info(ui, p));
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(theme::PANEL)
                    .inner_margin(Margin::same(14)),
            )
            .show(ui, |ui| {
                if let Some(job) = self.job.take() {
                    self.progress(ui, &job);
                    self.job = Some(job);
                } else if let Some(p) = &profile {
                    let live = self.live.as_ref().map(|l| l.status.vowel());
                    let out = pad::show(ui, p, live, self.selected);
                    if let Some(i) = out.picked {
                        self.selected = Some(i);
                    }
                    if let Some(v) = out.vowel {
                        self.send(Event::Vowel(v));
                    }
                    // Holding the pad sings at the speaker's own pitch.
                    if out.held && self.pad_note.is_none() {
                        let note = (69.0 + 12.0 * (p.pitch.median / 440.0).log2())
                            .round()
                            .clamp(24.0, 100.0) as u8;
                        self.send(Event::NoteOn { note, velocity: 96 });
                        self.pad_note = Some(note);
                    } else if !out.held {
                        if let Some(n) = self.pad_note.take() {
                            self.send(Event::NoteOff { note: n });
                        }
                    }
                } else {
                    self.empty_state(ui);
                }
            });

        if let Some((msg, at)) = &self.toast {
            if at.elapsed() < Duration::from_secs(5) {
                egui::Area::new(egui::Id::unique("toast"))
                    .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 58.0))
                    .show(&ctx, |ui| {
                        egui::Frame::NONE
                            .fill(theme::CARD)
                            .corner_radius(8.0)
                            .inner_margin(Margin::symmetric(14, 8))
                            .show(ui, |ui| {
                                ui.label(RichText::new(msg).color(theme::TEXT));
                            });
                    });
            } else {
                self.toast = None;
            }
        }

        if self.live.is_some() || self.job.is_some() || self.toast.is_some() {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
        self.screenshot(&ctx);
    }
}

impl App {
    /// With TABLEMAKER_SCREENSHOT set: once the window has settled (and any
    /// analysis is done), save it as a PNG and close.
    fn screenshot(&mut self, ctx: &egui::Context) {
        let Some((path, frames)) = &mut self.screenshot else {
            return;
        };
        if self.job.is_none() {
            *frames += 1;
        }
        if *frames == 20 {
            let (path, c) = (path.clone(), ctx.clone());
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(
                egui::ScreenshotCallback::new(move |img| {
                    let rgba: Vec<u8> = img.pixels.iter().flat_map(|p| p.to_array()).collect();
                    let _ = std::fs::write(
                        &path,
                        png::encode(img.size[0] as u32, img.size[1] as u32, &rgba),
                    );
                    c.send_viewport_cmd(egui::ViewportCommand::Close);
                    c.request_repaint();
                }),
            ));
        }
        ctx.request_repaint();
    }
}

/// The vowel table as a wavetable for other synths: one frame per vowel, in
/// morph order, at the speaker's own pitch.
fn export_wavetable(p: &SpeakerProfile, path: &std::path::Path) -> Result<(), String> {
    let bank = Bank::new(p, 48000.0);
    let (zone, _) = Bank::zone(p.pitch.median);
    let frames: Vec<Vec<f64>> = bank
        .vowels
        .iter()
        .map(|v| {
            let t = v.table(zone);
            let peak = t.iter().fold(0.0f32, |m, x| m.max(x.abs())).max(1e-9);
            t.iter().map(|&x| (x / peak * 0.95) as f64).collect()
        })
        .collect();
    wav::write_wavetable(path, &frames, 48000, wav::Encoding::Float32).map_err(|e| e.to_string())
}

//! Colours, fonts and the overall look.

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, Visuals};
use std::sync::Arc;

pub const BG: Color32 = Color32::from_rgb(0x12, 0x13, 0x17);
pub const PANEL: Color32 = Color32::from_rgb(0x1a, 0x1c, 0x22);
pub const CARD: Color32 = Color32::from_rgb(0x22, 0x25, 0x2d);
pub const LINE: Color32 = Color32::from_rgb(0x34, 0x38, 0x44);
pub const TEXT: Color32 = Color32::from_rgb(0xe8, 0xea, 0xf0);
pub const DIM: Color32 = Color32::from_rgb(0x8a, 0x90, 0x9e);
pub const ACCENT: Color32 = Color32::from_rgb(0xff, 0x9e, 0x5e);

/// A colour per vowel entry, evenly around the hue circle.
pub fn vowel_color(i: usize, n: usize) -> Color32 {
    let h = i as f32 / n.max(1) as f32;
    let hsva = egui::ecolor::Hsva::new(0.58 + h * 0.85, 0.55, 0.95, 1.0);
    Color32::from(hsva)
}

pub fn apply(ctx: &egui::Context) {
    let mut v = Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = BG;
    v.faint_bg_color = CARD;
    v.selection.bg_fill = ACCENT.gamma_multiply(0.6);
    v.widgets.inactive.bg_fill = CARD;
    v.widgets.inactive.weak_bg_fill = CARD;
    v.widgets.hovered.weak_bg_fill = LINE;
    v.widgets.active.weak_bg_fill = ACCENT.gamma_multiply(0.5);
    v.slider_trailing_fill = true;
    ctx.set_visuals(v);
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(10.0, 5.0);
    });

    // IPA symbols (ɑ ɛ ɪ ʊ ɔ ʌ ɝ æ) need a font that has them: DejaVu Sans, given by the
    // package wrapper or dev shell, as a fallback after egui's own fonts.
    if let Some(bytes) = std::env::var_os("TABLEMAKER_UI_FONT").and_then(|p| std::fs::read(p).ok())
    {
        let mut fonts = FontDefinitions::default();
        fonts
            .font_data
            .insert("dejavu".into(), Arc::new(FontData::from_owned(bytes)));
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .push("dejavu".into());
        }
        ctx.set_fonts(fonts);
    }
}

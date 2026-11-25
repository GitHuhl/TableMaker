//! The on-screen keyboard, played with the mouse or the computer keyboard
//! (A W S E D F T G Y H U J K O L: one octave and a bit; Z and X shift
//! octaves).

use super::theme;
use eframe::egui::{self, Align2, Color32, FontId, Key, Pos2, Rect, Sense, Stroke, Vec2};

/// Computer keys and their semitone above the keyboard's base C.
pub const COMPUTER_KEYS: [(Key, u8); 15] = [
    (Key::A, 0),
    (Key::W, 1),
    (Key::S, 2),
    (Key::E, 3),
    (Key::D, 4),
    (Key::F, 5),
    (Key::T, 6),
    (Key::G, 7),
    (Key::Y, 8),
    (Key::H, 9),
    (Key::U, 10),
    (Key::J, 11),
    (Key::K, 12),
    (Key::O, 13),
    (Key::L, 14),
];

fn is_black(note: u8) -> bool {
    matches!(note % 12, 1 | 3 | 6 | 8 | 10)
}

fn key_letter(note: u8, base: u8) -> Option<&'static str> {
    let off = note.checked_sub(base)?;
    COMPUTER_KEYS
        .iter()
        .find(|(_, o)| *o == off)
        .map(|(k, _)| k.name())
}

/// Draw `low..=high` (white keys at both ends). Returns the note under the
/// pointer while it is held on the keyboard.
pub fn show(
    ui: &mut egui::Ui,
    low: u8,
    high: u8,
    computer_base: u8,
    sounding: impl Fn(u8) -> bool,
) -> Option<u8> {
    let rect = ui.available_rect_before_wrap();
    let resp = ui.allocate_rect(rect, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let whites: Vec<u8> = (low..=high).filter(|&n| !is_black(n)).collect();
    let w = rect.width() / whites.len() as f32;
    let white_rect = |i: usize| {
        Rect::from_min_size(
            Pos2::new(rect.left() + i as f32 * w, rect.top()),
            Vec2::new(w, rect.height()),
        )
    };
    let black_rect = |note: u8| -> Rect {
        let i = whites.iter().position(|&n| n == note - 1).unwrap_or(0);
        let x = rect.left() + (i as f32 + 1.0) * w;
        Rect::from_center_size(
            Pos2::new(x, rect.top() + rect.height() * 0.31),
            Vec2::new(w * 0.62, rect.height() * 0.62),
        )
    };
    let label = FontId::proportional(11.0);
    for (i, &note) in whites.iter().enumerate() {
        let r = white_rect(i).shrink2(Vec2::new(1.0, 0.0));
        let fill = if sounding(note) {
            theme::ACCENT
        } else {
            Color32::from_rgb(0xec, 0xee, 0xf2)
        };
        painter.rect_filled(r, 4.0, fill);
        if note % 12 == 0 {
            painter.text(
                r.center_bottom() - Vec2::new(0.0, 6.0),
                Align2::CENTER_BOTTOM,
                format!("C{}", note as i32 / 12 - 1),
                label.clone(),
                Color32::from_gray(120),
            );
        }
        if let Some(k) = key_letter(note, computer_base) {
            painter.text(
                r.center_bottom() - Vec2::new(0.0, 22.0),
                Align2::CENTER_BOTTOM,
                k,
                label.clone(),
                Color32::from_gray(160),
            );
        }
    }
    let blacks: Vec<u8> = (low..=high).filter(|&n| is_black(n)).collect();
    for &note in &blacks {
        let r = black_rect(note);
        let fill = if sounding(note) {
            theme::ACCENT
        } else {
            Color32::from_rgb(0x1c, 0x1e, 0x24)
        };
        painter.rect_filled(r, 3.0, fill);
        painter.rect_stroke(
            r,
            3.0,
            Stroke::new(1.0, theme::LINE),
            egui::StrokeKind::Inside,
        );
        if let Some(k) = key_letter(note, computer_base) {
            painter.text(
                r.center_bottom() - Vec2::new(0.0, 6.0),
                Align2::CENTER_BOTTOM,
                k,
                label.clone(),
                Color32::from_gray(150),
            );
        }
    }
    if !resp.is_pointer_button_down_on() {
        return None;
    }
    let q = resp.interact_pointer_pos()?;
    if let Some(&n) = blacks.iter().find(|&&n| black_rect(n).contains(q)) {
        return Some(n);
    }
    let i = ((q.x - rect.left()) / w)
        .floor()
        .clamp(0.0, (whites.len() - 1) as f32) as usize;
    Some(whites[i])
}

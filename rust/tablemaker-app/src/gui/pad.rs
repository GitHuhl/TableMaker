//! The vowel pad: the speaker's vowels laid out as on a vowel chart (front
//! to back across, close to open down), joined in morph order. Press to
//! sing, move to morph along the path.

use super::theme;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use tablemaker_dsp::speaker::SpeakerProfile;

pub struct PadOutput {
    /// Pointer position along the vowel path (0..1) while pressed.
    pub vowel: Option<f32>,
    /// Pointer is held down on the pad.
    pub held: bool,
    /// A vowel tile was pressed.
    pub picked: Option<usize>,
}

struct Map {
    rect: Rect,
    x: (f64, f64),
    y: (f64, f64),
}

impl Map {
    fn new(rect: Rect, p: &SpeakerProfile) -> Self {
        let f = |k: usize| {
            p.vowels
                .iter()
                .filter_map(move |v| v.formants.get(k).map(|f| f.freq.ln()))
        };
        let span = |it: &mut dyn Iterator<Item = f64>, lo: f64, hi: f64| {
            let (mut a, mut b) = (lo.ln(), hi.ln());
            for v in it {
                a = a.min(v);
                b = b.max(v);
            }
            let pad = (b - a) * 0.18;
            (a - pad, b + pad)
        };
        Map {
            rect,
            y: span(&mut f(0), 300.0, 750.0),
            x: span(&mut f(1), 900.0, 2300.0),
        }
    }

    fn pos(&self, f1: f64, f2: f64) -> Pos2 {
        let r = self.rect.shrink(36.0);
        // High F2 (front) on the left, low F1 (close) at the top.
        let tx = 1.0 - (f2.ln() - self.x.0) / (self.x.1 - self.x.0);
        let ty = (f1.ln() - self.y.0) / (self.y.1 - self.y.0);
        Pos2::new(
            r.left() + r.width() * tx as f32,
            r.top() + r.height() * ty as f32,
        )
    }
}

fn entry_pos(map: &Map, p: &SpeakerProfile, i: usize, fallback: Pos2) -> Pos2 {
    let v = &p.vowels[i];
    match (v.formants.first(), v.formants.get(1)) {
        (Some(a), Some(b)) => map.pos(a.freq, b.freq),
        _ => fallback,
    }
}

/// The point at `t` (0..1) along the path through `pts`.
fn along(pts: &[Pos2], t: f32) -> Pos2 {
    if pts.len() == 1 {
        return pts[0];
    }
    let x = t.clamp(0.0, 1.0) * (pts.len() - 1) as f32;
    let i = (x.floor() as usize).min(pts.len() - 2);
    pts[i] + (pts[i + 1] - pts[i]) * (x - i as f32)
}

/// Where along the path (0..1) a point projects.
fn project(pts: &[Pos2], q: Pos2) -> f32 {
    if pts.len() < 2 {
        return 0.0;
    }
    let mut best = (f32::INFINITY, 0.0);
    for i in 0..pts.len() - 1 {
        let d = pts[i + 1] - pts[i];
        let u = if d.length_sq() > 0.0 {
            ((q - pts[i]).dot(d) / d.length_sq()).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let dist = (pts[i] + d * u - q).length();
        if dist < best.0 {
            best = (dist, (i as f32 + u) / (pts.len() - 1) as f32);
        }
    }
    best.1
}

pub fn show(
    ui: &mut egui::Ui,
    p: &SpeakerProfile,
    live_vowel: Option<f32>,
    selected: Option<usize>,
) -> PadOutput {
    let rect = ui.available_rect_before_wrap();
    let resp = ui.allocate_rect(rect, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 14.0, theme::BG);
    let map = Map::new(rect, p);
    let n = p.vowels.len();

    // Orientation, in words a singer uses.
    let small = FontId::proportional(12.0);
    let inner = rect.shrink(12.0);
    painter.text(
        inner.left_top(),
        Align2::LEFT_TOP,
        "front",
        small.clone(),
        theme::DIM,
    );
    painter.text(
        inner.right_top(),
        Align2::RIGHT_TOP,
        "back",
        small.clone(),
        theme::DIM,
    );
    painter.text(
        inner.center_top(),
        Align2::CENTER_TOP,
        "close",
        small.clone(),
        theme::DIM,
    );
    painter.text(
        inner.center_bottom(),
        Align2::CENTER_BOTTOM,
        "open",
        small.clone(),
        theme::DIM,
    );

    // Every analysed cycle, faintly.
    for pt in &p.points {
        let c = theme::vowel_color(pt.entry, n).gamma_multiply(0.18);
        painter.circle_filled(map.pos(pt.f1, pt.f2), 1.6, c);
    }

    let centre = rect.center();
    let pts: Vec<Pos2> = (0..n).map(|i| entry_pos(&map, p, i, centre)).collect();
    if pts.len() > 1 {
        painter.add(egui::Shape::line(
            pts.clone(),
            Stroke::new(2.0, theme::LINE),
        ));
    }

    // The voice's current vowel.
    if let Some(v) = live_vowel {
        let at = along(&pts, v);
        painter.circle_filled(at, 14.0, theme::ACCENT.gamma_multiply(0.18));
        painter.circle_stroke(at, 9.0, Stroke::new(2.5, theme::ACCENT));
    }

    let radius = 24.0;
    let hover = resp.hover_pos();
    for (i, &at) in pts.iter().enumerate() {
        let c = theme::vowel_color(i, n);
        let hot = hover.is_some_and(|h| (h - at).length() < radius);
        painter.circle_filled(at, radius, if hot { c } else { c.gamma_multiply(0.85) });
        if selected == Some(i) {
            painter.circle_stroke(at, radius + 4.0, Stroke::new(2.0, theme::TEXT));
        }
        painter.text(
            at,
            Align2::CENTER_CENTER,
            &p.vowels[i].label,
            FontId::proportional(24.0),
            Color32::from_rgb(20, 20, 26),
        );
        painter.text(
            at + Vec2::new(0.0, radius + 10.0),
            Align2::CENTER_CENTER,
            format!("{:.0}%", p.vowels[i].share * 100.0),
            small.clone(),
            theme::DIM,
        );
    }

    let held = resp.is_pointer_button_down_on();
    let pointer = resp.interact_pointer_pos();
    let picked = if resp.drag_started() || resp.clicked() {
        pointer.and_then(|q| (0..n).find(|&i| (pts[i] - q).length() < radius))
    } else {
        None
    };
    PadOutput {
        vowel: if held {
            pointer.map(|q| project(&pts, q))
        } else {
            None
        },
        held,
        picked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_onto_the_path() {
        let pts = [
            Pos2::new(0.0, 0.0),
            Pos2::new(10.0, 0.0),
            Pos2::new(10.0, 10.0),
        ];
        assert_eq!(project(&pts, Pos2::new(-5.0, 1.0)), 0.0);
        assert!((project(&pts, Pos2::new(5.0, 3.0)) - 0.25).abs() < 1e-6);
        assert!((project(&pts, Pos2::new(12.0, 10.0)) - 1.0).abs() < 1e-6);
        assert_eq!(along(&pts, 0.75), Pos2::new(10.0, 5.0));
    }
}

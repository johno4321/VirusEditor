//! Filter-response and envelope visualisations, fed from the raw patch bytes
//! (A40–A43, A51–A53, B32; envelopes A54–A58 and A59–A63).

use egui::{pos2, vec2, Color32, Response, Sense, Stroke, Ui, Vec2, Widget};
use virus_syx::{Page, Patch};

/// Inputs of the dual-filter response, as raw parameter values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilterParams {
    pub cutoff1: u8,
    /// Effective Cutoff2: A41 as an offset from Cutoff when B32 Cutoff Link is on, else absolute.
    pub cutoff2: u8,
    pub resonance1: u8,
    pub resonance2: u8,
    pub mode1: u8,
    pub mode2: u8,
    pub routing: u8,
}

impl FilterParams {
    pub fn from_patch(patch: &Patch) -> Self {
        let cutoff1 = patch.get(Page::PageA, 40);
        let cutoff2_raw = patch.get(Page::PageA, 41);
        let linked = patch.get(Page::PageB, 32) != 0;
        let cutoff2 = if linked {
            (i16::from(cutoff1) + i16::from(cutoff2_raw) - 64).clamp(0, 127) as u8
        } else {
            cutoff2_raw
        };
        Self {
            cutoff1,
            cutoff2,
            resonance1: patch.get(Page::PageA, 42),
            resonance2: patch.get(Page::PageA, 43),
            mode1: patch.get(Page::PageA, 51),
            mode2: patch.get(Page::PageA, 52),
            routing: patch.get(Page::PageA, 53),
        }
    }
}

/// Inputs of an envelope view, as raw parameter values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvelopeParams {
    pub attack: u8,
    pub decay: u8,
    pub sustain: u8,
    /// Bipolar: below 64 the sustain level falls, above 64 it rises.
    pub sustain_time: u8,
    pub release: u8,
}

impl EnvelopeParams {
    /// Filter envelope: A54..A58.
    pub fn filter(patch: &Patch) -> Self {
        Self::read(patch, 54)
    }

    /// Amp envelope: A59..A63.
    pub fn amp(patch: &Patch) -> Self {
        Self::read(patch, 59)
    }

    fn read(patch: &Patch, first: u8) -> Self {
        let p = |i: u8| patch.get(Page::PageA, first + i);
        Self { attack: p(0), decay: p(1), sustain: p(2), sustain_time: p(3), release: p(4) }
    }
}

pub struct FilterCurve {
    params: FilterParams,
    size: Vec2,
}

impl FilterCurve {
    pub fn new(params: FilterParams) -> Self {
        Self { params, size: vec2(280.0, 110.0) }
    }
}

const ROUTING_NAMES: [&str; 4] = ["Serial 4", "Serial 6", "Parallel 4", "Split"];

impl Widget for FilterCurve {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, response) = ui.allocate_exact_size(self.size, Sense::hover());
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let painter = ui.painter();
        painter.rect_filled(rect, egui::Rounding::same(4.0), Color32::from_rgb(18, 20, 24));
        painter.rect_stroke(rect, egui::Rounding::same(4.0), Stroke::new(1.0_f32, Color32::from_rgb(45, 50, 60)));

        let log_span = 20_000.0_f32.log10() - 20.0_f32.log10();
        for f in [100.0_f32, 1_000.0, 10_000.0] {
            let x = rect.min.x + (f.log10() - 20.0_f32.log10()) / log_span * rect.width();
            painter.line_segment(
                [pos2(x, rect.min.y), pos2(x, rect.max.y)],
                Stroke::new(1.0_f32, Color32::from_rgb(32, 36, 44)),
            );
        }

        let p = self.params;
        let fc = |cutoff: u8| 30.0 * (18_000.0_f32 / 30.0).powf(f32::from(cutoff) / 127.0);
        let q = |reso: u8| 0.5 + f32::from(reso) / 127.0 * 8.0;
        let (fc1, q1, fc2, q2) = (fc(p.cutoff1), q(p.resonance1), fc(p.cutoff2), q(p.resonance2));
        const STEPS: usize = 64;
        let points = (0..=STEPS)
            .map(|i| {
                let t = i as f32 / STEPS as f32;
                let f = 20.0 * (20_000.0_f32 / 20.0).powf(t);
                let m1 = magnitude(f, fc1, q1, p.mode1);
                let m2 = magnitude(f, fc2, q2, p.mode2);
                let combined = match p.routing {
                    0 | 1 => m1 * m2,
                    _ => (m1 + m2) * 0.5,
                };
                let db = 20.0 * combined.max(0.0001).log10();
                let y = ((12.0 - db) / 48.0).clamp(0.05, 0.95);
                pos2(rect.min.x + t * rect.width(), rect.min.y + y * rect.height())
            })
            .collect();
        painter.add(egui::Shape::line(points, Stroke::new(2.0_f32, Color32::from_rgb(230, 80, 50))));

        let routing = ROUTING_NAMES.get(p.routing as usize).map_or_else(|| format!("#{}", p.routing), |s| (*s).to_owned());
        painter.text(
            rect.min + vec2(8.0, 6.0),
            egui::Align2::LEFT_TOP,
            format!("Filters: {routing}"),
            egui::FontId::proportional(10.0),
            Color32::from_rgb(140, 145, 160),
        );
        response
    }
}

/// Approximate magnitude response; modes 0 LP, 1 HP, 2 BP, 3 BS (others drawn as LP).
fn magnitude(f: f32, fc: f32, q: f32, mode: u8) -> f32 {
    let r = f / fc;
    let denominator = ((1.0 - r * r).powi(2) + (r / q).powi(2)).sqrt();
    match mode {
        1 => r * r / denominator,
        2 => (r / q) / denominator,
        3 => (1.0 - r * r).abs() / denominator,
        _ => 1.0 / denominator,
    }
}

pub struct EnvelopeCurve<'a> {
    label: &'a str,
    params: EnvelopeParams,
    size: Vec2,
}

impl<'a> EnvelopeCurve<'a> {
    pub fn new(label: &'a str, params: EnvelopeParams) -> Self {
        Self { label, params, size: vec2(220.0, 110.0) }
    }
}

impl Widget for EnvelopeCurve<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, response) = ui.allocate_exact_size(self.size, Sense::hover());
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let painter = ui.painter();
        painter.rect_filled(rect, egui::Rounding::same(4.0), Color32::from_rgb(20, 22, 26));
        painter.rect_stroke(rect, egui::Rounding::same(4.0), Stroke::new(1.0_f32, Color32::from_rgb(45, 50, 60)));

        let pad = 8.0;
        let (width, height) = (rect.width() - 2.0 * pad, rect.height() - 2.0 * pad - 10.0);
        let bottom = rect.max.y - pad;
        let top = bottom - height;
        let norm = |v: u8| f32::from(v) / 127.0;
        let e = self.params;
        let a = 0.05 + 0.30 * norm(e.attack);
        let d = 0.05 + 0.30 * norm(e.decay);
        let r = 0.05 + 0.30 * norm(e.release);
        let s = (1.0 - (a + d + r)).max(0.15);
        let level = |x: f32| bottom - x.clamp(0.0, 1.0) * height;
        // Sustain time: 64 holds the level; lower values fall towards 0, higher rise towards 1.
        let slope = (f32::from(e.sustain_time) - 64.0) / 64.0;
        let sustain_start = norm(e.sustain);
        let sustain_end = if slope < 0.0 {
            sustain_start * (1.0 + slope)
        } else {
            sustain_start + (1.0 - sustain_start) * slope
        };

        let p0 = pos2(rect.min.x + pad, bottom);
        let p1 = pos2(p0.x + a * width, top);
        let p2 = pos2(p1.x + d * width, level(sustain_start));
        let p3 = pos2(p2.x + s * width, level(sustain_end));
        let p4 = pos2(p3.x + r * width, bottom);
        let line = Color32::from_rgb(240, 150, 60);
        for segment in [[p0, p1], [p1, p2], [p2, p3], [p3, p4]] {
            painter.line_segment(segment, Stroke::new(2.0_f32, line));
        }
        for point in [p1, p2, p3] {
            painter.circle_filled(point, 2.5, Color32::from_rgb(255, 230, 200));
        }
        painter.text(
            rect.min + vec2(8.0, 4.0),
            egui::Align2::LEFT_TOP,
            self.label,
            egui::FontId::proportional(10.0),
            Color32::from_rgb(150, 155, 170),
        );
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_params_read_the_documented_bytes() {
        let mut patch = Patch::init();
        for (n, v) in [(40, 100), (41, 30), (42, 11), (43, 12), (51, 1), (52, 2), (53, 3)] {
            patch.set(Page::PageA, n, v);
        }
        let params = FilterParams::from_patch(&patch);
        assert_eq!(
            params,
            FilterParams { cutoff1: 100, cutoff2: 30, resonance1: 11, resonance2: 12, mode1: 1, mode2: 2, routing: 3 }
        );
        patch.set(Page::PageB, 32, 1);
        assert_eq!(FilterParams::from_patch(&patch).cutoff2, 66);
    }

    #[test]
    fn envelope_params_read_their_five_bytes() {
        let mut patch = Patch::init();
        for n in 54..=63 {
            patch.set(Page::PageA, n, n);
        }
        assert_eq!(
            EnvelopeParams::filter(&patch),
            EnvelopeParams { attack: 54, decay: 55, sustain: 56, sustain_time: 57, release: 58 }
        );
        assert_eq!(EnvelopeParams::amp(&patch).release, 63);
    }
}

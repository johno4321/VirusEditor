//! Graphical curve visualizers for Access Virus Filter frequency responses and ADSR envelopes.

use egui::{
    pos2, vec2, Color32, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2, Widget,
};
use virus_syx::domain::types::{FilterMode, FilterRouting, Unipolar7};

/// Interactive / visual dual filter frequency response curve.
pub struct FilterCurveWidget {
    pub cutoff1: u8,
    pub resonance1: u8,
    pub mode1: FilterMode,
    pub cutoff2: u8,
    pub resonance2: u8,
    pub mode2: FilterMode,
    pub routing: FilterRouting,
    pub size: Vec2,
}

impl FilterCurveWidget {
    pub fn new(
        cutoff1: Unipolar7,
        resonance1: Unipolar7,
        mode1: FilterMode,
        cutoff2: Unipolar7,
        resonance2: Unipolar7,
        mode2: FilterMode,
        routing: FilterRouting,
    ) -> Self {
        Self {
            cutoff1: cutoff1.0,
            resonance1: resonance1.0,
            mode1,
            cutoff2: cutoff2.0,
            resonance2: resonance2.0,
            mode2,
            routing,
            size: vec2(280.0, 110.0),
        }
    }

    pub fn size(mut self, size: Vec2) -> Self {
        self.size = size;
        self
    }
}

impl Widget for FilterCurveWidget {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, response) = ui.allocate_exact_size(self.size, Sense::hover());

        if ui.is_rect_visible(rect) {
            let painter = ui.painter();

            // 1. Background grid
            painter.rect_filled(rect, egui::Rounding::same(4.0), Color32::from_rgb(18, 20, 24));
            painter.rect_stroke(rect, egui::Rounding::same(4.0), Stroke::new(1.0, Color32::from_rgb(45, 50, 60)));

            // Frequency grid markers (100 Hz, 1 kHz, 10 kHz)
            let f_markers = [100.0f32, 1000.0, 10000.0];
            for &f in &f_markers {
                let norm_x = (f.log10() - 20.0f32.log10()) / (20000.0f32.log10() - 20.0f32.log10());
                let x = rect.min.x + (norm_x * rect.width());
                painter.line_segment(
                    [pos2(x, rect.min.y), pos2(x, rect.max.y)],
                    Stroke::new(1.0, Color32::from_rgb(32, 36, 44)),
                );
            }

            // 0 dB center line
            let zero_db_y = rect.min.y + (rect.height() * 0.45);
            painter.line_segment(
                [pos2(rect.min.x, zero_db_y), pos2(rect.max.x, zero_db_y)],
                Stroke::new(1.0, Color32::from_rgb(38, 42, 52)),
            );

            // 2. Compute Filter response curve
            let steps = 64;
            let mut points = Vec::with_capacity(steps + 1);

            // Map 0..127 to ~30Hz..18kHz
            let fc1 = 30.0 * (18000.0 / 30.0f32).powf(self.cutoff1 as f32 / 127.0);
            let q1 = 0.5 + (self.resonance1 as f32 / 127.0) * 8.0;

            let fc2 = 30.0 * (18000.0 / 30.0f32).powf(self.cutoff2 as f32 / 127.0);
            let q2 = 0.5 + (self.resonance2 as f32 / 127.0) * 8.0;

            for i in 0..=steps {
                let t = i as f32 / steps as f32;
                // Log frequency from 20 Hz to 20000 Hz
                let f = 20.0 * (20000.0 / 20.0f32).powf(t);

                let mag1 = eval_filter_mag(f, fc1, q1, self.mode1);
                let mag2 = eval_filter_mag(f, fc2, q2, self.mode2);

                let combined_mag = match self.routing {
                    FilterRouting::Serial4 | FilterRouting::Serial6 => mag1 * mag2,
                    FilterRouting::Parallel => (mag1 + mag2) * 0.5,
                    FilterRouting::Split => (mag1 + mag2) * 0.5,
                };

                // Convert to dB
                let db = 20.0 * combined_mag.max(0.0001).log10();
                // Map +12 dB to rect.min.y, -36 dB to rect.max.y
                let norm_y = (12.0 - db) / 48.0;
                let clamped_norm_y = norm_y.clamp(0.05, 0.95);

                let x = rect.min.x + (t * rect.width());
                let y = rect.min.y + (clamped_norm_y * rect.height());
                points.push(pos2(x, y));
            }

            // Draw response line
            let curve_color = Color32::from_rgb(230, 80, 50); // Signature Virus red/amber
            painter.add(egui::Shape::line(points, Stroke::new(2.0, curve_color)));

            // Routing and Cutoff readout overlay
            let routing_str = match self.routing {
                FilterRouting::Serial4 => "Serial 4",
                FilterRouting::Serial6 => "Serial 6",
                FilterRouting::Parallel => "Parallel",
                FilterRouting::Split => "Split",
            };
            painter.text(
                pos2(rect.min.x + 8.0, rect.min.y + 6.0),
                egui::Align2::LEFT_TOP,
                format!("Filter: {}", routing_str),
                egui::FontId::proportional(10.0),
                Color32::from_rgb(140, 145, 160),
            );
        }

        response
    }
}

/// Computes filter magnitude response at frequency `f`.
fn eval_filter_mag(f: f32, fc: f32, q: f32, mode: FilterMode) -> f32 {
    let ratio = f / fc;
    match mode {
        FilterMode::Lowpass => {
            // 2nd-order / 4th-order lowpass approximation
            1.0 / ((1.0 - ratio * ratio).powi(2) + (ratio / q).powi(2)).sqrt()
        }
        FilterMode::Highpass => {
            (ratio * ratio) / ((1.0 - ratio * ratio).powi(2) + (ratio / q).powi(2)).sqrt()
        }
        FilterMode::Bandpass => {
            (ratio / q) / ((1.0 - ratio * ratio).powi(2) + (ratio / q).powi(2)).sqrt()
        }
        FilterMode::Notch => {
            ((1.0 - ratio * ratio).abs())
                / ((1.0 - ratio * ratio).powi(2) + (ratio / q).powi(2)).sqrt()
        }
    }
}

/// Interactive / visual ADSR Envelope Curve widget.
pub struct EnvelopeCurveWidget {
    pub attack: u8,
    pub decay: u8,
    pub sustain: u8,
    pub release: u8,
    pub label: String,
    pub size: Vec2,
}

impl EnvelopeCurveWidget {
    pub fn new(
        label: impl Into<String>,
        attack: Unipolar7,
        decay: Unipolar7,
        sustain: Unipolar7,
        release: Unipolar7,
    ) -> Self {
        Self {
            label: label.into(),
            attack: attack.0,
            decay: decay.0,
            sustain: sustain.0,
            release: release.0,
            size: vec2(200.0, 75.0),
        }
    }

    pub fn size(mut self, size: Vec2) -> Self {
        self.size = size;
        self
    }
}

impl Widget for EnvelopeCurveWidget {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, response) = ui.allocate_exact_size(self.size, Sense::hover());

        if ui.is_rect_visible(rect) {
            let painter = ui.painter();

            // Background
            painter.rect_filled(rect, egui::Rounding::same(4.0), Color32::from_rgb(20, 22, 26));
            painter.rect_stroke(rect, egui::Rounding::same(4.0), Stroke::new(1.0, Color32::from_rgb(45, 50, 60)));

            let pad_x = 8.0;
            let pad_y = 8.0;
            let inner_w = rect.width() - (pad_x * 2.0);
            let inner_h = rect.height() - (pad_y * 2.0);

            let bottom_y = rect.max.y - pad_y;
            let top_y = rect.min.y + pad_y;

            // Attack, Decay, Release fractional widths (total ~ 1.0)
            let a_frac = 0.05 + 0.30 * (self.attack as f32 / 127.0);
            let d_frac = 0.05 + 0.30 * (self.decay as f32 / 127.0);
            let r_frac = 0.05 + 0.30 * (self.release as f32 / 127.0);
            let s_frac = (1.0 - (a_frac + d_frac + r_frac)).max(0.15);

            let sustain_y = bottom_y - (self.sustain as f32 / 127.0) * inner_h;

            let p0 = pos2(rect.min.x + pad_x, bottom_y);
            let p1 = pos2(p0.x + a_frac * inner_w, top_y);
            let p2 = pos2(p1.x + d_frac * inner_w, sustain_y);
            let p3 = pos2(p2.x + s_frac * inner_w, sustain_y);
            let p4 = pos2(p3.x + r_frac * inner_w, bottom_y);

            // Shaded envelope polygon
            let fill_points = vec![p0, p1, p2, p3, p4, pos2(p4.x, bottom_y), p0];
            painter.add(egui::Shape::convex_polygon(
                fill_points,
                Color32::from_rgba_unmultiplied(220, 70, 40, 40),
                Stroke::NONE,
            ));

            // Envelope line segments
            let line_color = Color32::from_rgb(240, 150, 60);
            painter.line_segment([p0, p1], Stroke::new(2.0, line_color));
            painter.line_segment([p1, p2], Stroke::new(2.0, line_color));
            painter.line_segment([p2, p3], Stroke::new(2.0, line_color));
            painter.line_segment([p3, p4], Stroke::new(2.0, line_color));

            // Node dots
            let dot_color = Color32::from_rgb(255, 230, 200);
            for pt in [p1, p2, p3] {
                painter.circle_filled(pt, 2.5, dot_color);
            }

            // Top-left label
            painter.text(
                pos2(rect.min.x + 8.0, rect.min.y + 6.0),
                egui::Align2::LEFT_TOP,
                &self.label,
                egui::FontId::proportional(10.0),
                Color32::from_rgb(150, 155, 170),
            );
        }

        response
    }
}

//! Custom rotary knob widget with support for Unipolar7 (0..127) and Bipolar7 (-64..+63)
//! values with center-detent visual feedback.

use egui::{
    pos2, vec2, Color32, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2, Widget,
};
use std::f32::consts::PI;
use virus_syx::domain::types::{Bipolar7, Unipolar7};

const ARC_START_ANGLE: f32 = 0.75 * PI; // -135 degrees (bottom left)
const ARC_END_ANGLE: f32 = 2.25 * PI;   // +135 degrees (bottom right)
const ARC_TOTAL_ANGLE: f32 = 1.5 * PI;  // 270 degrees sweep

/// Visual theme colors for rotary knobs
#[derive(Clone, Copy)]
pub struct KnobColors {
    pub body: Color32,
    pub border: Color32,
    pub track: Color32,
    pub active_arc: Color32,
    pub needle: Color32,
    pub center_dot: Color32,
}

impl Default for KnobColors {
    fn default() -> Self {
        Self {
            body: Color32::from_rgb(32, 34, 40),
            border: Color32::from_rgb(60, 64, 75),
            track: Color32::from_rgb(45, 48, 58),
            active_arc: Color32::from_rgb(220, 60, 40), // Classic Virus red accent
            needle: Color32::from_rgb(255, 230, 200),
            center_dot: Color32::from_rgb(80, 85, 100),
        }
    }
}

/// Rotary knob for Unipolar 7-bit parameters (0..=127).
pub struct UnipolarKnob<'a> {
    value: &'a mut Unipolar7,
    label: &'a str,
    radius: f32,
    colors: KnobColors,
    default_val: u8,
}

impl<'a> UnipolarKnob<'a> {
    pub fn new(value: &'a mut Unipolar7, label: &'a str) -> Self {
        Self {
            value,
            label,
            radius: 18.0,
            colors: KnobColors::default(),
            default_val: 0,
        }
    }

    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    pub fn default_val(mut self, val: u8) -> Self {
        self.default_val = val;
        self
    }

    pub fn colors(mut self, colors: KnobColors) -> Self {
        self.colors = colors;
        self
    }
}

impl<'a> Widget for UnipolarKnob<'a> {
    fn ui(self, ui: &mut Ui) -> Response {
        let diameter = self.radius * 2.0;
        let font_height = ui.spacing().interact_size.y * 0.7;
        let total_size = vec2(diameter + 14.0, diameter + (font_height * 2.0) + 6.0);
        let (rect, mut response) = ui.allocate_exact_size(total_size, Sense::click_and_drag());

        let norm_val = self.value.0 as f32 / 127.0;

        // Interaction handling
        if response.dragged() {
            let delta = response.drag_delta();
            let sensitivity = if ui.input(|i| i.modifiers.shift) { 0.002 } else { 0.008 };
            let new_norm = (norm_val - delta.y * sensitivity).clamp(0.0, 1.0);
            let new_u8 = (new_norm * 127.0).round() as u8;
            if new_u8 != self.value.0 {
                self.value.0 = new_u8;
                response.mark_changed();
            }
        }

        // Scroll wheel adjustment
        if response.hovered() {
            let scroll_delta = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll_delta.abs() > 0.0 {
                let step = if scroll_delta > 0.0 { 1i16 } else { -1i16 };
                let new_val = (self.value.0 as i16 + step).clamp(0, 127) as u8;
                if new_val != self.value.0 {
                    self.value.0 = new_val;
                    response.mark_changed();
                }
            }
        }

        // Double click to reset to default
        if response.double_clicked() {
            if self.value.0 != self.default_val {
                self.value.0 = self.default_val;
                response.mark_changed();
            }
        }

        if ui.is_rect_visible(rect) {
            let painter = ui.painter();
            let center = pos2(rect.center().x, rect.min.y + self.radius + font_height + 2.0);

            // 1. Draw top label
            let label_pos = pos2(rect.center().x, rect.min.y + font_height * 0.5);
            painter.text(
                label_pos,
                egui::Align2::CENTER_CENTER,
                self.label,
                egui::FontId::proportional(11.0),
                ui.visuals().text_color(),
            );

            // 2. Outer track background arc
            let track_radius = self.radius + 3.0;
            draw_arc(
                painter,
                center,
                track_radius,
                ARC_START_ANGLE,
                ARC_END_ANGLE,
                self.colors.track,
                2.5,
            );

            // 3. Active arc from min (0) to current value
            let current_angle = ARC_START_ANGLE + (norm_val * ARC_TOTAL_ANGLE);
            if norm_val > 0.001 {
                draw_arc(
                    painter,
                    center,
                    track_radius,
                    ARC_START_ANGLE,
                    current_angle,
                    self.colors.active_arc,
                    2.5,
                );
            }

            // 4. Knob body
            painter.circle(
                center,
                self.radius,
                self.colors.body,
                Stroke::new(1.0, self.colors.border),
            );

            // 5. Needle indicator
            let needle_inner = center + vec2(current_angle.cos(), current_angle.sin()) * (self.radius * 0.35);
            let needle_outer = center + vec2(current_angle.cos(), current_angle.sin()) * (self.radius * 0.85);
            painter.line_segment(
                [needle_inner, needle_outer],
                Stroke::new(2.0, self.colors.needle),
            );

            // 6. Value label under the knob
            let val_pos = pos2(rect.center().x, rect.max.y - font_height * 0.5);
            painter.text(
                val_pos,
                egui::Align2::CENTER_CENTER,
                format!("{}", self.value.0),
                egui::FontId::monospace(10.0),
                Color32::from_rgb(170, 175, 190),
            );
        }

        response
    }
}

/// Rotary knob for Bipolar 7-bit parameters (-64..=+63) with center detent at 0.
pub struct BipolarKnob<'a> {
    value: &'a mut Bipolar7,
    label: &'a str,
    radius: f32,
    colors: KnobColors,
}

impl<'a> BipolarKnob<'a> {
    pub fn new(value: &'a mut Bipolar7, label: &'a str) -> Self {
        Self {
            value,
            label,
            radius: 18.0,
            colors: KnobColors::default(),
        }
    }

    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    pub fn colors(mut self, colors: KnobColors) -> Self {
        self.colors = colors;
        self
    }
}

impl<'a> Widget for BipolarKnob<'a> {
    fn ui(self, ui: &mut Ui) -> Response {
        let diameter = self.radius * 2.0;
        let font_height = ui.spacing().interact_size.y * 0.7;
        let total_size = vec2(diameter + 14.0, diameter + (font_height * 2.0) + 6.0);
        let (rect, mut response) = ui.allocate_exact_size(total_size, Sense::click_and_drag());

        // Normalized from 0.0 (-64) to 1.0 (+63), center (0) is at 64/127 ~= 0.5039
        let current_i8 = self.value.as_i8();
        let norm_val = (current_i8 as f32 + 64.0) / 127.0;

        // Center angle (12 o'clock) is 1.5 * PI (270 degrees)
        let center_angle = 1.5 * PI;

        // Interaction handling
        if response.dragged() {
            let delta = response.drag_delta();
            let sensitivity = if ui.input(|i| i.modifiers.shift) { 0.002 } else { 0.008 };
            let new_norm = (norm_val - delta.y * sensitivity).clamp(0.0, 1.0);
            let new_raw = (new_norm * 127.0).round() as u8;
            let new_bipolar = Bipolar7::from_raw(new_raw);
            if new_bipolar != *self.value {
                *self.value = new_bipolar;
                response.mark_changed();
            }
        }

        // Mouse wheel
        if response.hovered() {
            let scroll_delta = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll_delta.abs() > 0.0 {
                let step = if scroll_delta > 0.0 { 1i16 } else { -1i16 };
                let new_raw = (self.value.to_raw() as i16 + step).clamp(0, 127) as u8;
                let new_bipolar = Bipolar7::from_raw(new_raw);
                if new_bipolar != *self.value {
                    *self.value = new_bipolar;
                    response.mark_changed();
                }
            }
        }

        // Double click to reset to center (0)
        if response.double_clicked() {
            if self.value.as_i8() != 0 {
                *self.value = Bipolar7::new(0);
                response.mark_changed();
            }
        }

        if ui.is_rect_visible(rect) {
            let painter = ui.painter();
            let center = pos2(rect.center().x, rect.min.y + self.radius + font_height + 2.0);

            // 1. Draw top label
            let label_pos = pos2(rect.center().x, rect.min.y + font_height * 0.5);
            painter.text(
                label_pos,
                egui::Align2::CENTER_CENTER,
                self.label,
                egui::FontId::proportional(11.0),
                ui.visuals().text_color(),
            );

            // 2. Outer track background arc
            let track_radius = self.radius + 3.0;
            draw_arc(
                painter,
                center,
                track_radius,
                ARC_START_ANGLE,
                ARC_END_ANGLE,
                self.colors.track,
                2.5,
            );

            // 3. Center detent notch at 12 o'clock
            let detent_outer = center + vec2(0.0, -track_radius - 2.0);
            let detent_inner = center + vec2(0.0, -track_radius + 2.0);
            painter.line_segment(
                [detent_inner, detent_outer],
                Stroke::new(1.5, Color32::from_rgb(140, 145, 160)),
            );

            // 4. Bipolar active arc: from 12 o'clock towards current angle
            let current_angle = ARC_START_ANGLE + (norm_val * ARC_TOTAL_ANGLE);
            if current_i8 > 0 {
                draw_arc(
                    painter,
                    center,
                    track_radius,
                    center_angle,
                    current_angle,
                    self.colors.active_arc,
                    2.5,
                );
            } else if current_i8 < 0 {
                draw_arc(
                    painter,
                    center,
                    track_radius,
                    current_angle,
                    center_angle,
                    self.colors.active_arc,
                    2.5,
                );
            }

            // 5. Knob body
            painter.circle(
                center,
                self.radius,
                self.colors.body,
                Stroke::new(1.0, self.colors.border),
            );

            // 6. Needle indicator
            let needle_inner = center + vec2(current_angle.cos(), current_angle.sin()) * (self.radius * 0.35);
            let needle_outer = center + vec2(current_angle.cos(), current_angle.sin()) * (self.radius * 0.85);
            painter.line_segment(
                [needle_inner, needle_outer],
                Stroke::new(2.0, self.colors.needle),
            );

            // 7. Value display below knob
            let val_str = if current_i8 > 0 {
                format!("+{}", current_i8)
            } else {
                format!("{}", current_i8)
            };
            let val_pos = pos2(rect.center().x, rect.max.y - font_height * 0.5);
            painter.text(
                val_pos,
                egui::Align2::CENTER_CENTER,
                val_str,
                egui::FontId::monospace(10.0),
                if current_i8 == 0 {
                    Color32::from_rgb(120, 125, 140)
                } else {
                    Color32::from_rgb(220, 180, 140)
                },
            );
        }

        response
    }
}

/// Helper function to instantiate a UnipolarKnob
pub fn unipolar_knob<'a>(ui: &mut Ui, value: &'a mut Unipolar7, label: &'a str) -> Response {
    ui.add(UnipolarKnob::new(value, label))
}

/// Helper function to instantiate a BipolarKnob
pub fn bipolar_knob<'a>(ui: &mut Ui, value: &'a mut Bipolar7, label: &'a str) -> Response {
    ui.add(BipolarKnob::new(value, label))
}

/// Draws an arc using polyline segments on the painter.
fn draw_arc(
    painter: &egui::Painter,
    center: Pos2,
    radius: f32,
    start_angle: f32,
    end_angle: f32,
    color: Color32,
    width: f32,
) {
    if (end_angle - start_angle).abs() < 0.01 {
        return;
    }
    let steps = 24;
    let mut points = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let angle = start_angle + (end_angle - start_angle) * t;
        points.push(center + vec2(angle.cos(), angle.sin()) * radius);
    }
    painter.add(egui::Shape::line(points, Stroke::new(width, color)));
}

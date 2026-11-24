//! Rotary knob operating directly on a raw 7-bit parameter byte.
//!
//! Unipolar knobs fill the arc from the minimum; bipolar knobs (stored 0..127, shown
//! −64..+63) fill from the 12 o'clock detent at raw 64.

use crate::theme;
use egui::{pos2, vec2, Color32, Id, Pos2, Response, Sense, Stroke, Ui, Widget};
use std::f32::consts::PI;

const ARC_START_ANGLE: f32 = 0.75 * PI;
const ARC_TOTAL_ANGLE: f32 = 1.5 * PI;
const BIPOLAR_CENTRE: u8 = 64;
const WIDTH: f32 = 68.0;

const BODY: Color32 = theme::KNOB_BODY;
const BORDER: Color32 = theme::KNOB_BORDER;
const TRACK: Color32 = theme::KNOB_TRACK;
const ACTIVE: Color32 = theme::KNOB_ARC;
const NEEDLE: Color32 = theme::KNOB_POINTER;

pub struct Knob<'a> {
    value: &'a mut u8,
    label: &'a str,
    bipolar: bool,
    min: u8,
    max: u8,
    /// Value restored by double-click: 0, or the centre for bipolar knobs.
    default: u8,
    radius: f32,
}

impl<'a> Knob<'a> {
    pub fn unipolar(value: &'a mut u8, label: &'a str) -> Self {
        Self { value, label, bipolar: false, min: 0, max: 127, default: 0, radius: 16.0 }
    }

    pub fn bipolar(value: &'a mut u8, label: &'a str) -> Self {
        Self { value, label, bipolar: true, min: 0, max: 127, default: BIPOLAR_CENTRE, radius: 16.0 }
    }

    /// Range reachable by dragging. A stored value outside it is shown as-is and kept until
    /// the knob is moved.
    pub fn range(mut self, min: u8, max: u8) -> Self {
        self.min = min;
        self.max = max.max(min);
        self
    }

    fn normalized(&self, value: f32) -> f32 {
        let span = f32::from(self.max - self.min).max(1.0);
        ((value - f32::from(self.min)) / span).clamp(0.0, 1.0)
    }

    fn angle(&self, value: f32) -> f32 {
        ARC_START_ANGLE + self.normalized(value) * ARC_TOTAL_ANGLE
    }

    fn value_text(&self) -> String {
        if self.bipolar {
            match i16::from(*self.value) - 64 {
                v if v > 0 => format!("+{v}"),
                v => v.to_string(),
            }
        } else {
            self.value.to_string()
        }
    }
}

impl Widget for Knob<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let font_height = 12.0_f32;
        let size = vec2(WIDTH, self.radius * 2.0 + font_height * 2.0 + 8.0);
        let (rect, mut response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let span = f32::from(self.max - self.min).max(1.0);
        let accumulator = Id::new(("knob-drag", response.id));

        if response.drag_started() {
            ui.data_mut(|d| d.insert_temp(accumulator, f32::from(*self.value)));
        }
        if response.dragged() {
            let fine = ui.input(|i| i.modifiers.shift);
            // A full-range sweep takes ~150 px (600 px with Shift).
            let per_pixel = span / if fine { 600.0 } else { 150.0 };
            let delta = -response.drag_delta().y * per_pixel;
            let target = ui.data_mut(|d| {
                let acc = d.get_temp_mut_or(accumulator, f32::from(*self.value));
                *acc = (*acc + delta).clamp(f32::from(self.min), f32::from(self.max));
                *acc
            });
            let new_value = target.round() as u8;
            if new_value != *self.value {
                *self.value = new_value;
                response.mark_changed();
            }
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.raw_scroll_delta.y);
            if scroll != 0.0 {
                let current = (*self.value).clamp(self.min, self.max);
                let new_value = if scroll > 0.0 {
                    current.saturating_add(1).min(self.max)
                } else {
                    current.saturating_sub(1).max(self.min)
                };
                if new_value != *self.value {
                    *self.value = new_value;
                    response.mark_changed();
                }
            }
        }
        if response.double_clicked() && *self.value != self.default {
            *self.value = self.default;
            response.mark_changed();
        }

        if ui.is_rect_visible(rect) {
            let painter = ui.painter();
            let centre = pos2(rect.center().x, rect.min.y + font_height + 4.0 + self.radius);
            painter.text(
                pos2(rect.center().x, rect.min.y + font_height * 0.5),
                egui::Align2::CENTER_CENTER,
                self.label,
                egui::FontId::proportional(10.5),
                ui.visuals().text_color(),
            );

            let track_radius = self.radius + 3.0;
            draw_arc(painter, centre, track_radius, ARC_START_ANGLE, ARC_START_ANGLE + ARC_TOTAL_ANGLE, TRACK);
            let current = self.angle(f32::from(*self.value));
            if self.bipolar {
                let centre_angle = self.angle(f32::from(BIPOLAR_CENTRE));
                painter.line_segment(
                    [centre + vec2(0.0, -track_radius - 2.0), centre + vec2(0.0, -track_radius + 2.0)],
                    Stroke::new(1.5_f32, theme::DIM),
                );
                draw_arc(painter, centre, track_radius, centre_angle.min(current), centre_angle.max(current), ACTIVE);
            } else {
                draw_arc(painter, centre, track_radius, ARC_START_ANGLE, current, ACTIVE);
            }

            painter.circle(centre, self.radius, BODY, Stroke::new(1.0_f32, BORDER));
            let direction = vec2(current.cos(), current.sin());
            painter.line_segment(
                [centre + direction * self.radius * 0.35, centre + direction * self.radius * 0.85],
                Stroke::new(2.0_f32, NEEDLE),
            );

            let out_of_range = *self.value < self.min || *self.value > self.max;
            let neutral = if self.bipolar { *self.value == BIPOLAR_CENTRE } else { false };
            let colour = if out_of_range {
                theme::WARN
            } else if neutral {
                theme::DIM
            } else {
                theme::LEGEND
            };
            painter.text(
                pos2(rect.center().x, rect.max.y - font_height * 0.5),
                egui::Align2::CENTER_CENTER,
                self.value_text(),
                egui::FontId::monospace(10.0),
                colour,
            );
        }
        response
    }
}

fn draw_arc(painter: &egui::Painter, centre: Pos2, radius: f32, start: f32, end: f32, colour: Color32) {
    if (end - start).abs() < 0.01 {
        return;
    }
    const STEPS: usize = 24;
    let points = (0..=STEPS)
        .map(|i| {
            let angle = start + (end - start) * i as f32 / STEPS as f32;
            centre + vec2(angle.cos(), angle.sin()) * radius
        })
        .collect();
    painter.add(egui::Shape::line(points, Stroke::new(2.5_f32, colour)));
}

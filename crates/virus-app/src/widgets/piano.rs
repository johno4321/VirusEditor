//! On-screen piano keyboard for auditioning patches (SPEC §5.9).
//!
//! The widget only reports presses and releases; [`crate::audition::Audition`] turns them into
//! MIDI and tracks what is sounding.

use crate::theme;
use egui::{Align2, Color32, FontId, Pos2, Rect, Response, Rounding, Sense, Stroke, Ui, Vec2};
use virus_midi::notes::note_name;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PianoEvent {
    Press { note: u8, velocity: u8 },
    Release { note: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VelocityMode {
    /// Every key plays at this velocity (1..=127).
    Fixed(u8),
    /// Top of the key = soft, bottom = loud.
    FromPosition,
}

/// Black keys are this fraction of the keyboard height.
const BLACK_HEIGHT: f32 = 0.62;
const BLACK_WIDTH: f32 = 0.6;
/// Softest velocity when clicking at the very top of a key, so a click is always audible.
const MIN_POSITION_VELOCITY: u8 = 16;

pub fn is_black(note: u8) -> bool {
    matches!(note % 12, 1 | 3 | 6 | 8 | 10)
}

/// Key geometry in widget-local coordinates (origin top-left).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyLayout {
    /// Lowest note; rounded down to a C by [`KeyLayout::new`].
    pub first_note: u8,
    /// Highest note shown (inclusive), a C or the top of the MIDI range.
    pub last_note: u8,
    pub white_width: f32,
    pub height: f32,
}

impl KeyLayout {
    /// `octaves` octaves from the C at or below `first_note`, plus the top C.
    pub fn new(first_note: u8, octaves: u8, white_width: f32, height: f32) -> Self {
        let first_note = (first_note & 0x7F) - (first_note & 0x7F) % 12;
        let last_note = (u16::from(first_note) + 12 * u16::from(octaves.max(1))).min(127) as u8;
        Self {
            first_note,
            last_note,
            white_width,
            height,
        }
    }

    fn white_index(&self, note: u8) -> usize {
        (self.first_note..note).filter(|&n| !is_black(n)).count()
    }

    pub fn white_count(&self) -> usize {
        self.white_index(self.last_note) + usize::from(!is_black(self.last_note))
    }

    pub fn size(&self) -> Vec2 {
        Vec2::new(self.white_count() as f32 * self.white_width, self.height)
    }

    /// The key's rectangle relative to the widget's top-left corner.
    pub fn key_rect(&self, note: u8) -> Rect {
        if is_black(note) {
            let centre = self.white_index(note) as f32 * self.white_width;
            let w = self.white_width * BLACK_WIDTH;
            Rect::from_min_size(
                Pos2::new(centre - w / 2.0, 0.0),
                Vec2::new(w, self.height * BLACK_HEIGHT),
            )
        } else {
            Rect::from_min_size(
                Pos2::new(self.white_index(note) as f32 * self.white_width, 0.0),
                Vec2::new(self.white_width, self.height),
            )
        }
    }

    pub fn notes(&self) -> impl Iterator<Item = u8> {
        self.first_note..=self.last_note
    }

    /// The key under a widget-local position; black keys win where they overlap white ones.
    pub fn key_at(&self, pos: Pos2) -> Option<u8> {
        if pos.y < 0.0 || pos.y >= self.height || pos.x < 0.0 {
            return None;
        }
        if pos.y < self.height * BLACK_HEIGHT {
            if let Some(n) = self
                .notes()
                .filter(|&n| is_black(n))
                .find(|&n| self.key_rect(n).contains(pos))
            {
                return Some(n);
            }
        }
        let index = (pos.x / self.white_width) as usize;
        self.notes().filter(|&n| !is_black(n)).nth(index)
    }
}

/// Velocity for a click at fraction `y` (0 = top, 1 = bottom) of the key's height.
pub fn velocity_from_y(y: f32) -> u8 {
    let lo = f32::from(MIN_POSITION_VELOCITY);
    (lo + y.clamp(0.0, 1.0) * (127.0 - lo)).round() as u8
}

/// Piano keyboard widget. Click a key to play it, drag to glide across keys.
pub struct Piano {
    id_source: egui::Id,
    layout: KeyLayout,
    sounding: u128,
    velocity: VelocityMode,
    enabled: bool,
}

impl Piano {
    pub fn new(id_source: impl std::hash::Hash, first_note: u8, octaves: u8) -> Self {
        Self {
            id_source: egui::Id::new(id_source),
            layout: KeyLayout::new(first_note, octaves, 18.0, 70.0),
            sounding: 0,
            velocity: VelocityMode::FromPosition,
            enabled: true,
        }
    }

    /// Notes to draw as held (bit `n` = note `n`), e.g. `NoteTracker::sounding_mask`.
    pub fn sounding(mut self, mask: u128) -> Self {
        self.sounding = mask;
        self
    }

    pub fn velocity(mut self, mode: VelocityMode) -> Self {
        self.velocity = mode;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn show(self, ui: &mut Ui) -> (Response, Vec<PianoEvent>) {
        let layout = self.layout;
        let (rect, response) = ui.allocate_exact_size(layout.size(), Sense::click_and_drag());
        let id = ui.make_persistent_id(self.id_source);
        let previous: Option<u8> = ui.data(|d| d.get_temp(id)).flatten();

        let mut current = None;
        let mut velocity = 100;
        if self.enabled && response.is_pointer_button_down_on() {
            if let Some(pos) = response.interact_pointer_pos() {
                let local = (pos - rect.min).to_pos2();
                current = layout.key_at(local);
                if let Some(note) = current {
                    velocity = match self.velocity {
                        VelocityMode::Fixed(v) => v.clamp(1, 127),
                        VelocityMode::FromPosition => {
                            let key = layout.key_rect(note);
                            velocity_from_y((local.y - key.min.y) / key.height())
                        }
                    };
                }
            }
        }

        let mut events = Vec::new();
        if current != previous {
            if let Some(note) = previous {
                events.push(PianoEvent::Release { note });
            }
            if let Some(note) = current {
                events.push(PianoEvent::Press { note, velocity });
            }
            ui.data_mut(|d| d.insert_temp(id, current));
        }

        if ui.is_rect_visible(rect) {
            self.paint(ui, rect, current);
        }
        (response, events)
    }

    fn paint(&self, ui: &Ui, rect: Rect, mouse_note: Option<u8>) {
        let painter = ui.painter_at(rect);
        let held_colour = theme::KEY_HELD;
        let outline = Stroke::new(1.0_f32, theme::KEY_OUTLINE);
        let dim = |c: Color32| {
            if self.enabled {
                c
            } else {
                c.gamma_multiply(0.5)
            }
        };
        let held = |n: u8| self.sounding & (1u128 << n) != 0 || mouse_note == Some(n);
        let place = |n: u8| self.layout.key_rect(n).translate(rect.min.to_vec2());

        for n in self.layout.notes().filter(|&n| !is_black(n)) {
            let r = place(n);
            let fill = if held(n) {
                held_colour
            } else {
                theme::KEY_WHITE
            };
            painter.rect_filled(r, Rounding::same(2.0_f32), dim(fill));
            painter.rect_stroke(r, Rounding::same(2.0_f32), outline);
            if n % 12 == 0 {
                painter.text(
                    r.center_bottom() - Vec2::new(0.0, 3.0),
                    Align2::CENTER_BOTTOM,
                    note_name(n),
                    FontId::proportional(9.0),
                    dim(theme::KEY_LABEL),
                );
            }
        }
        for n in self.layout.notes().filter(|&n| is_black(n)) {
            let r = place(n);
            let fill = if held(n) {
                held_colour
            } else {
                theme::KEY_BLACK
            };
            painter.rect_filled(r, Rounding::same(2.0_f32), dim(fill));
            painter.rect_stroke(r, Rounding::same(2.0_f32), outline);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> KeyLayout {
        KeyLayout::new(48, 2, 10.0, 100.0)
    }

    #[test]
    fn layout_rounds_down_to_c_and_ends_on_c() {
        let l = KeyLayout::new(50, 2, 10.0, 100.0);
        assert_eq!((l.first_note, l.last_note), (48, 72));
        assert_eq!(l.white_count(), 15);
        assert_eq!(l.size(), Vec2::new(150.0, 100.0));
        let top = KeyLayout::new(120, 2, 10.0, 100.0);
        assert_eq!(top.last_note, 127);
    }

    #[test]
    fn key_at_finds_white_and_black_keys() {
        let l = layout();
        assert_eq!(l.key_at(Pos2::new(2.0, 90.0)), Some(48));
        assert_eq!(l.key_at(Pos2::new(12.0, 90.0)), Some(50));
        // Black C# straddles the C/D boundary at x = 10, upper part only.
        assert_eq!(l.key_at(Pos2::new(10.0, 20.0)), Some(49));
        assert_eq!(l.key_at(Pos2::new(10.0, 80.0)), Some(50));
        // No black key between E and F.
        assert_eq!(l.key_at(Pos2::new(29.0, 20.0)), Some(52));
        assert_eq!(l.key_at(Pos2::new(145.0, 90.0)), Some(72));
        assert_eq!(l.key_at(Pos2::new(151.0, 90.0)), None);
        assert_eq!(l.key_at(Pos2::new(5.0, 100.0)), None);
        assert_eq!(l.key_at(Pos2::new(-1.0, 50.0)), None);
    }

    #[test]
    fn every_key_is_reachable() {
        let l = layout();
        for n in l.notes() {
            let r = l.key_rect(n);
            let probe = Pos2::new(r.center().x, r.max.y - 1.0);
            assert_eq!(l.key_at(probe), Some(n), "note {n}");
        }
    }

    #[test]
    fn velocity_grows_towards_the_bottom() {
        assert_eq!(velocity_from_y(0.0), MIN_POSITION_VELOCITY);
        assert_eq!(velocity_from_y(1.0), 127);
        assert_eq!(velocity_from_y(2.0), 127);
        assert!(velocity_from_y(0.25) < velocity_from_y(0.75));
    }
}

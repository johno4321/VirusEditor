//! Note audition: play the synth from the editor to hear patch changes (SPEC §5.9).
//!
//! [`Audition`] owns the note state and turns piano clicks, computer keys, the Audition button
//! and test phrases into MIDI messages. The caller sends every returned message with
//! `MidiCommand::Send` (in order) and calls [`Audition::release_all`] on disconnect, part
//! change and exit, so no note is left hanging.

use crate::widgets::piano::{Piano, PianoEvent, VelocityMode};
use egui::{Event, Key, Modifiers, Ui};
use virus_midi::notes::{note_name, Message, NoteTracker, Phrase, PhrasePlayer, ReleaseTrigger};

/// "C3" on the Virus.
pub const MIDDLE_C: u8 = 60;
/// Octave shift range: the 13 computer-keyboard notes stay within 0..=127.
pub const MIN_OCTAVE: i8 = -5;
pub const MAX_OCTAVE: i8 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// Semitones above the current C (0..=12).
    Note(u8),
    OctaveDown,
    OctaveUp,
}

/// Computer-keyboard layout: `A W S E D F T G Y H U J K` = C..C, `Z`/`X` = octave down/up.
pub fn key_action(key: Key) -> Option<KeyAction> {
    let semitone = match key {
        Key::A => 0,
        Key::W => 1,
        Key::S => 2,
        Key::E => 3,
        Key::D => 4,
        Key::F => 5,
        Key::T => 6,
        Key::G => 7,
        Key::Y => 8,
        Key::H => 9,
        Key::U => 10,
        Key::J => 11,
        Key::K => 12,
        Key::Z => return Some(KeyAction::OctaveDown),
        Key::X => return Some(KeyAction::OctaveUp),
        _ => return None,
    };
    Some(KeyAction::Note(semitone))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestPhrase {
    Single,
    Chord,
    BassLine,
}

impl TestPhrase {
    pub const ALL: [TestPhrase; 3] = [TestPhrase::Single, TestPhrase::Chord, TestPhrase::BassLine];

    pub fn label(self) -> &'static str {
        match self {
            TestPhrase::Single => "Note",
            TestPhrase::Chord => "Chord",
            TestPhrase::BassLine => "Bass line",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditionSettings {
    /// Fixed velocity, used when `velocity_from_position` is off and for computer keys.
    pub velocity: u8,
    /// Piano velocity from the click height instead of `velocity`.
    pub velocity_from_position: bool,
    /// Notes played by the Audition button (the Virus' own Audition plays C3).
    pub audition_notes: Vec<u8>,
    pub audition_length_ms: u64,
    pub tempo_bpm: u16,
    pub octaves_shown: u8,
}

impl Default for AuditionSettings {
    fn default() -> Self {
        Self {
            velocity: 100,
            velocity_from_position: true,
            audition_notes: vec![MIDDLE_C],
            audition_length_ms: 1_000,
            tempo_bpm: 120,
            octaves_shown: 3,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Audition {
    pub settings: AuditionSettings,
    /// Whether the keyboard strip is expanded.
    pub open: bool,
    tracker: NoteTracker,
    player: PhrasePlayer,
    channel: u8,
    octave: i8,
    /// Computer keys currently down and the note each one started.
    keys_down: Vec<(Key, u8)>,
}

impl Default for Audition {
    fn default() -> Self {
        Self::new()
    }
}

impl Audition {
    pub fn new() -> Self {
        Self {
            settings: AuditionSettings::default(),
            open: true,
            tracker: NoteTracker::new(),
            player: PhrasePlayer::new(),
            channel: 0,
            octave: 0,
            keys_down: Vec::new(),
        }
    }

    /// MIDI channel (0..=15) notes are sent on.
    pub fn channel(&self) -> u8 {
        self.channel
    }

    pub fn octave(&self) -> i8 {
        self.octave
    }

    /// The note played by computer key `A`.
    pub fn base_note(&self) -> u8 {
        (i16::from(MIDDLE_C) + 12 * i16::from(self.octave)) as u8
    }

    pub fn latch(&self) -> bool {
        self.tracker.latch()
    }

    pub fn is_active(&self) -> bool {
        self.tracker.is_active() || self.player.is_playing()
    }

    pub fn sounding_mask(&self) -> u128 {
        self.tracker.sounding_mask(self.channel)
    }

    /// Changing the target channel (Single ↔ Multi, Global Channel, or the edited part's
    /// channel) releases everything on the old one first.
    pub fn set_channel(&mut self, channel: u8) -> Vec<Message> {
        let channel = channel & 0x0F;
        if channel == self.channel {
            return Vec::new();
        }
        let out = self.release_all(ReleaseTrigger::ChannelChange);
        self.channel = channel;
        out
    }

    pub fn shift_octave(&mut self, delta: i8) -> Vec<Message> {
        let octave = self
            .octave
            .saturating_add(delta)
            .clamp(MIN_OCTAVE, MAX_OCTAVE);
        if octave == self.octave {
            return Vec::new();
        }
        self.octave = octave;
        self.keys_down.clear();
        self.tracker.release_for(ReleaseTrigger::OctaveChange)
    }

    pub fn set_latch(&mut self, on: bool) -> Vec<Message> {
        self.tracker.set_latch(on)
    }

    /// Releases notes for a safety trigger. Call before sending `Disconnect` or quitting, so
    /// the note offs still reach the synth.
    pub fn release_all(&mut self, trigger: ReleaseTrigger) -> Vec<Message> {
        if trigger == ReleaseTrigger::OctaveChange {
            self.keys_down.clear();
            return self.tracker.release_for(trigger);
        }
        let mut out = self.stop_phrase();
        out.extend(self.tracker.release_for(trigger));
        self.keys_down.clear();
        out
    }

    /// The output port vanished: nothing can be sent, so just forget the state.
    pub fn forget_all(&mut self) {
        self.player.stop();
        self.tracker.forget_all();
        self.keys_down.clear();
    }

    /// Note offs for everything, then All Notes Off and All Sound Off.
    pub fn panic(&mut self) -> Vec<Message> {
        self.player.stop();
        self.keys_down.clear();
        self.tracker.panic(self.channel)
    }

    pub fn piano_events(&mut self, events: &[PianoEvent]) -> Vec<Message> {
        let mut out = Vec::new();
        for event in events {
            match *event {
                PianoEvent::Press { note, velocity } => {
                    out.extend(self.tracker.press(self.channel, note, velocity))
                }
                PianoEvent::Release { note } => {
                    out.extend(self.tracker.release(self.channel, note))
                }
            }
        }
        out
    }

    /// Handles computer-keyboard events. Presses are ignored while a text field has focus
    /// (`text_focus`), with Ctrl/Alt/Cmd held, and on key repeat; releases always count.
    /// Window focus loss releases everything.
    pub fn keyboard_events(&mut self, events: &[Event], text_focus: bool) -> Vec<Message> {
        let mut out = Vec::new();
        for event in events {
            match event {
                Event::Key {
                    key,
                    pressed,
                    repeat,
                    modifiers,
                    ..
                } => {
                    if !*pressed {
                        if let Some(i) = self.keys_down.iter().position(|(k, _)| k == key) {
                            let (_, note) = self.keys_down.remove(i);
                            out.extend(self.tracker.release(self.channel, note));
                        }
                    } else if !*repeat && !text_focus && !has_command_modifier(*modifiers) {
                        out.extend(self.key_pressed(*key));
                    }
                }
                Event::WindowFocused(false) => {
                    out.extend(self.release_all(ReleaseTrigger::FocusLost));
                }
                _ => {}
            }
        }
        out
    }

    fn key_pressed(&mut self, key: Key) -> Vec<Message> {
        if self.keys_down.iter().any(|(k, _)| *k == key) {
            return Vec::new();
        }
        match key_action(key) {
            Some(KeyAction::Note(semitone)) => {
                let note = self.base_note() + semitone;
                self.keys_down.push((key, note));
                self.tracker
                    .press(self.channel, note, self.settings.velocity)
            }
            Some(KeyAction::OctaveDown) => self.shift_octave(-1),
            Some(KeyAction::OctaveUp) => self.shift_octave(1),
            None => Vec::new(),
        }
    }

    /// The Audition button: plays the configured note/chord for the configured time.
    pub fn audition(&mut self, now_ms: u64) -> Vec<Message> {
        let s = &self.settings;
        let phrase = Phrase::chord(&s.audition_notes, s.velocity, s.audition_length_ms);
        self.play(&phrase, now_ms)
    }

    pub fn play_test_phrase(&mut self, kind: TestPhrase, now_ms: u64) -> Vec<Message> {
        let s = &self.settings;
        let beat = virus_midi::notes::beat_ms(s.tempo_bpm);
        let root = self.base_note();
        let phrase = match kind {
            TestPhrase::Single => Phrase::single(root, s.velocity, beat * 2),
            TestPhrase::Chord => Phrase::chord(&[root, root + 4, root + 7], s.velocity, beat * 4),
            TestPhrase::BassLine => {
                Phrase::bass_line(root.saturating_sub(12), s.velocity, s.tempo_bpm)
            }
        };
        self.play(&phrase, now_ms)
    }

    fn play(&mut self, phrase: &Phrase, now_ms: u64) -> Vec<Message> {
        let mut out = Vec::new();
        for e in self.player.start(phrase, self.channel, now_ms) {
            out.extend(self.tracker.apply(e));
        }
        out.extend(self.tick(now_ms));
        out
    }

    pub fn stop_phrase(&mut self) -> Vec<Message> {
        let mut out = Vec::new();
        for e in self.player.stop() {
            out.extend(self.tracker.apply(e));
        }
        out
    }

    pub fn is_playing_phrase(&self) -> bool {
        self.player.is_playing()
    }

    /// Advances the phrase player; call every frame.
    pub fn tick(&mut self, now_ms: u64) -> Vec<Message> {
        let mut out = Vec::new();
        for e in self.player.poll(now_ms) {
            out.extend(self.tracker.apply(e));
        }
        out
    }

    /// When the UI should wake up next to keep a phrase in time.
    pub fn repaint_after(&self, now_ms: u64) -> Option<std::time::Duration> {
        self.player
            .time_until_next(now_ms)
            .map(std::time::Duration::from_millis)
    }

    /// Draws the keyboard strip and returns the messages to send. `now_ms` is a monotonic
    /// clock (e.g. `ui.input(|i| i.time) * 1000`); `enabled` is false without MIDI output.
    pub fn show(&mut self, ui: &mut Ui, now_ms: u64, enabled: bool) -> Vec<Message> {
        let mut out = self.tick(now_ms);
        let text_focus = ui.ctx().wants_keyboard_input();
        let events = ui.input(|i| i.events.clone());
        if enabled {
            out.extend(self.keyboard_events(&events, text_focus));
        } else if self.is_active() {
            out.extend(self.release_all(ReleaseTrigger::Disconnect));
        }

        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("🎹 Ch {}", self.channel + 1));
                if ui.button("◀").on_hover_text("Octave down (Z)").clicked() {
                    out.extend(self.shift_octave(-1));
                }
                ui.label(note_name(self.base_note()));
                if ui.button("▶").on_hover_text("Octave up (X)").clicked() {
                    out.extend(self.shift_octave(1));
                }
                ui.separator();
                let mut latch = self.latch();
                if ui
                    .toggle_value(&mut latch, "Hold")
                    .on_hover_text("Keep notes on after release (pads, arpeggiator)")
                    .changed()
                {
                    out.extend(self.set_latch(latch));
                }
                ui.checkbox(&mut self.settings.velocity_from_position, "Vel. by height");
                ui.add_enabled(
                    !self.settings.velocity_from_position,
                    egui::Slider::new(&mut self.settings.velocity, 1..=127).text("Vel"),
                );
                ui.separator();
                if ui
                    .button("▶ Audition")
                    .on_hover_text(format!(
                        "Play {} for {} ms",
                        self.settings
                            .audition_notes
                            .iter()
                            .map(|&n| note_name(n))
                            .collect::<Vec<_>>()
                            .join("+"),
                        self.settings.audition_length_ms
                    ))
                    .clicked()
                {
                    out.extend(self.audition(now_ms));
                }
                for kind in TestPhrase::ALL {
                    if ui.small_button(kind.label()).clicked() {
                        out.extend(self.play_test_phrase(kind, now_ms));
                    }
                }
                ui.add(
                    egui::DragValue::new(&mut self.settings.tempo_bpm)
                        .clamp_range(20..=300)
                        .suffix(" bpm"),
                );
                if self.is_playing_phrase() && ui.small_button("■").clicked() {
                    out.extend(self.stop_phrase());
                }
                ui.separator();
                if ui
                    .button("🛑 Panic")
                    .on_hover_text("Note off for all notes, All Notes Off, All Sound Off")
                    .clicked()
                {
                    out.extend(self.panic());
                }
            });

            let velocity = if self.settings.velocity_from_position {
                VelocityMode::FromPosition
            } else {
                VelocityMode::Fixed(self.settings.velocity)
            };
            let first = self.base_note().saturating_sub(12);
            let (_, events) = Piano::new("audition_piano", first, self.settings.octaves_shown)
                .sounding(self.sounding_mask())
                .velocity(velocity)
                .enabled(enabled)
                .show(ui);
            out.extend(self.piano_events(&events));
        });

        if let Some(wait) = self.repaint_after(now_ms) {
            ui.ctx().request_repaint_after(wait);
        }
        out
    }
}

fn has_command_modifier(m: Modifiers) -> bool {
    m.ctrl || m.alt || m.command || m.mac_cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use virus_midi::notes::{all_notes_off, note_off, note_on};

    fn key(key: Key, pressed: bool, repeat: bool) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat,
            modifiers: Modifiers::NONE,
        }
    }

    #[test]
    fn computer_keys_map_to_one_octave() {
        let row = [
            Key::A,
            Key::W,
            Key::S,
            Key::E,
            Key::D,
            Key::F,
            Key::T,
            Key::G,
            Key::Y,
            Key::H,
            Key::U,
            Key::J,
            Key::K,
        ];
        for (i, k) in row.into_iter().enumerate() {
            assert_eq!(key_action(k), Some(KeyAction::Note(i as u8)));
        }
        assert_eq!(key_action(Key::Z), Some(KeyAction::OctaveDown));
        assert_eq!(key_action(Key::X), Some(KeyAction::OctaveUp));
        assert_eq!(key_action(Key::Q), None);
    }

    #[test]
    fn computer_key_plays_and_releases() {
        let mut a = Audition::new();
        a.set_channel(2);
        let on = a.keyboard_events(&[key(Key::A, true, false)], false);
        assert_eq!(on, vec![note_on(2, 60, 100)]);
        assert!(a
            .keyboard_events(&[key(Key::A, true, true), key(Key::A, true, false)], false)
            .is_empty());
        let off = a.keyboard_events(&[key(Key::A, false, false)], false);
        assert_eq!(off, vec![note_off(2, 60)]);
    }

    #[test]
    fn text_focus_blocks_presses_but_not_releases() {
        let mut a = Audition::new();
        assert!(a
            .keyboard_events(&[key(Key::A, true, false)], true)
            .is_empty());
        a.keyboard_events(&[key(Key::S, true, false)], false);
        let off = a.keyboard_events(&[key(Key::S, false, false)], true);
        assert_eq!(off, vec![note_off(0, 62)]);
    }

    #[test]
    fn octave_keys_shift_and_release_held_notes() {
        let mut a = Audition::new();
        a.keyboard_events(&[key(Key::A, true, false)], false);
        let out = a.keyboard_events(&[key(Key::X, true, false)], false);
        assert_eq!(out, vec![note_off(0, 60)]);
        assert_eq!(a.base_note(), 72);
        // The old key's key-up must not release anything at the new octave.
        assert!(a
            .keyboard_events(&[key(Key::A, false, false)], false)
            .is_empty());
        assert!(!a.is_active());
    }

    #[test]
    fn octave_is_clamped_to_midi_range() {
        let mut a = Audition::new();
        for _ in 0..20 {
            a.shift_octave(1);
        }
        assert_eq!(a.base_note() + 12, 120);
        for _ in 0..20 {
            a.shift_octave(-1);
        }
        assert_eq!(a.base_note(), 0);
    }

    #[test]
    fn focus_loss_and_channel_change_release_everything() {
        let mut a = Audition::new();
        a.set_latch(true);
        a.piano_events(&[
            PianoEvent::Press {
                note: 50,
                velocity: 90,
            },
            PianoEvent::Release { note: 50 },
        ]);
        assert!(a.is_active());
        let out = a.keyboard_events(&[Event::WindowFocused(false)], false);
        assert_eq!(out, vec![note_off(0, 50)]);

        a.piano_events(&[PianoEvent::Press {
            note: 50,
            velocity: 90,
        }]);
        assert_eq!(a.set_channel(4), vec![note_off(0, 50)]);
        assert!(!a.is_active());
    }

    #[test]
    fn audition_plays_for_its_length() {
        let mut a = Audition::new();
        a.settings.audition_notes = vec![60, 64];
        a.settings.audition_length_ms = 500;
        let out = a.audition(1_000);
        assert_eq!(out, vec![note_on(0, 60, 100), note_on(0, 64, 100)]);
        assert!(a.tick(1_499).is_empty());
        assert_eq!(a.tick(1_500), vec![note_off(0, 60), note_off(0, 64)]);
        assert!(!a.is_active());
    }

    #[test]
    fn panic_stops_phrase_and_clears_channel() {
        let mut a = Audition::new();
        a.play_test_phrase(TestPhrase::BassLine, 0);
        let out = a.panic();
        assert!(out.contains(&note_off(0, 48)));
        assert!(out.ends_with(&[all_notes_off(0), virus_midi::notes::all_sound_off(0)]));
        assert!(!a.is_active());
        assert!(a.tick(10_000).is_empty());
    }
}

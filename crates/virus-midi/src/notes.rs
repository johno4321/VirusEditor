//! Playing notes on the synth for auditioning patches (SPEC §5.9).
//!
//! Everything here is pure: builders return raw messages for [`crate::MidiCommand::Send`],
//! [`NoteTracker`] remembers what is sounding so nothing gets stuck, and [`PhrasePlayer`]
//! schedules test phrases against a clock the caller passes in.
//!
//! Channels are 0..=15 and notes/values 0..=127; out-of-range inputs are masked.

/// A raw MIDI message, ready for [`crate::MidiCommand::Send`].
pub type Message = Vec<u8>;

pub const CC_HOLD_PEDAL: u8 = 64;
pub const CC_ALL_SOUND_OFF: u8 = 120;
pub const CC_ALL_NOTES_OFF: u8 = 123;
/// Velocity sent with note offs (the MIDI "no release velocity" default).
pub const RELEASE_VELOCITY: u8 = 0x40;

/// Note On. Velocity is clamped to 1..=127, because velocity 0 would mean note off.
pub fn note_on(channel: u8, note: u8, velocity: u8) -> Message {
    vec![0x90 | (channel & 0x0F), note & 0x7F, velocity.clamp(1, 127)]
}

/// Note Off (`8c nn 40`), never the "note on with velocity 0" form.
pub fn note_off(channel: u8, note: u8) -> Message {
    vec![0x80 | (channel & 0x0F), note & 0x7F, RELEASE_VELOCITY]
}

pub fn control_change(channel: u8, controller: u8, value: u8) -> Message {
    vec![0xB0 | (channel & 0x0F), controller & 0x7F, value & 0x7F]
}

/// CC64: `true` = pedal down (127), `false` = up (0).
pub fn hold_pedal(channel: u8, down: bool) -> Message {
    control_change(channel, CC_HOLD_PEDAL, if down { 127 } else { 0 })
}

/// CC123 All Notes Off (A123).
pub fn all_notes_off(channel: u8) -> Message {
    control_change(channel, CC_ALL_NOTES_OFF, 0)
}

/// CC120 All Sound Off: also cuts release tails.
pub fn all_sound_off(channel: u8) -> Message {
    control_change(channel, CC_ALL_SOUND_OFF, 0)
}

/// Note name in the Virus convention, where note 60 is "C3".
pub fn note_name(note: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let note = note & 0x7F;
    format!(
        "{}{}",
        NAMES[usize::from(note % 12)],
        i32::from(note / 12) - 2
    )
}

/// A note starting or stopping on a channel, as produced by [`PhrasePlayer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteEvent {
    On { channel: u8, note: u8, velocity: u8 },
    Off { channel: u8, note: u8 },
}

/// Situations in which held notes must be released so that none get stuck (SPEC §5.9 item 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseTrigger {
    /// Pressed keys are released; latched notes keep sounding.
    OctaveChange,
    ChannelChange,
    PartChange,
    /// Call before sending `Disconnect`, so the note offs still reach the synth.
    Disconnect,
    FocusLost,
    Exit,
}

impl ReleaseTrigger {
    pub const ALL: [ReleaseTrigger; 6] = [
        ReleaseTrigger::OctaveChange,
        ReleaseTrigger::ChannelChange,
        ReleaseTrigger::PartChange,
        ReleaseTrigger::Disconnect,
        ReleaseTrigger::FocusLost,
        ReleaseTrigger::Exit,
    ];
}

/// Remembers which notes the editor has switched on, so each one gets a note off.
///
/// *Pressed* notes are keys the user is holding down; *sounding* notes are those with an
/// outstanding note on. With latch on, a released key keeps sounding until it is pressed again
/// or latch is turned off.
#[derive(Debug, Clone, Default)]
pub struct NoteTracker {
    sounding: [u128; 16],
    pressed: [u128; 16],
    pedal_down: u16,
    latch: bool,
}

fn bit(note: u8) -> u128 {
    1u128 << (note & 0x7F)
}

fn notes_in(mask: u128) -> impl Iterator<Item = u8> {
    (0u8..128).filter(move |&n| mask & bit(n) != 0)
}

impl NoteTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn latch(&self) -> bool {
        self.latch
    }

    pub fn is_sounding(&self, channel: u8, note: u8) -> bool {
        self.sounding[usize::from(channel & 0x0F)] & bit(note) != 0
    }

    pub fn is_pressed(&self, channel: u8, note: u8) -> bool {
        self.pressed[usize::from(channel & 0x0F)] & bit(note) != 0
    }

    /// Sounding notes on `channel`, ascending.
    pub fn sounding_notes(&self, channel: u8) -> Vec<u8> {
        notes_in(self.sounding[usize::from(channel & 0x0F)]).collect()
    }

    /// Bitset of sounding notes on `channel` (bit `n` = note `n`), handy for drawing keys.
    pub fn sounding_mask(&self, channel: u8) -> u128 {
        self.sounding[usize::from(channel & 0x0F)]
    }

    /// True if any note is sounding or a hold pedal is down on any channel.
    pub fn is_active(&self) -> bool {
        self.pedal_down != 0 || self.sounding.iter().any(|&m| m != 0)
    }

    /// A key went down. With latch on, pressing a latched note switches it off instead.
    pub fn press(&mut self, channel: u8, note: u8, velocity: u8) -> Vec<Message> {
        let ch = usize::from(channel & 0x0F);
        let b = bit(note);
        self.pressed[ch] |= b;
        if self.latch && self.sounding[ch] & b != 0 {
            self.sounding[ch] &= !b;
            return vec![note_off(channel, note)];
        }
        self.note_on(channel, note, velocity)
    }

    /// A key went up. Ignored for keys that aren't pressed (e.g. after an octave change).
    pub fn release(&mut self, channel: u8, note: u8) -> Vec<Message> {
        let ch = usize::from(channel & 0x0F);
        let b = bit(note);
        if self.pressed[ch] & b == 0 {
            return Vec::new();
        }
        self.pressed[ch] &= !b;
        if self.latch {
            return Vec::new();
        }
        self.note_off(channel, note)
    }

    /// Switches a note on regardless of latch, retriggering it if it is already sounding.
    pub fn note_on(&mut self, channel: u8, note: u8, velocity: u8) -> Vec<Message> {
        let ch = usize::from(channel & 0x0F);
        let b = bit(note);
        let mut out = Vec::with_capacity(2);
        if self.sounding[ch] & b != 0 {
            out.push(note_off(channel, note));
        }
        self.sounding[ch] |= b;
        out.push(note_on(channel, note, velocity));
        out
    }

    /// Switches a note off if it is sounding (no message otherwise).
    pub fn note_off(&mut self, channel: u8, note: u8) -> Vec<Message> {
        let ch = usize::from(channel & 0x0F);
        let b = bit(note);
        if self.sounding[ch] & b == 0 {
            return Vec::new();
        }
        self.sounding[ch] &= !b;
        vec![note_off(channel, note)]
    }

    pub fn apply(&mut self, event: NoteEvent) -> Vec<Message> {
        match event {
            NoteEvent::On {
                channel,
                note,
                velocity,
            } => self.note_on(channel, note, velocity),
            NoteEvent::Off { channel, note } => self.note_off(channel, note),
        }
    }

    /// Turning latch off releases latched notes whose keys are no longer held.
    pub fn set_latch(&mut self, on: bool) -> Vec<Message> {
        self.latch = on;
        if on {
            return Vec::new();
        }
        let mut out = Vec::new();
        for ch in 0..16u8 {
            let i = usize::from(ch);
            let stale = self.sounding[i] & !self.pressed[i];
            out.extend(notes_in(stale).map(|n| note_off(ch, n)));
            self.sounding[i] &= self.pressed[i];
        }
        out
    }

    /// Sends CC64 and remembers it, so [`Self::all_off`] lifts the pedal again.
    pub fn set_hold_pedal(&mut self, channel: u8, down: bool) -> Vec<Message> {
        let b = 1u16 << (channel & 0x0F);
        if down {
            self.pedal_down |= b;
        } else {
            self.pedal_down &= !b;
        }
        vec![hold_pedal(channel, down)]
    }

    /// Releases every pressed key as if it had gone up (latch rules apply).
    pub fn release_pressed(&mut self) -> Vec<Message> {
        let mut out = Vec::new();
        for ch in 0..16u8 {
            for n in notes_in(self.pressed[usize::from(ch)]) {
                out.extend(self.release(ch, n));
            }
        }
        out
    }

    /// Note off for everything sounding, and lifts any hold pedal the editor put down.
    pub fn all_off(&mut self) -> Vec<Message> {
        let mut out = Vec::new();
        for ch in 0..16u8 {
            let i = usize::from(ch);
            out.extend(notes_in(self.sounding[i]).map(|n| note_off(ch, n)));
            if self.pedal_down & (1 << ch) != 0 {
                out.push(hold_pedal(ch, false));
            }
        }
        self.forget_all();
        out
    }

    pub fn release_for(&mut self, trigger: ReleaseTrigger) -> Vec<Message> {
        match trigger {
            ReleaseTrigger::OctaveChange => self.release_pressed(),
            _ => self.all_off(),
        }
    }

    /// [`Self::all_off`], then CC123 All Notes Off and CC120 All Sound Off on `channel` and
    /// on every other channel that had notes or a pedal down.
    pub fn panic(&mut self, channel: u8) -> Vec<Message> {
        let mut channels = 1u16 << (channel & 0x0F);
        for ch in 0..16u8 {
            if self.sounding[usize::from(ch)] != 0 || self.pedal_down & (1 << ch) != 0 {
                channels |= 1 << ch;
            }
        }
        let mut out = self.all_off();
        for ch in (0..16u8).filter(|ch| channels & (1 << ch) != 0) {
            out.push(all_notes_off(ch));
            out.push(all_sound_off(ch));
        }
        out
    }

    /// Clears the state without sending anything, e.g. when the output port vanished.
    pub fn forget_all(&mut self) {
        self.sounding = [0; 16];
        self.pressed = [0; 16];
        self.pedal_down = 0;
    }
}

/// One note of a [`Phrase`], timed relative to the phrase start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhraseNote {
    pub start_ms: u64,
    pub note: u8,
    pub velocity: u8,
    pub length_ms: u64,
}

/// A short sequence of notes for auditioning, e.g. the Audition chord or a bass line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Phrase {
    pub notes: Vec<PhraseNote>,
}

/// Length of one beat (a quarter note) in ms; `bpm` is clamped to 20..=300.
pub fn beat_ms(bpm: u16) -> u64 {
    60_000 / u64::from(bpm.clamp(20, 300))
}

fn transpose(note: u8, semitones: i16) -> u8 {
    (i16::from(note) + semitones).clamp(0, 127) as u8
}

impl Phrase {
    pub fn single(note: u8, velocity: u8, length_ms: u64) -> Self {
        Self::chord(&[note], velocity, length_ms)
    }

    /// All `notes` together, like the Virus' own Audition (which plays C3 = 60).
    pub fn chord(notes: &[u8], velocity: u8, length_ms: u64) -> Self {
        Self {
            notes: notes
                .iter()
                .map(|&note| PhraseNote {
                    start_ms: 0,
                    note: note & 0x7F,
                    velocity,
                    length_ms,
                })
                .collect(),
        }
    }

    /// Steps of `(start beat, semitones from root, length in beats)` at `bpm`.
    pub fn from_beats(root: u8, velocity: u8, bpm: u16, steps: &[(f32, i16, f32)]) -> Self {
        let beat = beat_ms(bpm) as f32;
        Self {
            notes: steps
                .iter()
                .map(|&(start, offset, length)| PhraseNote {
                    start_ms: (start.max(0.0) * beat).round() as u64,
                    note: transpose(root, offset),
                    velocity,
                    length_ms: (length.max(0.0) * beat).round() as u64,
                })
                .collect(),
        }
    }

    /// A one-bar eighth-note bass line on `root` (root, octave, fifth, minor seventh).
    pub fn bass_line(root: u8, velocity: u8, bpm: u16) -> Self {
        const STEPS: [(f32, i16, f32); 8] = [
            (0.0, 0, 0.4),
            (0.5, 0, 0.4),
            (1.0, 12, 0.4),
            (1.5, 0, 0.4),
            (2.0, 7, 0.4),
            (2.5, 0, 0.4),
            (3.0, 10, 0.4),
            (3.5, 12, 0.4),
        ];
        Self::from_beats(root, velocity, bpm, &STEPS)
    }

    /// Time from the first note on to the last note off.
    pub fn duration_ms(&self) -> u64 {
        self.notes
            .iter()
            .map(|n| n.start_ms + n.length_ms.max(1))
            .max()
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy)]
struct Scheduled {
    at_ms: u64,
    note: u8,
    /// `None` = note off.
    velocity: Option<u8>,
}

/// Plays a [`Phrase`] against a millisecond clock supplied by the caller (e.g. the GUI frame
/// time): call [`Self::poll`] regularly and send the returned events through a [`NoteTracker`].
#[derive(Debug, Clone, Default)]
pub struct PhrasePlayer {
    events: Vec<Scheduled>,
    next: usize,
    start_ms: u64,
    channel: u8,
    sounding: u128,
}

impl PhrasePlayer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts `phrase` on `channel` at `now_ms`. A phrase already playing is stopped first and
    /// its note offs are returned (the new phrase's first notes come from the next poll).
    pub fn start(&mut self, phrase: &Phrase, channel: u8, now_ms: u64) -> Vec<NoteEvent> {
        let stopped = self.stop();
        let mut events: Vec<Scheduled> = phrase
            .notes
            .iter()
            .flat_map(|n| {
                [
                    Scheduled {
                        at_ms: n.start_ms,
                        note: n.note & 0x7F,
                        velocity: Some(n.velocity),
                    },
                    Scheduled {
                        at_ms: n.start_ms + n.length_ms.max(1),
                        note: n.note & 0x7F,
                        velocity: None,
                    },
                ]
            })
            .collect();
        // Note offs first at equal times, so a repeated note is retriggered, not cut.
        events.sort_by_key(|e| (e.at_ms, e.velocity.is_some()));
        self.events = events;
        self.next = 0;
        self.start_ms = now_ms;
        self.channel = channel & 0x0F;
        stopped
    }

    pub fn is_playing(&self) -> bool {
        self.next < self.events.len()
    }

    pub fn channel(&self) -> u8 {
        self.channel
    }

    /// Events due at `now_ms`, in order. Late polls catch up on everything missed.
    pub fn poll(&mut self, now_ms: u64) -> Vec<NoteEvent> {
        let elapsed = now_ms.saturating_sub(self.start_ms);
        let mut out = Vec::new();
        while let Some(e) = self.events.get(self.next).copied() {
            if e.at_ms > elapsed {
                break;
            }
            self.next += 1;
            let channel = self.channel;
            match e.velocity {
                Some(velocity) => {
                    self.sounding |= bit(e.note);
                    out.push(NoteEvent::On {
                        channel,
                        note: e.note,
                        velocity,
                    });
                }
                None => {
                    if self.sounding & bit(e.note) != 0 {
                        self.sounding &= !bit(e.note);
                        out.push(NoteEvent::Off {
                            channel,
                            note: e.note,
                        });
                    }
                }
            }
        }
        if !self.is_playing() {
            self.events.clear();
            self.next = 0;
        }
        out
    }

    /// Milliseconds from `now_ms` until the next event, for scheduling a repaint.
    pub fn time_until_next(&self, now_ms: u64) -> Option<u64> {
        let e = self.events.get(self.next)?;
        Some((self.start_ms + e.at_ms).saturating_sub(now_ms))
    }

    /// Stops playback and returns note offs for the notes it left sounding.
    pub fn stop(&mut self) -> Vec<NoteEvent> {
        let channel = self.channel;
        let out = notes_in(self.sounding)
            .map(|note| NoteEvent::Off { channel, note })
            .collect();
        self.sounding = 0;
        self.events.clear();
        self.next = 0;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builders_produce_channel_messages() {
        assert_eq!(note_on(0, 60, 100), vec![0x90, 60, 100]);
        assert_eq!(note_on(15, 127, 127), vec![0x9F, 127, 127]);
        assert_eq!(note_off(3, 60), vec![0x83, 60, 0x40]);
        assert_eq!(control_change(1, 74, 5), vec![0xB1, 74, 5]);
        assert_eq!(all_notes_off(2), vec![0xB2, 123, 0]);
        assert_eq!(all_sound_off(2), vec![0xB2, 120, 0]);
        assert_eq!(hold_pedal(0, true), vec![0xB0, 64, 127]);
        assert_eq!(hold_pedal(0, false), vec![0xB0, 64, 0]);
    }

    #[test]
    fn builders_keep_messages_valid() {
        // Velocity 0 would be a note off; data bytes must stay below 0x80.
        assert_eq!(note_on(0, 60, 0), vec![0x90, 60, 1]);
        assert_eq!(note_on(0x13, 0xBC, 200), vec![0x93, 0x3C, 127]);
        assert_eq!(control_change(0, 0xFF, 0xFF), vec![0xB0, 0x7F, 0x7F]);
    }

    #[test]
    fn note_names_follow_virus_convention() {
        assert_eq!(note_name(60), "C3");
        assert_eq!(note_name(0), "C-2");
        assert_eq!(note_name(61), "C#3");
        assert_eq!(note_name(127), "G8");
    }

    #[test]
    fn press_and_release_pair_up() {
        let mut t = NoteTracker::new();
        assert_eq!(t.press(0, 60, 90), vec![note_on(0, 60, 90)]);
        assert!(t.is_sounding(0, 60));
        assert_eq!(t.release(0, 60), vec![note_off(0, 60)]);
        assert!(!t.is_active());
        assert!(t.release(0, 60).is_empty(), "no second note off");
    }

    #[test]
    fn retrigger_sends_note_off_first() {
        let mut t = NoteTracker::new();
        t.note_on(0, 60, 90);
        assert_eq!(
            t.note_on(0, 60, 50),
            vec![note_off(0, 60), note_on(0, 60, 50)]
        );
    }

    #[test]
    fn latch_keeps_notes_until_pressed_again() {
        let mut t = NoteTracker::new();
        t.set_latch(true);
        t.press(0, 60, 90);
        assert!(t.release(0, 60).is_empty());
        assert!(t.is_sounding(0, 60));
        assert_eq!(t.press(0, 60, 90), vec![note_off(0, 60)]);
        assert!(t.release(0, 60).is_empty());
        assert!(!t.is_active());
    }

    #[test]
    fn latch_off_releases_only_unheld_notes() {
        let mut t = NoteTracker::new();
        t.set_latch(true);
        t.press(0, 60, 90);
        t.release(0, 60);
        t.press(0, 64, 90);
        assert_eq!(t.set_latch(false), vec![note_off(0, 60)]);
        assert!(t.is_sounding(0, 64));
        assert_eq!(t.release(0, 64), vec![note_off(0, 64)]);
        assert!(!t.is_active());
    }

    /// Sets up pressed, latched, phrase and pedal notes on two channels.
    fn busy_tracker(latch: bool) -> NoteTracker {
        let mut t = NoteTracker::new();
        t.set_latch(latch);
        t.press(0, 60, 100);
        t.press(0, 64, 100);
        t.release(0, 64);
        t.press(5, 48, 100);
        t.note_on(5, 72, 100);
        t.set_hold_pedal(5, true);
        t
    }

    #[test]
    fn every_trigger_except_octave_change_silences_everything() {
        for trigger in ReleaseTrigger::ALL {
            if trigger == ReleaseTrigger::OctaveChange {
                continue;
            }
            for latch in [false, true] {
                let mut t = busy_tracker(latch);
                let out = t.release_for(trigger);
                assert!(!t.is_active(), "{trigger:?} latch={latch}");
                assert!(out.contains(&note_off(0, 60)));
                assert!(out.contains(&note_off(5, 48)));
                assert!(out.contains(&note_off(5, 72)));
                assert!(out.contains(&hold_pedal(5, false)));
                // Keys that come up afterwards produce nothing.
                assert!(t.release(0, 60).is_empty());
            }
        }
    }

    #[test]
    fn octave_change_releases_pressed_keys() {
        let mut t = busy_tracker(false);
        let out = t.release_for(ReleaseTrigger::OctaveChange);
        assert!(out.contains(&note_off(0, 60)));
        assert!(out.contains(&note_off(5, 48)));
        assert!(!t.is_sounding(0, 60) && !t.is_sounding(5, 48));
        // The old key's key-up maps to a new note after the shift: nothing to do.
        assert!(t.release(0, 72).is_empty());
        assert!(t.release(0, 60).is_empty());
    }

    #[test]
    fn octave_change_keeps_latched_notes() {
        let mut t = busy_tracker(true);
        assert!(t.release_for(ReleaseTrigger::OctaveChange).is_empty());
        assert!(t.is_sounding(0, 60) && t.is_sounding(0, 64));
        assert!(!t.is_pressed(0, 60));
    }

    #[test]
    fn panic_sends_note_offs_then_controllers() {
        let mut t = busy_tracker(false);
        let out = t.panic(1);
        assert!(!t.is_active());
        let first_cc = out.iter().position(|m| m[0] & 0xF0 == 0xB0).unwrap();
        assert!(out[..first_cc].iter().all(|m| m[0] & 0xF0 == 0x80));
        for ch in [0, 1, 5] {
            assert!(out.contains(&all_notes_off(ch)));
            assert!(out.contains(&all_sound_off(ch)));
        }
        assert!(!out.contains(&all_notes_off(2)));
    }

    #[test]
    fn panic_on_idle_tracker_still_clears_target_channel() {
        let mut t = NoteTracker::new();
        assert_eq!(t.panic(3), vec![all_notes_off(3), all_sound_off(3)]);
    }

    #[test]
    fn forget_all_sends_nothing() {
        let mut t = busy_tracker(false);
        t.forget_all();
        assert!(!t.is_active());
        assert!(t.all_off().is_empty());
    }

    #[test]
    fn phrase_plays_notes_on_time() {
        let mut p = PhrasePlayer::new();
        let phrase = Phrase {
            notes: vec![
                PhraseNote {
                    start_ms: 0,
                    note: 60,
                    velocity: 100,
                    length_ms: 100,
                },
                PhraseNote {
                    start_ms: 100,
                    note: 62,
                    velocity: 80,
                    length_ms: 50,
                },
            ],
        };
        assert!(p.start(&phrase, 2, 1_000).is_empty());
        let on60 = NoteEvent::On {
            channel: 2,
            note: 60,
            velocity: 100,
        };
        assert_eq!(p.poll(1_000), vec![on60]);
        assert!(p.poll(1_099).is_empty());
        assert_eq!(p.time_until_next(1_099), Some(1));
        assert_eq!(
            p.poll(1_100),
            vec![
                NoteEvent::Off {
                    channel: 2,
                    note: 60
                },
                NoteEvent::On {
                    channel: 2,
                    note: 62,
                    velocity: 80
                },
            ]
        );
        assert!(p.is_playing());
        assert_eq!(
            p.poll(1_150),
            vec![NoteEvent::Off {
                channel: 2,
                note: 62
            }]
        );
        assert!(!p.is_playing());
        assert_eq!(p.time_until_next(1_150), None);
    }

    #[test]
    fn late_poll_catches_up_and_leaves_nothing_on() {
        let mut p = PhrasePlayer::new();
        p.start(&Phrase::bass_line(36, 100, 120), 0, 0);
        let events = p.poll(10_000);
        let ons = events
            .iter()
            .filter(|e| matches!(e, NoteEvent::On { .. }))
            .count();
        let offs = events.len() - ons;
        assert_eq!((ons, offs), (8, 8));
        assert!(!p.is_playing());
        assert!(p.stop().is_empty());
    }

    #[test]
    fn repeated_note_is_retriggered_not_cut() {
        let mut p = PhrasePlayer::new();
        let phrase = Phrase {
            notes: vec![
                PhraseNote {
                    start_ms: 0,
                    note: 60,
                    velocity: 100,
                    length_ms: 100,
                },
                PhraseNote {
                    start_ms: 100,
                    note: 60,
                    velocity: 100,
                    length_ms: 100,
                },
            ],
        };
        p.start(&phrase, 0, 0);
        p.poll(0);
        let at_100 = p.poll(100);
        assert!(matches!(at_100[0], NoteEvent::Off { .. }));
        assert!(matches!(at_100[1], NoteEvent::On { .. }));
    }

    #[test]
    fn stop_and_restart_release_sounding_notes() {
        let mut p = PhrasePlayer::new();
        p.start(&Phrase::chord(&[60, 64, 67], 100, 1_000), 1, 0);
        p.poll(10);
        let mut t = NoteTracker::new();
        let restart = p.start(&Phrase::single(48, 100, 500), 1, 20);
        assert_eq!(restart.len(), 3);
        p.poll(20);
        let offs = p.stop();
        assert_eq!(
            offs,
            vec![NoteEvent::Off {
                channel: 1,
                note: 48
            }]
        );
        // Through a tracker, an off for a note that was never sent is dropped.
        assert!(t.apply(offs[0]).is_empty());
    }

    #[test]
    fn phrase_builders() {
        let chord = Phrase::chord(&[60, 64, 67], 100, 1_000);
        assert_eq!(chord.notes.len(), 3);
        assert_eq!(chord.duration_ms(), 1_000);
        assert_eq!(beat_ms(120), 500);
        let bass = Phrase::bass_line(36, 100, 120);
        assert_eq!(bass.notes[2].note, 48);
        assert_eq!(bass.notes[2].start_ms, 500);
        assert_eq!(bass.duration_ms(), 1_950);
        let high = Phrase::bass_line(120, 100, 120);
        assert!(high.notes.iter().all(|n| n.note <= 127));
    }

    #[test]
    fn zero_length_notes_still_get_a_note_off() {
        let mut p = PhrasePlayer::new();
        p.start(&Phrase::single(60, 100, 0), 0, 0);
        assert_eq!(p.poll(0).len(), 1);
        assert_eq!(p.poll(1).len(), 1);
        assert!(!p.is_playing());
    }
}

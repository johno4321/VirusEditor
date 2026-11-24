//! Incremental MIDI 1.0 byte-stream parser.
//!
//! Turns arbitrary input chunks into [`MidiEvent`]s: Control Change, Poly Pressure,
//! Program Change and complete SysEx frames. Handles several messages per chunk,
//! running status, SysEx split across chunks and real-time bytes anywhere in the stream.

use crate::events::MidiEvent;

pub const SYSEX_START: u8 = 0xF0;
pub const SYSEX_END: u8 = 0xF7;

/// Largest SysEx frame (including `F0`/`F7`) accepted before the frame is discarded.
pub const MAX_SYSEX_LEN: usize = 64 * 1024;

/// Stateful parser for incoming MIDI bytes. One instance per input connection.
#[derive(Debug, Default)]
pub struct MidiInputParser {
    running_status: Option<u8>,
    data: [u8; 2],
    data_len: usize,
    sysex: Option<Vec<u8>>,
}

impl MidiInputParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds a chunk of raw MIDI bytes and returns the events it completes.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<MidiEvent> {
        let mut events = Vec::new();
        for &byte in chunk {
            if let Some(event) = self.push(byte) {
                events.push(event);
            }
        }
        events
    }

    fn push(&mut self, byte: u8) -> Option<MidiEvent> {
        match byte {
            // System real-time: may appear anywhere, never affects parser state.
            0xF8..=0xFF => None,
            SYSEX_START => {
                self.running_status = None;
                self.data_len = 0;
                let mut buf = Vec::with_capacity(512);
                buf.push(byte);
                self.sysex = Some(buf);
                None
            }
            SYSEX_END => {
                self.running_status = None;
                self.data_len = 0;
                self.sysex.take().map(|mut buf| {
                    buf.push(byte);
                    MidiEvent::SysEx(buf)
                })
            }
            // Any other status byte aborts an unterminated SysEx (MIDI 1.0).
            0x80..=0xEF => {
                self.sysex = None;
                self.running_status = Some(byte);
                self.data_len = 0;
                None
            }
            // System common (F1..F6): clears running status; its data bytes are ignored.
            0xF1..=0xF6 => {
                self.sysex = None;
                self.running_status = None;
                self.data_len = 0;
                None
            }
            _ => self.push_data(byte),
        }
    }

    fn push_data(&mut self, byte: u8) -> Option<MidiEvent> {
        if let Some(buf) = self.sysex.as_mut() {
            if buf.len() < MAX_SYSEX_LEN - 1 {
                buf.push(byte);
            } else {
                // Runaway frame: drop it; following data bytes are ignored until a new status.
                self.sysex = None;
            }
            return None;
        }

        let status = self.running_status?;
        self.data[self.data_len] = byte;
        self.data_len += 1;
        if self.data_len < data_bytes_for(status) {
            return None;
        }
        self.data_len = 0;

        let channel = status & 0x0F;
        let [d1, d2] = self.data;
        match status & 0xF0 {
            0xA0 => Some(MidiEvent::PolyPressure {
                channel,
                note: d1,
                pressure: d2,
            }),
            0xB0 => Some(MidiEvent::ControlChange {
                channel,
                controller: d1,
                value: d2,
            }),
            0xC0 => Some(MidiEvent::ProgramChange {
                channel,
                program: d1,
            }),
            _ => None,
        }
    }
}

fn data_bytes_for(status: u8) -> usize {
    match status & 0xF0 {
        0xC0 | 0xD0 => 1,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cc(channel: u8, controller: u8, value: u8) -> MidiEvent {
        MidiEvent::ControlChange {
            channel,
            controller,
            value,
        }
    }

    #[test]
    fn parses_channel_messages() {
        let mut p = MidiInputParser::new();
        assert_eq!(p.feed(&[0xB3, 0x11, 0x40]), vec![cc(3, 0x11, 0x40)]);
        assert_eq!(
            p.feed(&[0xAF, 0x05, 0x7F]),
            vec![MidiEvent::PolyPressure {
                channel: 15,
                note: 5,
                pressure: 0x7F
            }]
        );
        assert_eq!(
            p.feed(&[0xC0, 0x2A]),
            vec![MidiEvent::ProgramChange {
                channel: 0,
                program: 0x2A
            }]
        );
    }

    #[test]
    fn ignores_other_channel_messages() {
        let mut p = MidiInputParser::new();
        assert!(p
            .feed(&[0x90, 60, 100, 0x80, 60, 0, 0xD0, 5, 0xE0, 0, 64])
            .is_empty());
        // Running status of an ignored message must not leak into the next one.
        assert_eq!(p.feed(&[0xB0, 1, 2]), vec![cc(0, 1, 2)]);
    }

    #[test]
    fn handles_running_status_across_chunks() {
        let mut p = MidiInputParser::new();
        assert_eq!(p.feed(&[0xB1, 7, 100, 10]), vec![cc(1, 7, 100)]);
        assert_eq!(p.feed(&[64, 11]), vec![cc(1, 10, 64)]);
        assert_eq!(p.feed(&[0xF8, 127]), vec![cc(1, 11, 127)]);
        assert_eq!(
            p.feed(&[0xC2, 1, 2]),
            vec![
                MidiEvent::ProgramChange {
                    channel: 2,
                    program: 1
                },
                MidiEvent::ProgramChange {
                    channel: 2,
                    program: 2
                },
            ]
        );
    }

    #[test]
    fn parses_multiple_messages_in_one_chunk() {
        let mut p = MidiInputParser::new();
        let events = p.feed(&[0xF0, 0x01, 0xF7, 0xB0, 1, 2, 0xF0, 0x03, 0x04, 0xF7]);
        assert_eq!(
            events,
            vec![
                MidiEvent::SysEx(vec![0xF0, 0x01, 0xF7]),
                cc(0, 1, 2),
                MidiEvent::SysEx(vec![0xF0, 0x03, 0x04, 0xF7]),
            ]
        );
    }

    #[test]
    fn assembles_split_sysex() {
        let mut p = MidiInputParser::new();
        assert!(p.feed(&[0xF0, 0x00, 0x20]).is_empty());
        assert!(p.feed(&[0x33, 0x01]).is_empty());
        assert_eq!(
            p.feed(&[0x10, 0xF7]),
            vec![MidiEvent::SysEx(vec![
                0xF0, 0x00, 0x20, 0x33, 0x01, 0x10, 0xF7
            ])]
        );
    }

    #[test]
    fn real_time_inside_sysex_is_dropped() {
        let mut p = MidiInputParser::new();
        let events = p.feed(&[0xF0, 0x00, 0xF8, 0x20, 0xFE, 0x33, 0xFF, 0xF7]);
        assert_eq!(
            events,
            vec![MidiEvent::SysEx(vec![0xF0, 0x00, 0x20, 0x33, 0xF7])]
        );
    }

    #[test]
    fn status_byte_aborts_sysex_and_is_processed() {
        let mut p = MidiInputParser::new();
        assert_eq!(p.feed(&[0xF0, 0x00, 0x20, 0xB0, 5, 6]), vec![cc(0, 5, 6)]);
        // The stray F7 after the aborted frame is not a SysEx.
        assert!(p.feed(&[0xF7]).is_empty());

        assert!(p.feed(&[0xF0, 0x01, 0xF2, 0x10, 0x20, 0xF7]).is_empty());

        // A new F0 also drops the unterminated frame.
        assert_eq!(
            p.feed(&[0xF0, 0x01, 0xF0, 0x02, 0xF7]),
            vec![MidiEvent::SysEx(vec![0xF0, 0x02, 0xF7])]
        );
    }

    #[test]
    fn runaway_sysex_is_discarded() {
        let mut p = MidiInputParser::new();
        p.feed(&[0xF0]);
        assert!(p.feed(&vec![0x11; MAX_SYSEX_LEN]).is_empty());
        assert!(p.feed(&[0xF7]).is_empty());

        // Largest accepted frame is exactly MAX_SYSEX_LEN bytes.
        let mut frame = vec![0xF0];
        frame.extend(std::iter::repeat_n(0x22, MAX_SYSEX_LEN - 2));
        frame.push(0xF7);
        assert_eq!(p.feed(&frame), vec![MidiEvent::SysEx(frame)]);
    }
}

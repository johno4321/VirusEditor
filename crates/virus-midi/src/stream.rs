//! Robust System Exclusive stream assembler.
//!
//! Handles fragmented incoming MIDI packets, interleaved real-time messages (0xF8..=0xFF),
//! and framing boundaries (0xF0..=0xF7).

use virus_syx::sysex::{MIDI_SYSEX_END, MIDI_SYSEX_START};

/// Stateful assembler that reconstructs complete SysEx frames from fragmented MIDI byte chunks.
#[derive(Debug, Default)]
pub struct SysExStreamAssembler {
    buffer: Vec<u8>,
    in_sysex: bool,
}

impl SysExStreamAssembler {
    /// Creates a new empty `SysExStreamAssembler`.
    pub fn new() -> Self {
        Self {
            buffer: Vec::with_capacity(512),
            in_sysex: false,
        }
    }

    /// Feeds incoming raw MIDI bytes and returns any completed SysEx frames.
    ///
    /// Per MIDI 1.0 specification, System Real-Time messages (`0xF8..=0xFF`, e.g., Clock or
    /// Active Sensing) can arrive at any byte offset inside a SysEx message without corrupting
    /// or terminating it. They are filtered out here during frame assembly.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<Vec<u8>> {
        let mut completed_frames = Vec::new();

        for &byte in chunk {
            // Filter out system real-time messages (Clock = 0xF8, ActiveSense = 0xFE, etc.)
            if byte >= 0xF8 {
                continue;
            }

            if byte == MIDI_SYSEX_START {
                self.buffer.clear();
                self.buffer.push(byte);
                self.in_sysex = true;
            } else if byte == MIDI_SYSEX_END {
                if self.in_sysex {
                    self.buffer.push(byte);
                    completed_frames.push(std::mem::take(&mut self.buffer));
                    self.in_sysex = false;
                }
            } else if self.in_sysex {
                // Safety limit: Access Virus max dumps are typically <= 1KB.
                // Discard runaway streams exceeding 64KB to prevent unbounded memory growth.
                if self.buffer.len() < 65536 {
                    self.buffer.push(byte);
                } else {
                    self.buffer.clear();
                    self.in_sysex = false;
                }
            }
        }

        completed_frames
    }

    /// Resets the internal assembly buffer.
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.in_sysex = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_packet_assembly() {
        let mut asm = SysExStreamAssembler::new();
        let packet = [0xF0, 0x00, 0x20, 0x33, 0x01, 0x10, 0xF7];
        let frames = asm.feed(&packet);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0], packet);
    }

    #[test]
    fn test_fragmented_packet_assembly() {
        let mut asm = SysExStreamAssembler::new();
        let part1 = [0xF0, 0x00, 0x20];
        let part2 = [0x33, 0x01, 0x10];
        let part3 = [0x70, 0x40, 0x28, 0x5F, 0xF7];

        assert!(asm.feed(&part1).is_empty());
        assert!(asm.feed(&part2).is_empty());
        let frames = asm.feed(&part3);
        assert_eq!(frames.len(), 1);
        assert_eq!(
            frames[0],
            [0xF0, 0x00, 0x20, 0x33, 0x01, 0x10, 0x70, 0x40, 0x28, 0x5F, 0xF7]
        );
    }

    #[test]
    fn test_interleaved_midi_clock_and_active_sense() {
        let mut asm = SysExStreamAssembler::new();
        // 0xF8 (Clock) and 0xFE (ActiveSense) interleaved inside SysEx
        let packet = [0xF0, 0x00, 0xF8, 0x20, 0x33, 0xFE, 0x01, 0xF8, 0xF7];
        let frames = asm.feed(&packet);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0], [0xF0, 0x00, 0x20, 0x33, 0x01, 0xF7]);
    }

    #[test]
    fn test_multiple_messages_in_one_chunk() {
        let mut asm = SysExStreamAssembler::new();
        let chunk = [0xF0, 0x01, 0x02, 0xF7, 0xF0, 0x03, 0x04, 0xF7];
        let frames = asm.feed(&chunk);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0], [0xF0, 0x01, 0x02, 0xF7]);
        assert_eq!(frames[1], [0xF0, 0x03, 0x04, 0xF7]);
    }
}

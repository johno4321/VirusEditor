//! Extracting SysEx frames from `.syx` byte streams and Standard MIDI Files (`.mid`).

use std::path::Path;

const SYSEX_START: u8 = 0xF0;
const SYSEX_END: u8 = 0xF7;

/// Returns every complete `F0 … F7` frame in `bytes`.
///
/// Files starting with `MThd` are parsed as Standard MIDI Files: SysEx events (`F0 <len> data`)
/// and continuation packets (`F7 <len> data`) are reassembled across events. Anything else is
/// treated as a raw byte stream: bytes outside frames are ignored, and a frame interrupted by a
/// new `F0` or another non-real-time status byte is dropped.
pub fn extract_sysex_frames(bytes: &[u8]) -> Vec<Vec<u8>> {
    if bytes.starts_with(b"MThd") {
        extract_from_smf(bytes)
    } else {
        extract_from_stream(bytes)
    }
}

/// Reads a `.syx` or `.mid` file and returns its SysEx frames.
pub fn read_sysex_file(path: impl AsRef<Path>) -> std::io::Result<Vec<Vec<u8>>> {
    Ok(extract_sysex_frames(&std::fs::read(path)?))
}

#[derive(Default)]
struct StreamAssembler {
    current: Option<Vec<u8>>,
    frames: Vec<Vec<u8>>,
}

impl StreamAssembler {
    fn feed(&mut self, data: &[u8]) {
        for &b in data {
            match b {
                SYSEX_START => self.current = Some(vec![SYSEX_START]),
                SYSEX_END => {
                    if let Some(mut frame) = self.current.take() {
                        frame.push(SYSEX_END);
                        self.frames.push(frame);
                    }
                }
                // Real-time messages may be interleaved with SysEx; skip them.
                0xF8..=0xFF => {}
                0x80..=0xF6 => self.current = None,
                _ => {
                    if let Some(frame) = &mut self.current {
                        frame.push(b);
                    }
                }
            }
        }
    }
}

fn extract_from_stream(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut assembler = StreamAssembler::default();
    assembler.feed(bytes);
    assembler.frames
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn u8(&mut self) -> Option<u8> {
        let b = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(len)?;
        let slice = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }

    fn u32_be(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Data of an `F0`/continuation event. Some writers (including the one that produced the
    /// factory soundset) declare a length longer than the data, so the event ends at its first
    /// `F7`; SysEx data bytes are always below 0x80, so this is unambiguous.
    fn sysex_data(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.pos.saturating_add(len).min(self.data.len());
        let available = &self.data[self.pos..end];
        match available.iter().position(|&b| b == SYSEX_END) {
            Some(i) => self.take(i + 1),
            None => self.take(len),
        }
    }

    /// SMF variable-length quantity (at most 4 bytes).
    fn varlen(&mut self) -> Option<usize> {
        let mut value = 0usize;
        for _ in 0..4 {
            let b = self.u8()?;
            value = (value << 7) | (b & 0x7F) as usize;
            if b & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }
}

/// Parses the SMF chunk structure. If any track doesn't parse cleanly (e.g. lengths corrupted
/// by a text-mode transfer), falls back to scanning the whole file for `F0 … F7` frames.
fn extract_from_smf(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut reader = Reader { data: bytes, pos: 0 };
    let mut frames = Vec::new();
    let mut clean = true;
    while let (Some(id), Some(len)) = (reader.take(4), reader.u32_be()) {
        let len = len as usize;
        let available = bytes.len() - reader.pos;
        clean &= len <= available;
        let chunk = reader.take(len.min(available)).unwrap_or_default();
        if id == b"MTrk" {
            clean &= parse_track(chunk, &mut frames);
        }
    }
    if clean {
        frames
    } else {
        scan_smf_sysex_events(bytes)
    }
}

/// Fallback for malformed SMFs: finds `F0 <varlen> data… F7` events by scanning, ignoring the
/// declared length. Continuation packets are not reassembled in this mode.
fn scan_smf_sysex_events(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == SYSEX_START {
            let mut reader = Reader { data: bytes, pos: i + 1 };
            if reader.varlen().is_some() {
                let start = reader.pos;
                if let Some(len) = bytes[start..].iter().position(|&b| b >= 0x80) {
                    if bytes[start + len] == SYSEX_END {
                        let mut frame = vec![SYSEX_START];
                        frame.extend_from_slice(&bytes[start..=start + len]);
                        frames.push(frame);
                        i = start + len + 1;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    frames
}

/// Parses one `MTrk` chunk, appending complete SysEx frames. Returns false if the track is
/// malformed (it stops at the first bad event).
fn parse_track(track: &[u8], frames: &mut Vec<Vec<u8>>) -> bool {
    let mut reader = Reader { data: track, pos: 0 };
    let mut running_status: Option<u8> = None;
    let mut pending: Option<Vec<u8>> = None;
    // Raw bytes inside F7 "escape" events may themselves contain complete SysEx frames.
    let mut escaped = StreamAssembler::default();

    let mut parse = || -> Option<()> {
        loop {
            reader.varlen()?; // delta time
            let first = reader.u8()?;
            match first {
                0xFF => {
                    running_status = None;
                    let meta_type = reader.u8()?;
                    let len = reader.varlen()?;
                    reader.take(len)?;
                    if meta_type == 0x2F {
                        return Some(());
                    }
                }
                SYSEX_START => {
                    running_status = None;
                    let len = reader.varlen()?;
                    let data = reader.sysex_data(len)?;
                    let mut frame = vec![SYSEX_START];
                    frame.extend_from_slice(data);
                    pending = complete_or_pending(frame, frames);
                }
                SYSEX_END => {
                    running_status = None;
                    let len = reader.varlen()?;
                    match pending.take() {
                        Some(mut frame) => {
                            frame.extend_from_slice(reader.sysex_data(len)?);
                            pending = complete_or_pending(frame, frames);
                        }
                        None => {
                            escaped.feed(reader.take(len)?);
                            frames.append(&mut escaped.frames);
                        }
                    }
                }
                0x80..=0xEF => {
                    running_status = Some(first);
                    reader.take(channel_data_len(first))?;
                }
                0xF1..=0xFE => return None,
                _ => {
                    // Running status: `first` is the first data byte.
                    let status = running_status?;
                    reader.take(channel_data_len(status) - 1)?;
                }
            }
        }
    };
    let ended_with_meta = parse().is_some();
    ended_with_meta || (reader.pos == track.len() && pending.is_none())
}

fn complete_or_pending(frame: Vec<u8>, frames: &mut Vec<Vec<u8>>) -> Option<Vec<u8>> {
    if frame.last() == Some(&SYSEX_END) && frame.len() > 1 {
        frames.push(frame);
        None
    } else {
        Some(frame)
    }
}

fn channel_data_len(status: u8) -> usize {
    match status & 0xF0 {
        0xC0 | 0xD0 => 1,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn smf(track: &[u8]) -> Vec<u8> {
        let mut file = b"MThd".to_vec();
        file.extend_from_slice(&6u32.to_be_bytes());
        file.extend_from_slice(&[0, 0, 0, 1, 0, 96]);
        file.extend_from_slice(b"MTrk");
        file.extend_from_slice(&(track.len() as u32).to_be_bytes());
        file.extend_from_slice(track);
        file
    }

    #[test]
    fn smf_continuation_packets_are_reassembled() {
        let track = [
            0x00, 0xFF, 0x03, 0x02, b'h', b'i', // track name
            0x00, 0x90, 0x3C, 0x40, // note on
            0x10, 0x3C, 0x00, // running status note "off"
            0x00, 0xF0, 0x03, 0x00, 0x20, 0x33, // first packet, no F7
            0x81, 0x00, 0xF7, 0x03, 0x01, 0x10, 0x12, // continuation (2-byte delta)
            0x00, 0xF7, 0x02, 0x05, 0xF7, // final packet
            0x00, 0xC0, 0x05, // program change (1 data byte)
            0x00, 0xF0, 0x02, 0x7E, 0xF7, // complete single-event sysex
            0x00, 0xFF, 0x2F, 0x00,
        ];
        let frames = extract_sysex_frames(&smf(&track));
        assert_eq!(
            frames,
            vec![
                vec![0xF0, 0x00, 0x20, 0x33, 0x01, 0x10, 0x12, 0x05, 0xF7],
                vec![0xF0, 0x7E, 0xF7],
            ]
        );
    }

    #[test]
    fn smf_sysex_with_overlong_declared_length_ends_at_f7() {
        // As in the factory .mid files: the declared length exceeds the data actually present.
        let track = [0x00, 0xF0, 0x05, 0x7D, 0xF7, 0x00, 0xF0, 0x02, 0x7E, 0xF7, 0x00, 0xFF, 0x2F, 0x00];
        assert_eq!(extract_sysex_frames(&smf(&track)), vec![vec![0xF0, 0x7D, 0xF7], vec![0xF0, 0x7E, 0xF7]]);
    }

    #[test]
    fn malformed_smf_falls_back_to_event_scanning() {
        // 0xF4 is not a valid SMF event; the track structure can't be parsed past it.
        let track = [0x00, 0xF4, 0x00, 0xF0, 0x82, 0x01, 0x10, 0x11, 0xF7, 0x00, 0xFF, 0x2F, 0x00];
        assert_eq!(extract_sysex_frames(&smf(&track)), vec![vec![0xF0, 0x10, 0x11, 0xF7]]);
    }

    #[test]
    fn raw_stream_drops_interrupted_frames_and_noise() {
        let stream = [
            0x12, 0xF0, 0x01, 0x02, // interrupted by a new F0
            0xF0, 0x03, 0xF8, 0x04, 0xF7, // real-time byte skipped
            0x55, 0xF7, // stray bytes ignored
            0xF0, 0x05, 0x90, 0xF7, // aborted by a channel status byte
        ];
        assert_eq!(extract_sysex_frames(&stream), vec![vec![0xF0, 0x03, 0x04, 0xF7]]);
    }
}

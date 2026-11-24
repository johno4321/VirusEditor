//! The boundary to the synth: a [`Transport`] that sends bytes and yields [`MidiEvent`]s, and
//! [`Collector`] state machines that turn incoming events into answers. Collectors are fed
//! plain events, so they (and everything built on [`exchange`]) are tested without hardware.

use std::fmt;
use std::time::{Duration, Instant};

use virus_midi::MidiEvent;
use virus_syx::{parse_sysex, SysExMessage};

use crate::settings::{bank_label, DeviceId};

/// Sends MIDI and receives events. Implemented over `virus_midi::MidiService` for real ports
/// and by an in-memory fake Virus in tests.
pub trait Transport {
    /// Sends one complete message now.
    fn send(&mut self, message: &[u8]) -> Result<(), String>;

    /// Sends messages one by one with `gap` after each, calling `progress(sent, total)`.
    fn send_paced(
        &mut self,
        messages: &[Vec<u8>],
        gap: Duration,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<(), String>;

    /// The next incoming event, or `None` if nothing arrived within `timeout`.
    fn recv(&mut self, timeout: Duration) -> Option<MidiEvent>;

    /// Drops events that are already waiting (stale answers); returns how many.
    fn discard_pending(&mut self) -> usize;
}

/// What one event meant to a collector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feed {
    /// Not relevant; does not extend the idle timeout.
    Ignored,
    /// Relevant but not finished; restarts the idle timeout.
    Progress,
    /// The collector has everything it needs.
    Done,
}

/// A state machine waiting for an answer from the synth.
pub trait Collector {
    fn feed(&mut self, event: &MidiEvent) -> Feed;
    /// Human-readable description of what is still missing (for timeouts).
    fn missing(&self) -> String;
    /// Relevant-looking messages that were ignored (wrong part, device, bank…), for diagnostics.
    fn notes(&self) -> &[String];
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectError {
    /// Nothing relevant arrived for `idle` (the synth may be on another device ID, the cable
    /// may be wrong, or Dump Tx may be off).
    Timeout { idle: Duration, missing: String, notes: Vec<String> },
    /// The MIDI layer reported an error.
    Midi(String),
}

impl fmt::Display for CollectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout { idle, missing, notes } => {
                write!(f, "timed out after {} ms without an answer: {missing}", idle.as_millis())?;
                for note in notes.iter().take(10) {
                    write!(f, "\n  note: {note}")?;
                }
                if notes.len() > 10 {
                    write!(f, "\n  note: … {} more", notes.len() - 10)?;
                }
                Ok(())
            }
            Self::Midi(e) => write!(f, "MIDI error: {e}"),
        }
    }
}

impl std::error::Error for CollectError {}

/// Waits until no event has arrived for `quiet`, dropping whatever does arrive (stale answers,
/// the rest of an interrupted bank stream). Returns how many events were dropped.
///
/// The Virus Rack ignores a request that arrives shortly after it has sent or applied
/// something (measured 24 Nov: a bank request right after an edit-buffer answer gets no reply,
/// 2 s later it does), so every request waits for a quiet line first.
pub fn wait_quiet<T: Transport + ?Sized>(transport: &mut T, quiet: Duration) -> usize {
    let mut dropped = transport.discard_pending();
    if quiet.is_zero() {
        return dropped;
    }
    while transport.recv(quiet).is_some() {
        dropped += 1;
    }
    dropped
}

/// Waits for a quiet line (see [`wait_quiet`]), sends `request` (if any) and feeds incoming
/// events to `collector` until it is done. Times out when nothing relevant arrives for `idle`;
/// `progress` is called after every relevant event.
pub fn exchange<T: Transport + ?Sized, C: Collector>(
    transport: &mut T,
    request: Option<&[u8]>,
    collector: &mut C,
    quiet: Duration,
    idle: Duration,
    mut progress: impl FnMut(&C),
) -> Result<(), CollectError> {
    wait_quiet(transport, quiet);
    if let Some(request) = request {
        transport.send(request).map_err(CollectError::Midi)?;
    }
    let mut deadline = Instant::now() + idle;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let Some(event) = transport.recv(remaining) else {
            return Err(CollectError::Timeout {
                idle,
                missing: collector.missing(),
                notes: collector.notes().to_vec(),
            });
        };
        if let MidiEvent::Error(e) = event {
            return Err(CollectError::Midi(e));
        }
        match collector.feed(&event) {
            Feed::Ignored => {}
            Feed::Progress => {
                progress(collector);
                deadline = Instant::now() + idle;
            }
            Feed::Done => {
                progress(collector);
                return Ok(());
            }
        }
    }
}

/// A single dump as received, with the exact frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedDump {
    pub frame: Vec<u8>,
    pub device_id: u8,
    pub bank: u8,
    pub program: u8,
    pub data: [u8; 256],
    pub checksum_ok: bool,
}

/// Parses a Virus single dump frame; `None` for anything else.
fn single_dump(frame: &[u8]) -> Option<ReceivedDump> {
    match parse_sysex(frame).ok()? {
        SysExMessage::SingleDump { device_id, bank, program, data, checksum_ok } => Some(ReceivedDump {
            frame: frame.to_vec(),
            device_id,
            bank,
            program,
            data: *data,
            checksum_ok,
        }),
        _ => None,
    }
}

/// Waits for the single dump of one edit buffer (`10 00 pp`), the answer to `30 00 pp`.
#[derive(Debug)]
pub struct EditBufferCollector {
    device: DeviceId,
    part: u8,
    result: Option<ReceivedDump>,
    notes: Vec<String>,
}

impl EditBufferCollector {
    pub fn new(device: DeviceId, part: u8) -> Self {
        Self { device, part, result: None, notes: Vec::new() }
    }

    pub fn result(&self) -> Option<&ReceivedDump> {
        self.result.as_ref()
    }

    pub fn into_result(self) -> Option<ReceivedDump> {
        self.result
    }
}

impl Collector for EditBufferCollector {
    fn feed(&mut self, event: &MidiEvent) -> Feed {
        let MidiEvent::SysEx(frame) = event else {
            return Feed::Ignored;
        };
        let Some(dump) = single_dump(frame) else {
            return Feed::Ignored;
        };
        if !self.device.accepts(dump.device_id) {
            self.notes.push(format!("single dump from device 0x{:02X} ignored", dump.device_id));
            return Feed::Ignored;
        }
        if dump.bank != 0 || dump.program != self.part {
            self.notes.push(format!(
                "single dump for {} {} ignored (waiting for edit buffer part 0x{:02X})",
                if dump.bank == 0 { "edit buffer part".to_owned() } else { format!("bank {}", bank_label(dump.bank)) },
                dump.program,
                self.part
            ));
            return Feed::Ignored;
        }
        self.result = Some(dump);
        Feed::Done
    }

    fn missing(&self) -> String {
        format!("single dump of edit buffer part 0x{:02X}", self.part)
    }

    fn notes(&self) -> &[String] {
        &self.notes
    }
}

/// Collects the 128 single dumps of one bank, the answer to `32 bb`.
#[derive(Debug)]
pub struct BankCollector {
    device: DeviceId,
    bank: u8,
    slots: Vec<Option<ReceivedDump>>,
    received: usize,
    duplicates: usize,
    notes: Vec<String>,
}

pub const BANK_SIZE: usize = 128;

impl BankCollector {
    pub fn new(device: DeviceId, bank: u8) -> Self {
        Self { device, bank, slots: vec![None; BANK_SIZE], received: 0, duplicates: 0, notes: Vec::new() }
    }

    /// Distinct programs received so far.
    pub fn count(&self) -> usize {
        self.received
    }

    pub fn duplicates(&self) -> usize {
        self.duplicates
    }

    pub fn bad_checksums(&self) -> Vec<u8> {
        self.slots.iter().flatten().filter(|d| !d.checksum_ok).map(|d| d.program).collect()
    }

    pub fn missing_programs(&self) -> Vec<u8> {
        (0..BANK_SIZE as u8).filter(|&p| self.slots[p as usize].is_none()).collect()
    }

    /// Received dumps in program order.
    pub fn dumps(&self) -> Vec<&ReceivedDump> {
        self.slots.iter().flatten().collect()
    }
}

impl Collector for BankCollector {
    fn feed(&mut self, event: &MidiEvent) -> Feed {
        let MidiEvent::SysEx(frame) = event else {
            return Feed::Ignored;
        };
        let Some(dump) = single_dump(frame) else {
            return Feed::Ignored;
        };
        if !self.device.accepts(dump.device_id) {
            self.notes.push(format!("single dump from device 0x{:02X} ignored", dump.device_id));
            return Feed::Ignored;
        }
        if dump.bank != self.bank || dump.program as usize >= BANK_SIZE {
            self.notes.push(format!(
                "single dump bb=0x{:02X} ss={} ignored (waiting for bank {})",
                dump.bank,
                dump.program,
                bank_label(self.bank)
            ));
            return Feed::Ignored;
        }
        let slot = &mut self.slots[dump.program as usize];
        if slot.is_some() {
            self.duplicates += 1;
        } else {
            self.received += 1;
        }
        *slot = Some(dump);
        if self.received == BANK_SIZE {
            Feed::Done
        } else {
            Feed::Progress
        }
    }

    fn missing(&self) -> String {
        let missing = self.missing_programs();
        let shown: Vec<String> = missing.iter().take(12).map(u8::to_string).collect();
        format!(
            "bank {}: {} of {BANK_SIZE} received, missing programs {}{}",
            bank_label(self.bank),
            self.received,
            shown.join(", "),
            if missing.len() > 12 { ", …" } else { "" }
        )
    }

    fn notes(&self) -> &[String] {
        &self.notes
    }
}

/// Waits for the first Virus SysEx message from the device (any command except requests),
/// e.g. the undocumented answer to a Global request.
#[derive(Debug)]
pub struct FirstVirusMessage {
    device: DeviceId,
    result: Option<Vec<u8>>,
    notes: Vec<String>,
}

impl FirstVirusMessage {
    pub fn new(device: DeviceId) -> Self {
        Self { device, result: None, notes: Vec::new() }
    }

    pub fn into_result(self) -> Option<Vec<u8>> {
        self.result
    }
}

impl Collector for FirstVirusMessage {
    fn feed(&mut self, event: &MidiEvent) -> Feed {
        let MidiEvent::SysEx(frame) = event else {
            return Feed::Ignored;
        };
        let Ok(message) = parse_sysex(frame) else {
            return Feed::Ignored;
        };
        if !self.device.accepts(message.device_id()) {
            self.notes.push(format!("message from device 0x{:02X} ignored", message.device_id()));
            return Feed::Ignored;
        }
        let is_request = frame.get(6).is_some_and(|c| (0x30..=0x37).contains(c));
        if is_request {
            return Feed::Ignored;
        }
        self.result = Some(frame.clone());
        Feed::Done
    }

    fn missing(&self) -> String {
        "any Virus SysEx answer".to_owned()
    }

    fn notes(&self) -> &[String] {
        &self.notes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use virus_syx::encode_program_dump;

    fn dump_event(device: u8, bank: u8, program: u8, fill: u8) -> MidiEvent {
        MidiEvent::SysEx(encode_program_dump(device, bank, program, &[fill; 256]))
    }

    #[test]
    fn edit_buffer_collector_waits_for_its_part_and_device() {
        let mut c = EditBufferCollector::new(DeviceId(0), 0x40);
        assert_eq!(c.feed(&MidiEvent::ControlChange { channel: 0, controller: 1, value: 2 }), Feed::Ignored);
        assert_eq!(c.feed(&dump_event(0, 0, 0x00, 1)), Feed::Ignored);
        assert_eq!(c.feed(&dump_event(3, 0, 0x40, 1)), Feed::Ignored);
        assert_eq!(c.feed(&dump_event(0, 1, 0x40, 1)), Feed::Ignored);
        assert_eq!(c.notes().len(), 3);
        assert_eq!(c.feed(&dump_event(0x10, 0, 0x40, 7)), Feed::Done);
        let result = c.into_result().unwrap();
        assert_eq!(result.data, [7; 256]);
        assert!(result.checksum_ok);
        assert_eq!(result.frame.len(), 267);
    }

    #[test]
    fn bank_collector_counts_distinct_programs_and_flags_checksums() {
        let mut c = BankCollector::new(DeviceId(0), 1);
        assert_eq!(c.feed(&dump_event(0, 2, 0, 0)), Feed::Ignored);
        for p in 0..127u8 {
            assert_eq!(c.feed(&dump_event(0, 1, p, p)), Feed::Progress);
        }
        assert_eq!(c.feed(&dump_event(0, 1, 5, 5)), Feed::Progress);
        assert_eq!(c.duplicates(), 1);
        assert_eq!(c.missing_programs(), vec![127]);
        assert!(c.missing().contains("127 of 128"));
        let MidiEvent::SysEx(mut bad) = dump_event(0, 1, 127, 9) else { unreachable!() };
        bad[265] ^= 1;
        assert_eq!(c.feed(&MidiEvent::SysEx(bad)), Feed::Done);
        assert_eq!(c.count(), 128);
        assert_eq!(c.bad_checksums(), vec![127]);
        let dumps = c.dumps();
        assert!(dumps.iter().enumerate().all(|(i, d)| d.program as usize == i));
    }

    /// A transport that replays a fixed event list.
    struct Replay(std::collections::VecDeque<MidiEvent>, Vec<Vec<u8>>);

    impl Transport for Replay {
        fn send(&mut self, message: &[u8]) -> Result<(), String> {
            self.1.push(message.to_vec());
            Ok(())
        }
        fn send_paced(&mut self, _: &[Vec<u8>], _: Duration, _: &mut dyn FnMut(usize, usize)) -> Result<(), String> {
            Ok(())
        }
        fn recv(&mut self, _: Duration) -> Option<MidiEvent> {
            self.0.pop_front()
        }
        fn discard_pending(&mut self) -> usize {
            0
        }
    }

    #[test]
    fn exchange_times_out_with_notes_and_reports_midi_errors() {
        let mut t = Replay(vec![dump_event(0, 0, 0x03, 0)].into(), Vec::new());
        let mut c = EditBufferCollector::new(DeviceId(0), 0x40);
        let err = exchange(&mut t, Some(&[0xF0, 0xF7]), &mut c, Duration::ZERO, Duration::from_millis(10), |_| {})
            .unwrap_err();
        assert_eq!(t.1, vec![vec![0xF0, 0xF7]]);
        match &err {
            CollectError::Timeout { notes, .. } => assert_eq!(notes.len(), 1),
            other => panic!("unexpected {other:?}"),
        }
        assert!(err.to_string().contains("edit buffer part 0x40"));

        let mut t = Replay(vec![MidiEvent::Error("boom".into())].into(), Vec::new());
        let mut c = EditBufferCollector::new(DeviceId(0), 0x40);
        assert_eq!(
            exchange(&mut t, None, &mut c, Duration::ZERO, Duration::from_millis(10), |_| {}),
            Err(CollectError::Midi("boom".into()))
        );
    }

    #[test]
    fn a_stale_stream_is_drained_before_the_request_is_sent() {
        // The rest of an interrupted bank A stream, still arriving when the next command starts.
        let stale: Vec<MidiEvent> = (68..72).map(|p| dump_event(0, 1, p, 0)).collect();
        let mut t = Replay(stale.into(), Vec::new());
        assert_eq!(wait_quiet(&mut t, Duration::from_millis(5)), 4);
        assert!(t.1.is_empty(), "nothing may be sent while the line is busy");

        let mut t = Replay(vec![dump_event(0, 1, 68, 0), dump_event(0, 0, 0x40, 0)].into(), Vec::new());
        let mut c = EditBufferCollector::new(DeviceId(0), 0x40);
        // The edit-buffer answer arrives inside the quiet window, so it counts as stale too and the
        // collector waits for a fresh answer.
        let err = exchange(&mut t, Some(&[0xF0, 0xF7]), &mut c, Duration::from_millis(5), Duration::from_millis(10), |_| {});
        assert!(matches!(err, Err(CollectError::Timeout { .. })));
        assert_eq!(t.1, vec![vec![0xF0, 0xF7]]);
    }
}

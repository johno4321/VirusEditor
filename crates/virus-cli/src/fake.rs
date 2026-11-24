//! An in-memory Access Virus for tests: it answers requests, applies parameter changes and
//! stores dumps like the real synth (as far as SPEC §3 documents it).

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use virus_midi::MidiEvent;
use virus_syx::{encode_program_dump, parse_sysex, Page, Patch, SysExMessage, DEVICE_ID_OMNI};

use crate::ops::Synth;
use crate::settings::Target;
use crate::transport::Transport;

/// A [`Synth`] over a fake with the default target, a short timeout and no settle pause.
pub fn synth(fake: FakeVirus) -> Synth<FakeVirus> {
    Synth::new(fake, Target::default(), Duration::from_millis(20), Duration::ZERO)
}

pub struct FakeVirus {
    pub device: u8,
    pub edit_buffers: HashMap<u8, [u8; 256]>,
    pub banks: HashMap<u8, Vec<[u8; 256]>>,
    /// Parameter changes above this value are clamped (simulates a range check).
    pub max_values: HashMap<(Page, u8), u8>,
    /// Every message the host sent, in order.
    pub received: Vec<Vec<u8>>,
    /// Answer nothing at all.
    pub silent: bool,
    /// Ignore every single dump sent to it (like a synth that rejects or never receives them).
    pub ignore_dumps: bool,
    /// Someone keeps turning a knob: whenever nothing else is queued, another CC arrives, so
    /// the MIDI input never goes quiet.
    pub chatter: bool,
    queue: VecDeque<MidiEvent>,
}

/// Distinct, recognisable patch data: byte `i` = `(i * 3 + seed) % 128`, OS 2.0 sound version.
pub fn pattern(seed: u8) -> [u8; 256] {
    let mut bytes = [0u8; 256];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = ((i * 3 + seed as usize) % 128) as u8;
    }
    bytes[0] = 6;
    bytes
}

impl FakeVirus {
    pub fn new(device: u8) -> Self {
        let banks = (1..=4u8).map(|b| (b, (0..128u8).map(|p| pattern(b * 31 + p)).collect())).collect();
        let mut edit_buffers = HashMap::new();
        edit_buffers.insert(0x40, *Patch::init().as_bytes());
        for part in 0..16u8 {
            edit_buffers.insert(part, pattern(part));
        }
        Self {
            device,
            edit_buffers,
            banks,
            max_values: HashMap::new(),
            received: Vec::new(),
            silent: false,
            ignore_dumps: false,
            chatter: false,
            queue: VecDeque::new(),
        }
    }

    fn reply(&mut self, frame: Vec<u8>) {
        if !self.silent {
            self.queue.push_back(MidiEvent::SysEx(frame));
        }
    }

    /// Queues an event as if it were still arriving from an earlier exchange.
    pub fn push_stale(&mut self, event: MidiEvent) {
        self.queue.push_back(event);
    }

    fn handle(&mut self, message: &[u8]) {
        let Ok(parsed) = parse_sysex(message) else {
            return;
        };
        let dd = parsed.device_id();
        if dd != self.device && dd != DEVICE_ID_OMNI {
            return;
        }
        let device = self.device;
        match parsed {
            SysExMessage::SingleRequest { bank: 0, program: part, .. } => {
                if let Some(data) = self.edit_buffers.get(&part).copied() {
                    self.reply(encode_program_dump(device, 0, part, &data));
                }
            }
            SysExMessage::SingleRequest { bank, program, .. } => {
                if let Some(data) = self.banks.get(&bank).map(|b| b[program as usize & 0x7F]) {
                    self.reply(encode_program_dump(device, bank, program, &data));
                }
            }
            SysExMessage::SingleBankRequest { bank, .. } => {
                let dumps: Vec<Vec<u8>> = self
                    .banks
                    .get(&bank)
                    .map(|b| b.iter().enumerate().map(|(p, d)| encode_program_dump(device, bank, p as u8, d)).collect())
                    .unwrap_or_default();
                for dump in dumps {
                    self.reply(dump);
                }
            }
            SysExMessage::SingleDump { .. } if self.ignore_dumps => {}
            SysExMessage::SingleDump { bank: 0, program: part, data, .. } => {
                self.edit_buffers.insert(part, *data);
            }
            SysExMessage::SingleDump { bank, program, data, .. } => {
                if let Some(slots) = self.banks.get_mut(&bank) {
                    slots[program as usize & 0x7F] = *data;
                }
            }
            SysExMessage::ParamChange { page, part, param, value, .. } => {
                let Some(base) = page.dump_offset() else { return };
                let value = self.max_values.get(&(page, param)).map_or(value, |&max| value.min(max));
                if let Some(buffer) = self.edit_buffers.get_mut(&part) {
                    buffer[base + param as usize] = value;
                }
            }
            SysExMessage::GlobalRequest { .. } => {
                self.reply(vec![0xF0, 0x00, 0x20, 0x33, 0x01, device, 0x12, 0x00, 0x01, 0xF7]);
            }
            _ => {}
        }
    }
}

impl Transport for FakeVirus {
    fn send(&mut self, message: &[u8]) -> Result<(), String> {
        self.received.push(message.to_vec());
        self.handle(message);
        Ok(())
    }

    fn send_paced(
        &mut self,
        messages: &[Vec<u8>],
        _gap: Duration,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<(), String> {
        for (i, message) in messages.iter().enumerate() {
            self.send(message)?;
            progress(i + 1, messages.len());
        }
        Ok(())
    }

    fn recv(&mut self, _timeout: Duration) -> Option<MidiEvent> {
        match self.queue.pop_front() {
            None if self.chatter => Some(MidiEvent::ControlChange { channel: 0, controller: 40, value: 64 }),
            event => event,
        }
    }

    fn discard_pending(&mut self) -> usize {
        let n = self.queue.len();
        self.queue.clear();
        n
    }
}

//! The 256-byte Access Virus Multi program (Rack addendum OS 2.0, Multi Dump Table pp. 25–27,
//! Page C p. 22; SPEC §3.3, §4.3, §4.4).
//!
//! The raw bytes are the single source of truth (SPEC §5.1). The internal bytes 0–3, 14,
//! 25–31, 208–239 and bit 4 (and the undocumented bits 3 and 7) of each part-state byte are
//! never touched by any accessor.

use crate::params::{format_kind, ValueKind, LFO_SHAPE_NAMES};
use crate::sysex::{
    encode_multi_dump, encode_param_change, parse_sysex, part_bank_from_c32, part_bank_to_c32, Page, SysExMessage,
    SyxError, PAGE_C_PART_BANK_CHANGE,
};

/// Length of a multi program (the data block of a multi dump).
pub const MULTI_LEN: usize = 256;
/// Number of parts in a multi.
pub const MULTI_PARTS: usize = 16;
/// Number of programs in the Multi bank.
pub const MULTI_BANK_SLOTS: usize = 128;
/// Name location: bytes 4..13.
pub const MULTI_NAME_OFFSET: usize = 4;
pub const MULTI_NAME_LEN: usize = 10;
/// Part state bitfield of part 1..16: bytes 240..255.
pub const PART_STATE_OFFSET: usize = 240;
/// Multi dump `bb` of the Multi edit buffer and of the Multi bank.
pub const MULTI_EDIT_BUFFER_BANK: u8 = 0x00;
pub const MULTI_BANK: u8 = 0x01;

/// Where a multi dump goes / came from (`bb mm`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MultiDumpTarget {
    /// `bb = 00`: the Multi edit buffer (`mm` is ignored by the synth and sent as `00`).
    EditBuffer,
    /// `bb = 01`: program `mm` of the Multi bank.
    Bank { program: u8 },
}

impl MultiDumpTarget {
    /// Decodes `bb`/`mm`; `None` for bank bytes other than `00`/`01`.
    pub fn from_raw(bb: u8, mm: u8) -> Option<Self> {
        match bb {
            MULTI_EDIT_BUFFER_BANK => Some(Self::EditBuffer),
            MULTI_BANK => Some(Self::Bank { program: mm }),
            _ => None,
        }
    }

    /// Returns `(bb, mm)`.
    pub fn to_raw(self) -> (u8, u8) {
        match self {
            Self::EditBuffer => (MULTI_EDIT_BUFFER_BANK, 0),
            Self::Bank { program } => (MULTI_BANK, program),
        }
    }
}

/// The documented bits of a part-state byte (bytes 240..255).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PartFlag {
    /// Bit 0, C72 Part Enable.
    Enable,
    /// Bit 1, C73 Part MIDI Volume Enable.
    MidiVolume,
    /// Bit 2, C74 Part Hold Pedal Enable.
    HoldPedal,
    /// Bit 5, C77 Note Steal Priority (0 Low, 1 High).
    PriorityHigh,
    /// Bit 6, C78 Part Program Change Enable.
    ProgramChange,
}

impl PartFlag {
    pub const ALL: [Self; 5] = [Self::Enable, Self::MidiVolume, Self::HoldPedal, Self::PriorityHigh, Self::ProgramChange];

    pub const fn bit(self) -> u8 {
        match self {
            Self::Enable => 0,
            Self::MidiVolume => 1,
            Self::HoldPedal => 2,
            Self::PriorityHigh => 5,
            Self::ProgramChange => 6,
        }
    }
}

/// Where a Multi parameter lives in the 256 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MultiLocation {
    /// One byte of the multi header (bytes 0..31).
    Common(usize),
    /// A per-part array: part `p` (0..15) is byte `base + p`.
    Part(usize),
    /// One bit of part `p`'s state byte `240 + p`.
    PartFlag(PartFlag),
}

/// The parameter change that edits a Multi value live (SPEC §3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LiveParam {
    pub page: Page,
    pub number: u8,
}

/// One row of the Multi Dump table.
///
/// `min`/`max` are the **documented** range; factory data exceeds some (part bank 2 in
/// multi 127), so never clamp stored bytes to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MultiParamDef {
    pub name: &'static str,
    /// Compact label for table headers.
    pub short: &'static str,
    pub section: &'static str,
    pub location: MultiLocation,
    pub kind: ValueKind,
    pub min: u8,
    pub max: u8,
    /// Parameter change that edits the value live; `None` = only by sending the whole multi.
    pub live: Option<LiveParam>,
}

impl MultiParamDef {
    pub fn is_part_param(&self) -> bool {
        !matches!(self.location, MultiLocation::Common(_))
    }

    /// Byte offset of this parameter for `part` (0..15; ignored for common parameters).
    pub fn offset(&self, part: u8) -> usize {
        let part = (part as usize) % MULTI_PARTS;
        match self.location {
            MultiLocation::Common(offset) => offset,
            MultiLocation::Part(base) => base + part,
            MultiLocation::PartFlag(_) => PART_STATE_OFFSET + part,
        }
    }

    pub fn format(&self, raw: u8) -> String {
        format_kind(self.kind, raw)
    }

    /// The SysEx parameter change for `value` of `part` (0..15), or `None` if the parameter
    /// can't be edited live. Common parameters are sent with part byte `00` (class `np`).
    /// `value` is the stored byte; the Part Bank row converts it with [`part_bank_to_c32`].
    pub fn param_change(&self, device_id: u8, part: u8, value: u8) -> Option<Vec<u8>> {
        let live = self.live?;
        let part = if self.is_part_param() { part & 0x0F } else { 0 };
        let value = if self.is_part_bank() { part_bank_to_c32(value) } else { value };
        Some(encode_param_change(device_id, live.page, part, live.number, value))
    }

    /// The stored byte for a value received live as this row's parameter change, the inverse of
    /// [`param_change`](Self::param_change); `None` if the value has no stored counterpart (a C32
    /// value below bank A).
    pub fn stored_from_live(&self, value: u8) -> Option<u8> {
        if self.is_part_bank() {
            part_bank_from_c32(value)
        } else {
            Some(value)
        }
    }

    /// The Part Bank row: stored 0-based (A = 0), sent as C32 Part Bank Change.
    fn is_part_bank(&self) -> bool {
        self.live == Some(LiveParam { page: Page::PageC, number: PAGE_C_PART_BANK_CHANGE })
    }
}

/// Bank of a part's single: dump bytes 32..47 (documented 0..1) and C31/C32 (0..3).
pub const PART_BANK_NAMES: [&str; 4] = ["A", "B", "C", "D"];
pub const MIDI_CHANNEL_NAMES: [&str; 16] =
    ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16"];
/// Output select (C22, C41, bytes 22 and 176..191).
///
/// Only 0..2 are printed in the addendum ("0:Out1L 1:Out1L+R 2:Out1R .."); 3..8 follow that
/// pattern for the Rack's three stereo outputs (factory multi 127 uses 1, 4 and 7 for three
/// layered parts). 9..14 are unknown and shown as `#n`. Needs a live test.
pub const OUTPUT_SELECT_NAMES: [&str; 9] =
    ["Out1 L", "Out1 L+R", "Out1 R", "Out2 L", "Out2 L+R", "Out2 R", "Out3 L", "Out3 L+R", "Out3 R"];
pub const PRIORITY_NAMES: [&str; 2] = ["Low", "High"];

const fn row(
    name: &'static str,
    short: &'static str,
    section: &'static str,
    location: MultiLocation,
    kind: ValueKind,
    max: u8,
    live: Option<LiveParam>,
) -> MultiParamDef {
    MultiParamDef { name, short, section, location, kind, min: 0, max, live }
}

const fn c(number: u8) -> Option<LiveParam> {
    Some(LiveParam { page: Page::PageC, number })
}

const fn name_char(index: u8, name: &'static str) -> MultiParamDef {
    MultiParamDef {
        name,
        short: name,
        section: "Name",
        location: MultiLocation::Common(MULTI_NAME_OFFSET + index as usize),
        kind: ValueKind::NameChar,
        min: 32,
        max: 127,
        live: c(5 + index),
    }
}

use MultiLocation::{Common, Part};

/// Every documented row of the OS 2.0 Multi Dump table (SPEC §4.4) with its live parameter
/// change (SPEC §4.3): common rows by offset, then part rows in column order.
///
/// Live mappings: the name is C5–C14 and Delay Output Select C22; the part rows are C32
/// Bank Change (immediate), C33 Program Change, C34–C41 and C72–C78 (addressed to the part).
/// Effect Send has no Page C number: it is the part single's A113 (class `ms`, stored with the
/// Multi in Multi mode). Clock tempo and the other Multi Delay bytes have no documented
/// parameter change, so they are sent with the whole multi.
pub static MULTI_PARAMS: &[MultiParamDef] = &[
    name_char(0, "Multi Name Char1"),
    name_char(1, "Multi Name Char2"),
    name_char(2, "Multi Name Char3"),
    name_char(3, "Multi Name Char4"),
    name_char(4, "Multi Name Char5"),
    name_char(5, "Multi Name Char6"),
    name_char(6, "Multi Name Char7"),
    name_char(7, "Multi Name Char8"),
    name_char(8, "Multi Name Char9"),
    name_char(9, "Multi Name Char10"),
    row("Multi Clock Tempo", "Tempo", "Multi", Common(15), ValueKind::Unipolar, 127, None),
    row("Multi Delay Mode", "Delay", "Delay", Common(16), ValueKind::Toggle, 1, None),
    row("Multi Delay Time", "Time", "Delay", Common(17), ValueKind::Unipolar, 127, None),
    row("Multi Delay Feedback", "Feedback", "Delay", Common(18), ValueKind::Unipolar, 127, None),
    row("Multi Delay Rate", "Rate", "Delay", Common(19), ValueKind::Unipolar, 127, None),
    row("Multi Delay Depth", "Depth", "Delay", Common(20), ValueKind::Unipolar, 127, None),
    row("Multi Delay Shape", "Shape", "Delay", Common(21), ValueKind::Choice(&LFO_SHAPE_NAMES), 5, None),
    row("Multi Delay Output Select", "Output", "Delay", Common(22), ValueKind::Choice(&OUTPUT_SELECT_NAMES), 14, c(22)),
    row("Multi Delay Clock", "Clock", "Delay", Common(23), ValueKind::Raw, 16, None),
    row("Multi Delay Color", "Color", "Delay", Common(24), ValueKind::Bipolar, 127, None),
    row("Part Bank", "Bank", "Part", Part(32), ValueKind::Choice(&PART_BANK_NAMES), 1, c(32)),
    row("Part Program", "Prog", "Part", Part(48), ValueKind::Unipolar, 127, c(33)),
    row("Part MIDI Channel", "Ch", "Part", Part(64), ValueKind::Choice(&MIDI_CHANNEL_NAMES), 15, c(34)),
    row("Part Low Key", "Low", "Part", Part(80), ValueKind::Note, 127, c(35)),
    row("Part High Key", "High", "Part", Part(96), ValueKind::Note, 127, c(36)),
    row("Part Transpose", "Transp", "Part", Part(112), ValueKind::Bipolar, 127, c(37)),
    row("Part Detune", "Detune", "Part", Part(128), ValueKind::Bipolar, 127, c(38)),
    row("Part Volume", "Vol", "Part", Part(144), ValueKind::Bipolar, 127, c(39)),
    row("Part MIDI Volume Init", "MIDI Vol", "Part", Part(160), ValueKind::Unipolar, 127, c(40)),
    row("Part Output Select", "Out", "Part", Part(176), ValueKind::Choice(&OUTPUT_SELECT_NAMES), 14, c(41)),
    row(
        "Part Effect Send",
        "FX Send",
        "Part",
        Part(192),
        ValueKind::Unipolar,
        127,
        Some(LiveParam { page: Page::PageA, number: 113 }),
    ),
    row("Part Enable", "On", "Part", MultiLocation::PartFlag(PartFlag::Enable), ValueKind::Toggle, 1, c(72)),
    row("Part MIDI Volume Enable", "Vol En", "Part", MultiLocation::PartFlag(PartFlag::MidiVolume), ValueKind::Toggle, 1, c(73)),
    row("Part Hold Pedal Enable", "Hold", "Part", MultiLocation::PartFlag(PartFlag::HoldPedal), ValueKind::Toggle, 1, c(74)),
    row(
        "Note Steal Priority",
        "Prio",
        "Part",
        MultiLocation::PartFlag(PartFlag::PriorityHigh),
        ValueKind::Choice(&PRIORITY_NAMES),
        1,
        c(77),
    ),
    row(
        "Part Prog Change Enable",
        "PC",
        "Part",
        MultiLocation::PartFlag(PartFlag::ProgramChange),
        ValueKind::Toggle,
        1,
        c(78),
    ),
];

/// The part rows of [`MULTI_PARAMS`], in table-column order.
pub fn multi_part_params() -> impl Iterator<Item = &'static MultiParamDef> {
    MULTI_PARAMS.iter().filter(|def| def.is_part_param())
}

/// The common (non-name) rows of [`MULTI_PARAMS`].
pub fn multi_common_params() -> impl Iterator<Item = &'static MultiParamDef> {
    MULTI_PARAMS.iter().filter(|def| !def.is_part_param() && def.kind != ValueKind::NameChar)
}

/// Looks up the Multi row edited by an incoming parameter change (e.g. `72 pp 27 vv` → Part
/// Volume of part `pp`).
pub fn multi_param_for_live(page: Page, number: u8) -> Option<&'static MultiParamDef> {
    MULTI_PARAMS.iter().find(|def| def.live == Some(LiveParam { page, number }))
}

/// Read-only typed view of one part (raw values; see [`MULTI_PARAMS`] for their meaning).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MultiPart {
    pub bank: u8,
    pub program: u8,
    pub midi_channel: u8,
    pub low_key: u8,
    pub high_key: u8,
    pub transpose: u8,
    pub detune: u8,
    pub volume: u8,
    pub midi_volume_init: u8,
    pub output: u8,
    pub effect_send: u8,
    /// The raw part-state byte, including internal bits.
    pub state: u8,
}

impl MultiPart {
    pub fn flag(&self, flag: PartFlag) -> bool {
        self.state & (1 << flag.bit()) != 0
    }
}

/// A complete multi program: the exact 256 data bytes of a multi dump.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Multi {
    bytes: [u8; MULTI_LEN],
}

impl Multi {
    /// Wraps the given bytes verbatim (no masking or normalisation).
    pub fn from_bytes(bytes: &[u8; MULTI_LEN]) -> Self {
        Self { bytes: *bytes }
    }

    pub fn to_bytes(&self) -> [u8; MULTI_LEN] {
        self.bytes
    }

    pub fn as_bytes(&self) -> &[u8; MULTI_LEN] {
        &self.bytes
    }

    /// The name (bytes 4..13) with trailing spaces trimmed. Bytes outside printable ASCII show as `?`.
    pub fn name(&self) -> String {
        let name: String = self.bytes[MULTI_NAME_OFFSET..MULTI_NAME_OFFSET + MULTI_NAME_LEN]
            .iter()
            .map(|&b| if (32..=126).contains(&b) { b as char } else { '?' })
            .collect();
        name.trim_end_matches(' ').to_owned()
    }

    /// Writes exactly 10 name bytes: characters outside ASCII 32..127 become `?`, the name is
    /// truncated to 10 characters and padded with spaces (SPEC §5.4).
    pub fn set_name(&mut self, name: &str) {
        let mut chars = name
            .chars()
            .map(|c| u8::try_from(c).ok().filter(|b| (32..=127).contains(b)).unwrap_or(b'?'));
        for slot in &mut self.bytes[MULTI_NAME_OFFSET..MULTI_NAME_OFFSET + MULTI_NAME_LEN] {
            *slot = chars.next().unwrap_or(b' ');
        }
    }

    /// Byte 15, 0..127 = 63..190 BPM.
    pub fn clock_tempo(&self) -> u8 {
        self.bytes[15]
    }

    pub fn clock_tempo_bpm(&self) -> u16 {
        self.bytes[15] as u16 + 63
    }

    /// Reads parameter `def` of `part` (0..15; ignored for common parameters).
    /// Part flags read as 0/1.
    pub fn get(&self, def: &MultiParamDef, part: u8) -> u8 {
        let byte = self.bytes[def.offset(part)];
        match def.location {
            MultiLocation::PartFlag(flag) => (byte >> flag.bit()) & 1,
            _ => byte,
        }
    }

    /// Writes parameter `def` of `part` and nothing else: one byte (masked to 0..127), or
    /// one bit of the part-state byte (any non-zero value sets it). Returns whether it changed.
    pub fn set(&mut self, def: &MultiParamDef, part: u8, value: u8) -> bool {
        match def.location {
            MultiLocation::PartFlag(flag) => self.set_part_flag(part, flag, value != 0),
            _ => {
                let slot = &mut self.bytes[def.offset(part)];
                let before = *slot;
                *slot = value & 0x7F;
                before != *slot
            }
        }
    }

    /// The raw part-state byte of `part` (0..15).
    pub fn part_state(&self, part: u8) -> u8 {
        self.bytes[PART_STATE_OFFSET + (part as usize % MULTI_PARTS)]
    }

    pub fn part_flag(&self, part: u8, flag: PartFlag) -> bool {
        self.part_state(part) & (1 << flag.bit()) != 0
    }

    /// Sets or clears one bit of the part-state byte, keeping every other bit (bit 4 is
    /// internal). Returns whether it changed.
    pub fn set_part_flag(&mut self, part: u8, flag: PartFlag, on: bool) -> bool {
        let slot = &mut self.bytes[PART_STATE_OFFSET + (part as usize % MULTI_PARTS)];
        let before = *slot;
        let mask = 1u8 << flag.bit();
        *slot = if on { before | mask } else { before & !mask };
        before != *slot
    }

    pub fn part_enabled(&self, part: u8) -> bool {
        self.part_flag(part, PartFlag::Enable)
    }

    pub fn set_part_enabled(&mut self, part: u8, on: bool) -> bool {
        self.set_part_flag(part, PartFlag::Enable, on)
    }

    pub fn part(&self, part: u8) -> MultiPart {
        let p = part as usize % MULTI_PARTS;
        let b = &self.bytes;
        MultiPart {
            bank: b[32 + p],
            program: b[48 + p],
            midi_channel: b[64 + p],
            low_key: b[80 + p],
            high_key: b[96 + p],
            transpose: b[112 + p],
            detune: b[128 + p],
            volume: b[144 + p],
            midi_volume_init: b[160 + p],
            output: b[176 + p],
            effect_send: b[192 + p],
            state: b[PART_STATE_OFFSET + p],
        }
    }

    /// Parses a multi dump frame. Wrong checksums are accepted (the Virus accepts them too);
    /// use [`parse_sysex`] when the checksum status matters.
    pub fn from_sysex(msg: &[u8]) -> Result<(Self, MultiDumpTarget), SyxError> {
        match parse_sysex(msg)? {
            SysExMessage::MultiDump { bank, program, data, .. } => {
                let target = MultiDumpTarget::from_raw(bank, program).ok_or(SyxError::UnsupportedMultiBank(bank))?;
                Ok((Self::from_bytes(&data), target))
            }
            _ => Err(SyxError::NotAMultiDump { command: msg[6] }),
        }
    }

    /// Encodes a 267-byte multi dump with a correct checksum.
    pub fn to_sysex(&self, device_id: u8, target: MultiDumpTarget) -> Vec<u8> {
        let (bank, program) = target.to_raw();
        encode_multi_dump(device_id, bank, program, &self.bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sysex::{encode_program_dump, DEVICE_ID_OMNI};

    fn patterned() -> Multi {
        let mut bytes = [0u8; MULTI_LEN];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = (i * 7 % 128) as u8;
        }
        Multi::from_bytes(&bytes)
    }

    fn param(name: &str) -> &'static MultiParamDef {
        MULTI_PARAMS.iter().find(|d| d.name == name).unwrap()
    }

    #[test]
    fn set_touches_only_its_own_byte_or_bit() {
        let original = patterned();
        for def in MULTI_PARAMS {
            for part in 0..16u8 {
                let mut multi = original.clone();
                let value = if matches!(def.location, MultiLocation::PartFlag(_)) {
                    1 - original.get(def, part)
                } else {
                    original.get(def, part) ^ 0x55
                };
                assert!(multi.set(def, part, value), "{} part {part}", def.name);
                assert_eq!(multi.get(def, part), value & 0x7F);
                let offset = def.offset(part);
                for (i, (&new, &old)) in multi.as_bytes().iter().zip(original.as_bytes()).enumerate() {
                    match def.location {
                        MultiLocation::PartFlag(flag) if i == offset => {
                            assert_eq!(new ^ old, 1 << flag.bit(), "{} touched other bits", def.name)
                        }
                        _ if i == offset => {}
                        _ => assert_eq!(new, old, "byte {i} changed by {} part {part}", def.name),
                    }
                }
            }
        }
    }

    #[test]
    fn table_never_addresses_internal_bytes() {
        let internal = |i: usize| i <= 3 || i == 14 || (25..=31).contains(&i) || (208..=239).contains(&i);
        for def in MULTI_PARAMS {
            for part in 0..16 {
                assert!(!internal(def.offset(part)), "{}", def.name);
            }
        }
        let bits: Vec<u8> = PartFlag::ALL.iter().map(|f| f.bit()).collect();
        assert_eq!(bits, [0, 1, 2, 5, 6]);
    }

    #[test]
    fn part_flags_preserve_internal_bits() {
        let mut bytes = [0u8; MULTI_LEN];
        bytes[PART_STATE_OFFSET + 3] = 0x18; // bits 3 and 4 (undocumented / internal)
        let mut multi = Multi::from_bytes(&bytes);
        for flag in PartFlag::ALL {
            assert!(multi.set_part_flag(3, flag, true));
            assert!(!multi.set_part_flag(3, flag, true));
        }
        assert_eq!(multi.part_state(3), 0x7F);
        assert!(multi.part(3).flag(PartFlag::PriorityHigh));
        for flag in PartFlag::ALL {
            multi.set_part_flag(3, flag, false);
        }
        assert_eq!(multi.part_state(3), 0x18);
        assert!(multi.set_part_enabled(3, true));
        assert!(multi.part_enabled(3));
        assert_eq!(multi.as_bytes()[..PART_STATE_OFFSET + 3], bytes[..PART_STATE_OFFSET + 3]);
    }

    #[test]
    fn name_sanitises_and_pads() {
        let mut multi = patterned();
        multi.set_name("Dr+Bass1RP and more");
        assert_eq!(&multi.as_bytes()[4..14], b"Dr+Bass1RP");
        multi.set_name("Sëq");
        assert_eq!(&multi.as_bytes()[4..14], b"S?q       ");
        assert_eq!(multi.name(), "S?q");
        assert_eq!(multi.as_bytes()[3], patterned().as_bytes()[3]);
        assert_eq!(multi.as_bytes()[14], patterned().as_bytes()[14]);
    }

    #[test]
    fn sysex_round_trip_and_errors() {
        let multi = patterned();
        let frame = multi.to_sysex(DEVICE_ID_OMNI, MultiDumpTarget::Bank { program: 9 });
        assert_eq!(frame.len(), 267);
        assert_eq!(frame[6..9], [0x11, 0x01, 0x09]);
        assert_eq!(Multi::from_sysex(&frame), Ok((multi.clone(), MultiDumpTarget::Bank { program: 9 })));

        let edit = multi.to_sysex(0, MultiDumpTarget::EditBuffer);
        assert_eq!(edit[6..9], [0x11, 0x00, 0x00]);
        assert_eq!(Multi::from_sysex(&edit).unwrap().1, MultiDumpTarget::EditBuffer);

        let single = encode_program_dump(0, 1, 0, multi.as_bytes());
        assert_eq!(Multi::from_sysex(&single), Err(SyxError::NotAMultiDump { command: 0x10 }));
        let bank2 = encode_multi_dump(0, 2, 0, multi.as_bytes());
        assert_eq!(Multi::from_sysex(&bank2), Err(SyxError::UnsupportedMultiBank(2)));
    }

    #[test]
    fn live_param_changes_address_the_part() {
        let vol = param("Part Volume");
        assert_eq!(vol.param_change(0x10, 3, 70).unwrap()[6..10], [0x72, 0x03, 39, 70]);
        let fx = param("Part Effect Send");
        assert_eq!(fx.param_change(0, 15, 5).unwrap()[6..10], [0x70, 0x0F, 113, 5]);
        let enable = param("Part Enable");
        assert_eq!(enable.param_change(0, 2, 1).unwrap()[6..10], [0x72, 0x02, 72, 1]);
        let bank = param("Part Bank");
        assert_eq!(bank.param_change(0, 1, 1).unwrap()[6..10], [0x72, 0x01, 32, 1]);
        let name1 = &MULTI_PARAMS[0];
        assert_eq!(name1.param_change(0, 7, b'X').unwrap()[6..10], [0x72, 0x00, 5, b'X']);
        assert_eq!(param("Multi Clock Tempo").param_change(0, 0, 1), None);
    }

    #[test]
    fn part_bank_edits_send_the_shared_c32_value() {
        let bank = param("Part Bank");
        for stored in 0..4 {
            let message = bank.param_change(0, 2, stored).unwrap();
            assert_eq!(message[6..10], [0x72, 0x02, 32, part_bank_to_c32(stored)]);
        }
    }

    #[test]
    fn incoming_part_bank_maps_back_to_the_stored_byte() {
        let bank = param("Part Bank");
        for stored in 0..4 {
            assert_eq!(bank.stored_from_live(part_bank_to_c32(stored)), Some(stored));
        }
    }

    #[test]
    fn other_rows_store_live_values_unchanged() {
        assert_eq!(param("Part Program").stored_from_live(17), Some(17));
        assert_eq!(param("Part Volume").stored_from_live(127), Some(127));
    }

    #[test]
    fn page_c_part_mapping_is_complete_and_unique() {
        let mut numbers: Vec<u8> = multi_part_params()
            .filter_map(|d| d.live)
            .filter(|l| l.page == Page::PageC)
            .map(|l| l.number)
            .collect();
        numbers.sort_unstable();
        assert_eq!(numbers, [32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 72, 73, 74, 77, 78]);
        for def in MULTI_PARAMS.iter().filter(|d| d.live.is_some()) {
            let live = def.live.unwrap();
            assert_eq!(multi_param_for_live(live.page, live.number), Some(def));
        }
        assert_eq!(multi_param_for_live(Page::PageC, 31), None);
        assert_eq!(multi_part_params().count(), 16);
        assert_eq!(multi_common_params().count(), 10);
    }

    #[test]
    fn formats_values() {
        assert_eq!(param("Part MIDI Channel").format(0), "1");
        assert_eq!(param("Part Low Key").format(36), "C2");
        assert_eq!(param("Part Volume").format(64), "0");
        assert_eq!(param("Part Output Select").format(4), "Out2 L+R");
        assert_eq!(param("Part Output Select").format(12), "#12");
        assert_eq!(param("Part Bank").format(1), "B");
    }
}

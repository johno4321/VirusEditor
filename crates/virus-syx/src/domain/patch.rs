//! The 256-byte Access Virus single patch. The raw bytes are the single source of truth (SPEC §5.1).

use crate::params::{ASSIGN_LAYOUT, LFO_ASSIGN_LAYOUT};
use crate::sysex::{encode_program_dump, parse_sysex, Page, SysExMessage, SyxError, PART_SINGLE_BUFFER};
use crate::types::{AssignSource, Category};

pub const PATCH_TOTAL_LEN: usize = 256;

/// Name location: B112..B121.
const NAME_OFFSET: usize = 128 + 112;
pub const NAME_LEN: usize = 10;
const CATEGORY1_OFFSET: usize = 128 + 123;
const CATEGORY2_OFFSET: usize = 128 + 124;
/// A0: Sound Version Number, owned by the synth.
const SOUND_VERSION_OFFSET: usize = 0;
/// Sound version written by OS 2.00 (all factory dumps).
pub const OS2_SOUND_VERSION: u8 = 6;

/// Single dump bank (`bb` = 1..4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SingleBank {
    A,
    B,
    C,
    D,
}

impl SingleBank {
    pub const ALL: [Self; 4] = [Self::A, Self::B, Self::C, Self::D];

    pub fn to_raw(self) -> u8 {
        match self {
            Self::A => 1,
            Self::B => 2,
            Self::C => 3,
            Self::D => 4,
        }
    }

    pub fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            1 => Some(Self::A),
            2 => Some(Self::B),
            3 => Some(Self::C),
            4 => Some(Self::D),
            _ => None,
        }
    }

    /// Banks C and D are Flash ROM.
    pub fn is_rom(self) -> bool {
        matches!(self, Self::C | Self::D)
    }

    pub fn letter(self) -> char {
        (b'A' + self.to_raw() - 1) as char
    }
}

/// Where a single dump goes / came from (`bb ss`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DumpTarget {
    /// `bb = 00`: edit buffer of `part` (`0x00..=0x0F` Multi part, `0x40` Single mode).
    EditBuffer { part: u8 },
    /// `bb = 01..04`: bank slot.
    Bank { bank: SingleBank, program: u8 },
}

impl DumpTarget {
    /// The Single-mode edit buffer.
    pub const SINGLE_BUFFER: Self = Self::EditBuffer { part: PART_SINGLE_BUFFER };

    /// Decodes `bb`/`ss`; `None` for undocumented bank numbers (e.g. banks E–H, Q2).
    pub fn from_raw(bb: u8, ss: u8) -> Option<Self> {
        if bb == 0 {
            Some(Self::EditBuffer { part: ss })
        } else {
            SingleBank::from_raw(bb).map(|bank| Self::Bank { bank, program: ss })
        }
    }

    /// Returns `(bb, ss)`.
    pub fn to_raw(self) -> (u8, u8) {
        match self {
            Self::EditBuffer { part } => (0, part),
            Self::Bank { bank, program } => (bank.to_raw(), program),
        }
    }
}

/// One (destination, amount) pair of an Assign slot. Raw bytes; destinations are unverified (Q1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AssignTarget {
    pub destination: u8,
    /// Bipolar amount, stored 0..127 (64 = 0).
    pub amount: u8,
}

/// Read-only view of Assign 1, 2 or 3.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AssignSlot {
    pub source: AssignSource,
    pub targets: Vec<AssignTarget>,
}

/// A complete single sound: the exact 256 data bytes of a single dump.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Patch {
    bytes: [u8; PATCH_TOTAL_LEN],
}

impl Default for Patch {
    fn default() -> Self {
        Self::init()
    }
}

impl Patch {
    /// Wraps the given bytes verbatim (no masking or normalisation).
    pub fn from_bytes(bytes: &[u8; PATCH_TOTAL_LEN]) -> Self {
        Self { bytes: *bytes }
    }

    pub fn to_bytes(&self) -> [u8; PATCH_TOTAL_LEN] {
        self.bytes
    }

    pub fn as_bytes(&self) -> &[u8; PATCH_TOTAL_LEN] {
        &self.bytes
    }

    /// Reads parameter `number` of Page A or B. Page C is not part of a single and always reads `0`.
    pub fn get(&self, page: Page, number: u8) -> u8 {
        match offset(page, number) {
            Some(index) => self.bytes[index],
            None => 0,
        }
    }

    /// Writes one byte (masked to 0..127) and nothing else.
    ///
    /// No-op for Page C (not part of a single) and for A0 (Sound Version, owned by the synth, SPEC §5.2).
    pub fn set(&mut self, page: Page, number: u8, value: u8) {
        if let Some(index) = offset(page, number) {
            if index != SOUND_VERSION_OFFSET {
                self.bytes[index] = value & 0x7F;
            }
        }
    }

    /// The name (B112..B121) with trailing spaces trimmed. Bytes outside printable ASCII show as `?`.
    pub fn name(&self) -> String {
        let name: String = self.bytes[NAME_OFFSET..NAME_OFFSET + NAME_LEN]
            .iter()
            .map(|&b| if (32..=126).contains(&b) { b as char } else { '?' })
            .collect();
        name.trim_end_matches(' ').to_owned()
    }

    /// Writes exactly 10 name bytes: characters outside ASCII 32..127 become `?`, the name is
    /// truncated to 10 characters and padded with spaces (SPEC §5.4).
    pub fn set_name(&mut self, name: &str) {
        let mut chars = name.chars().map(|c| {
            u8::try_from(c).ok().filter(|b| (32..=127).contains(b)).unwrap_or(b'?')
        });
        for slot in &mut self.bytes[NAME_OFFSET..NAME_OFFSET + NAME_LEN] {
            *slot = chars.next().unwrap_or(b' ');
        }
    }

    pub fn category1(&self) -> Category {
        Category::from_raw(self.bytes[CATEGORY1_OFFSET])
    }

    pub fn category2(&self) -> Category {
        Category::from_raw(self.bytes[CATEGORY2_OFFSET])
    }

    pub fn set_category1(&mut self, category: Category) {
        self.bytes[CATEGORY1_OFFSET] = category.to_raw() & 0x7F;
    }

    pub fn set_category2(&mut self, category: Category) {
        self.bytes[CATEGORY2_OFFSET] = category.to_raw() & 0x7F;
    }

    /// A0 Sound Version Number (read-only for the editor).
    pub fn sound_version(&self) -> u8 {
        self.bytes[SOUND_VERSION_OFFSET]
    }

    /// Assign 1, 2 and 3 (B64..B78).
    pub fn assign_slots(&self) -> [AssignSlot; 3] {
        ASSIGN_LAYOUT.map(|layout| AssignSlot {
            source: AssignSource::from_raw(self.get(Page::PageB, layout.source)),
            targets: layout
                .targets
                .iter()
                .map(|&(destination, amount)| self.assign_target(destination, amount))
                .collect(),
        })
    }

    /// LFO1 and LFO2 assign (B79/B80, B81/B82).
    pub fn lfo_assigns(&self) -> [AssignTarget; 2] {
        LFO_ASSIGN_LAYOUT.map(|(destination, amount)| self.assign_target(destination, amount))
    }

    fn assign_target(&self, destination: u8, amount: u8) -> AssignTarget {
        AssignTarget {
            destination: self.get(Page::PageB, destination),
            amount: self.get(Page::PageB, amount),
        }
    }

    /// A neutral starting sound: name "Init", OS 2.0 sound version, a plain open saw.
    /// Undocumented bytes are 0.
    pub fn init() -> Self {
        let mut bytes = [0u8; PATCH_TOTAL_LEN];
        bytes[SOUND_VERSION_OFFSET] = OS2_SOUND_VERSION;
        // Bipolar parameters centred at 64.
        const PAGE_A_CENTRED: &[u8] = &[
            10, 17, 18, 20, 22, 23, 25, 29, 30, 33, 36, 39, 41, 46, 47, 48, 55, 56, 57, 60, 62, 67,
            71, 74, 75, 76, 77, 78, 79, 83, 86, 87, 88, 89, 90, 93, 100, 106, 107, 108, 109, 114,
            116, 119,
        ];
        const PAGE_B_CENTRED: &[u8] = &[
            5, 7, 47, 48, 49, 50, 54, 55, 56, 57, 60, 61, 66, 69, 71, 74, 76, 78, 80, 82, 86, 87,
            88, 98,
        ];
        for &n in PAGE_A_CENTRED {
            bytes[n as usize] = 64;
        }
        for &n in PAGE_B_CENTRED {
            bytes[128 + n as usize] = 64;
        }
        let page_a: &[(u8, u8)] = &[
            (21, 96),  // Osc1 Keyfollow +32
            (31, 96),  // Osc2 Keyfollow +32
            (40, 127), // Cutoff open
            (61, 127), // Amp Env Sustain
            (91, 100), // Patch Volume (unity)
        ];
        let page_b: &[(u8, u8)] = &[
            (16, 57), // Clock Tempo 120 BPM
            (17, 1),  // Arp Clock (lowest documented value)
            (26, 66), // Bender Up +2
            (27, 62), // Bender Down -2
            (30, 1),  // Filter1 Env Polarity Positive
            (31, 1),  // Filter2 Env Polarity Positive
            (33, 60), // Filter Keytrack Base C4
        ];
        for &(n, v) in page_a {
            bytes[n as usize] = v;
        }
        for &(n, v) in page_b {
            bytes[128 + n as usize] = v;
        }
        let mut patch = Self { bytes };
        patch.set_name("Init");
        patch
    }

    /// Parses a single dump frame. Wrong checksums are accepted (the Virus accepts them too);
    /// use [`parse_sysex`] when the checksum status matters.
    pub fn from_sysex(msg: &[u8]) -> Result<(Self, DumpTarget), SyxError> {
        match parse_sysex(msg)? {
            SysExMessage::SingleDump { bank, program, data, .. } => {
                let target = DumpTarget::from_raw(bank, program).ok_or(SyxError::UnsupportedBank(bank))?;
                Ok((Self::from_bytes(&data), target))
            }
            _ => Err(SyxError::NotASingleDump { command: msg[6] }),
        }
    }

    /// Encodes a 267-byte single dump with a correct checksum.
    pub fn to_sysex(&self, device_id: u8, target: DumpTarget) -> Vec<u8> {
        let (bank, program) = target.to_raw();
        encode_program_dump(device_id, bank, program, &self.bytes)
    }
}

fn offset(page: Page, number: u8) -> Option<usize> {
    if number > 127 {
        return None;
    }
    page.dump_offset().map(|base| base + number as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sysex::{encode_param_change, DEVICE_ID_OMNI};

    fn patterned() -> Patch {
        let mut bytes = [0u8; 256];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = (i * 7 % 128) as u8;
        }
        Patch::from_bytes(&bytes)
    }

    #[test]
    fn set_touches_only_its_own_byte() {
        let original = patterned();
        for (page, base) in [(Page::PageA, 0usize), (Page::PageB, 128)] {
            for number in 1..128u8 {
                let mut patch = original.clone();
                patch.set(page, number, 0xFF);
                for (i, (&new, &old)) in patch.as_bytes().iter().zip(original.as_bytes()).enumerate() {
                    if i == base + number as usize {
                        assert_eq!(new, 0x7F);
                    } else {
                        assert_eq!(new, old, "byte {i} changed by {page:?} {number}");
                    }
                }
            }
        }
    }

    #[test]
    fn sound_version_and_page_c_are_immutable() {
        let mut patch = patterned();
        let before = patch.clone();
        patch.set(Page::PageA, 0, 42);
        patch.set(Page::PageC, 10, 42);
        assert_eq!(patch, before);
        assert_eq!(patch.get(Page::PageC, 10), 0);
    }

    #[test]
    fn set_name_sanitises_and_pads() {
        let mut patch = Patch::init();
        patch.set_name("Ünïcödé✓ Lead Extra");
        let name_bytes = &patch.as_bytes()[NAME_OFFSET..NAME_OFFSET + NAME_LEN];
        assert!(name_bytes.iter().all(|&b| (32..=127).contains(&b)));
        assert_eq!(patch.name(), "?n?c?d?? L");

        patch.set_name("Bass");
        assert_eq!(&patch.as_bytes()[NAME_OFFSET..NAME_OFFSET + NAME_LEN], b"Bass      ");
        assert_eq!(patch.name(), "Bass");

        patch.set_name("a\tb\n");
        assert_eq!(patch.name(), "a?b?");
    }

    #[test]
    fn unknown_category_round_trips() {
        let mut patch = Patch::init();
        patch.set_category2(Category::Unknown(127));
        assert_eq!(patch.as_bytes()[CATEGORY2_OFFSET], 127);
        assert_eq!(patch.category2(), Category::Unknown(127));
        patch.set_category1(Category::Favourite2);
        assert_eq!(patch.as_bytes()[CATEGORY1_OFFSET], 15);
    }

    #[test]
    fn init_is_a_valid_os2_sound() {
        let patch = Patch::default();
        assert_eq!(patch.name(), "Init");
        assert_eq!(patch.sound_version(), OS2_SOUND_VERSION);
        assert!(patch.as_bytes().iter().all(|&b| b <= 127));
    }

    #[test]
    fn from_sysex_rejects_non_dumps_and_unknown_banks() {
        let param = encode_param_change(DEVICE_ID_OMNI, Page::PageA, 0x40, 40, 1);
        assert_eq!(Patch::from_sysex(&param), Err(SyxError::NotASingleDump { command: 0x70 }));

        let dump = encode_program_dump(DEVICE_ID_OMNI, 5, 0, Patch::init().as_bytes());
        assert_eq!(Patch::from_sysex(&dump), Err(SyxError::UnsupportedBank(5)));

        let mut dump = Patch::init().to_sysex(DEVICE_ID_OMNI, DumpTarget::SINGLE_BUFFER);
        dump[265] ^= 1;
        let (_, target) = Patch::from_sysex(&dump).expect("bad checksum is accepted");
        assert_eq!(target, DumpTarget::EditBuffer { part: 0x40 });
    }

    #[test]
    fn assign_view_reads_rack_layout() {
        let mut patch = Patch::init();
        patch.set(Page::PageB, 72, 21); // Assign3 source Lfo1
        patch.set(Page::PageB, 77, 114);
        patch.set(Page::PageB, 78, 10);
        let slots = patch.assign_slots();
        assert_eq!(slots.iter().map(|s| s.targets.len()).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(slots[2].source, AssignSource::Lfo1);
        assert_eq!(slots[2].targets[2], AssignTarget { destination: 114, amount: 10 });
    }
}

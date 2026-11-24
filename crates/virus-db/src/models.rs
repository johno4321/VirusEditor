//! Database models, query filters, import/export types and category definitions.

use std::fmt;

use crate::multi_data::MULTI_LEN;
use crate::patch_data::PATCH_LEN;

/// Sound category (Page B 123/124), SPEC §4.5. Values outside 0..=16 are kept as `Unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SoundCategory {
    #[default]
    Off,
    Lead,
    Bass,
    Pad,
    Decay,
    Pluck,
    Acid,
    Classic,
    Arpeggiator,
    Effects,
    Drums,
    Percussion,
    Input,
    Vocoder,
    Favourite1,
    Favourite2,
    Favourite3,
    Unknown(u8),
}

impl SoundCategory {
    /// The 17 documented categories, in raw value order (index == raw value).
    pub const ALL: [SoundCategory; 17] = [
        Self::Off,
        Self::Lead,
        Self::Bass,
        Self::Pad,
        Self::Decay,
        Self::Pluck,
        Self::Acid,
        Self::Classic,
        Self::Arpeggiator,
        Self::Effects,
        Self::Drums,
        Self::Percussion,
        Self::Input,
        Self::Vocoder,
        Self::Favourite1,
        Self::Favourite2,
        Self::Favourite3,
    ];

    pub fn from_raw(value: u8) -> Self {
        Self::ALL
            .get(value as usize)
            .copied()
            .unwrap_or(Self::Unknown(value))
    }

    pub fn to_raw(self) -> u8 {
        match self {
            Self::Unknown(v) => v,
            known => Self::ALL.iter().position(|&c| c == known).unwrap_or(0) as u8,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Lead => "Lead",
            Self::Bass => "Bass",
            Self::Pad => "Pad",
            Self::Decay => "Decay",
            Self::Pluck => "Pluck",
            Self::Acid => "Acid",
            Self::Classic => "Classic",
            Self::Arpeggiator => "Arpeggiator",
            Self::Effects => "Effects",
            Self::Drums => "Drums",
            Self::Percussion => "Percussion",
            Self::Input => "Input",
            Self::Vocoder => "Vocoder",
            Self::Favourite1 => "Favourite 1",
            Self::Favourite2 => "Favourite 2",
            Self::Favourite3 => "Favourite 3",
            Self::Unknown(_) => "Unknown",
        }
    }
}

impl fmt::Display for SoundCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(v) => write!(f, "Unknown ({v})"),
            other => f.write_str(other.name()),
        }
    }
}

/// Bank descriptor. `bank_letter` is `Some("A".."D")` for the synth banks, `None` for user banks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BankRecord {
    pub id: i64,
    pub name: String,
    pub bank_letter: Option<String>,
    /// Banks C and D are Flash ROM on the synth and must not be sent as bank dumps.
    pub is_rom: bool,
}

/// User metadata stored alongside a patch (not part of the patch bytes).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PatchMeta {
    pub tags: String,
    pub favorite: bool,
}

/// Full patch record. `data` is the byte-exact source of truth; `name` and the categories
/// are derived from it when saved and are only cached in the DB for searching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchRecord {
    pub id: i64,
    pub bank_id: i64,
    pub program_number: u8,
    pub data: [u8; PATCH_LEN],
    pub name: String,
    pub category1: SoundCategory,
    pub category2: SoundCategory,
    pub tags: String,
    pub favorite: bool,
    /// `false` if the dump this patch was imported from had a wrong checksum.
    pub checksum_ok: bool,
}

/// Lightweight row for tables and search results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchSummary {
    pub id: i64,
    pub bank_id: i64,
    pub program_number: u8,
    pub name: String,
    pub category1: SoundCategory,
    pub category2: SoundCategory,
    pub tags: String,
    pub favorite: bool,
    pub checksum_ok: bool,
}

/// Filter criteria for [`crate::PatchDatabase::query_patches`].
#[derive(Debug, Clone, Default)]
pub struct PatchFilter {
    /// Case-insensitive literal substring search across name and tags.
    pub search_text: Option<String>,
    pub bank_id: Option<i64>,
    /// Matches category1 OR category2 (any value, including `Off` and `Unknown(n)`).
    pub category: Option<SoundCategory>,
    pub favorite_only: bool,
}

/// One received/parsed Single Dump (`F0 00 20 33 01 dd 10 bb ss ... F7`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingDump {
    /// Raw `bb`: 0 = edit buffer, 1..=4 = Single Bank A..D.
    pub bank: u8,
    /// Raw `ss`: program 0..=127, or the part number for edit-buffer dumps.
    pub program: u8,
    pub data: [u8; PATCH_LEN],
    pub checksum_ok: bool,
}

/// Where [`crate::PatchDatabase::import_single_dumps`] writes bank dumps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportTarget {
    /// `bb` 1..=4 go to the library banks with letters A..D (created if missing).
    OriginalBanks,
    /// Everything goes into this bank, by program number. All bank dumps must share one `bb`.
    Bank(i64),
}

/// Outcome of an import.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// Number of slots written.
    pub imported: usize,
    /// Slots written per bank name, in order of first appearance.
    pub per_bank: Vec<(String, usize)>,
    /// Edit-buffer dumps (`bb == 0`); never written to a slot. The UI may offer to store them.
    pub loose: Vec<IncomingDump>,
    /// Dumps with a wrong checksum (they are still imported/returned, and flagged).
    pub bad_checksum: usize,
    /// Dumps that could not be placed (unknown `bb`, program > 127, or data bytes ≥ 0x80).
    pub skipped: usize,
    /// Slots that received more than one dump in this import. The later dump wins; the
    /// earlier ones are not counted in `imported`/`per_bank`.
    pub duplicates: Vec<SlotRef>,
}

/// A library slot: bank name and program number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotRef {
    pub bank_id: i64,
    pub bank_name: String,
    pub program: u8,
}

/// An occupied slot an import would replace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotOverwrite {
    pub slot: SlotRef,
    pub old_name: String,
    pub new_name: String,
}

/// What [`crate::PatchDatabase::plan_import`] predicts for an import, without writing anything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportPlan {
    /// Occupied slots that would be replaced (the last dump for each slot).
    pub overwrites: Vec<SlotOverwrite>,
    /// Slots addressed by more than one dump in the input.
    pub duplicates: Vec<SlotRef>,
    /// Dumps that would be written (distinct slots).
    pub slots: usize,
    /// Dumps that would be skipped (unknown `bb`, program > 127, 8-bit data).
    pub skipped: usize,
}

/// What to do with patches flagged `checksum_ok = false` when exporting or sending a bank.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BadChecksumPolicy {
    /// Leave them out (default): their data is known or suspected to be damaged.
    #[default]
    Skip,
    /// Include them with a deliberately wrong checksum, so the damage stays detectable.
    KeepInvalid,
    /// Include them with a freshly computed, valid checksum. This hides the damage: only
    /// use it with the user's explicit consent.
    Rechecksum,
}

/// Frames of a bank export plus what was left out or altered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BankExport {
    pub frames: Vec<Vec<u8>>,
    /// Flagged patches left out under [`BadChecksumPolicy::Skip`].
    pub skipped_flagged: usize,
    /// Flagged patches included (with an invalid or a recomputed checksum).
    pub included_flagged: usize,
    /// Patches left out because their data contains bytes ≥ 0x80.
    pub skipped_invalid: usize,
}

/// One received/parsed Multi Dump (`F0 00 20 33 01 dd 11 bb mm ... F7`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingMultiDump {
    /// Raw `bb`: 0 = Multi edit buffer, 1 = Multi bank.
    pub bank: u8,
    /// Raw `mm`: Multi bank program 0..=127 (ignored for the edit buffer).
    pub program: u8,
    pub data: [u8; MULTI_LEN],
    pub checksum_ok: bool,
}

/// A stored multi. `data` is the byte-exact source of truth; `name` is derived from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiRecord {
    pub id: i64,
    /// Multi bank program 0..=127, or `None` for an unslotted multi.
    pub slot: Option<u8>,
    pub data: [u8; MULTI_LEN],
    pub name: String,
    pub tags: String,
    pub favorite: bool,
    pub checksum_ok: bool,
}

/// Lightweight multi row for lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiSummary {
    pub id: i64,
    pub slot: Option<u8>,
    pub name: String,
    pub tags: String,
    pub favorite: bool,
    pub checksum_ok: bool,
}

/// Outcome of [`crate::PatchDatabase::import_multi_dumps`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MultiImportReport {
    /// Multi bank slots written (distinct slots).
    pub imported: usize,
    /// Multi edit-buffer dumps (`bb == 0`); never written to a slot.
    pub loose: Vec<IncomingMultiDump>,
    /// Dumps with a wrong checksum (still imported/returned, and flagged).
    pub bad_checksum: usize,
    /// Dumps that could not be placed (`bb` > 1, program > 127, or data bytes ≥ 0x80).
    pub skipped: usize,
    /// Multi bank slots that received more than one dump in this import. The later dump wins;
    /// the earlier ones are not counted in `imported`.
    pub duplicates: Vec<u8>,
}

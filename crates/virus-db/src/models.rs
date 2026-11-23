//! Database models, query filters, and category definitions.
//!
//! Categories match the 17 official Access Virus categories defined in Chapter 13 (p. 84)
//! of the Access Virus Rack manual.

use virus_syx::domain::Patch;

/// The official 17 sound categories recognized by the Access Virus Rack / B.
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
}

impl SoundCategory {
    pub fn as_str(&self) -> &'static str {
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
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "lead" => Self::Lead,
            "bass" => Self::Bass,
            "pad" => Self::Pad,
            "decay" => Self::Decay,
            "pluck" | "plug" => Self::Pluck,
            "acid" => Self::Acid,
            "classic" => Self::Classic,
            "arpeggiator" | "arp" => Self::Arpeggiator,
            "effects" | "fx" => Self::Effects,
            "drums" | "drum" => Self::Drums,
            "percussion" | "perc" => Self::Percussion,
            "input" | "inp" => Self::Input,
            "vocoder" | "voc" => Self::Vocoder,
            "favourite 1" | "fav 1" | "favorite 1" => Self::Favourite1,
            "favourite 2" | "fav 2" | "favorite 2" => Self::Favourite2,
            "favourite 3" | "fav 3" | "favorite 3" => Self::Favourite3,
            _ => Self::Off,
        }
    }
}

/// Bank descriptor record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BankRecord {
    pub id: i64,
    pub name: String,
    pub bank_letter: Option<String>,
    pub is_factory: bool,
}

/// Detailed patch record including the deserialized 256-byte `Patch` and user metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchRecord {
    pub id: i64,
    pub bank_id: i64,
    pub program_number: u8,
    pub name: String,
    pub category1: SoundCategory,
    pub category2: SoundCategory,
    pub patch: Patch,
    pub checksum: u8,
    pub tags: String,
    pub favorite: bool,
}

/// Lightweight summary for virtualized tables and fast UI search grids.
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
}

/// Filter criteria for querying the patch library.
#[derive(Debug, Clone, Default)]
pub struct PatchFilter {
    /// Substring search across patch name and tags (case-insensitive).
    pub search_text: Option<String>,
    /// Filter by specific Bank ID.
    pub bank_id: Option<i64>,
    /// Filter by sound category (matches category1 OR category2).
    pub category: Option<SoundCategory>,
    /// Show favorites only.
    pub favorite_only: bool,
}

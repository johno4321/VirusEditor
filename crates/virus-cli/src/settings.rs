//! How the tool addresses the synth: SysEx device ID, MIDI channel and edit-buffer part,
//! plus the parsers clap uses for them.

use std::fmt;

use virus_syx::{DEVICE_ID_OMNI, PART_SINGLE_BUFFER};

/// SysEx device ID byte: `0x00..=0x0F` = device 1..16, `0x10` = Omni (SPEC §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceId(pub u8);

impl DeviceId {
    /// OS 1.50+ default: device 1.
    pub const DEFAULT: Self = Self(0x00);

    /// A message from device `dd` is for us if either side is Omni or the IDs match.
    pub fn accepts(self, dd: u8) -> bool {
        self.0 == DEVICE_ID_OMNI || dd == DEVICE_ID_OMNI || dd == self.0
    }
}

impl Default for DeviceId {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 == DEVICE_ID_OMNI {
            write!(f, "omni (0x10)")
        } else {
            write!(f, "{} (0x{:02X})", self.0 + 1, self.0)
        }
    }
}

/// Parses `1`..`16` or `omni`.
pub fn parse_device(text: &str) -> Result<DeviceId, String> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("omni") {
        return Ok(DeviceId(DEVICE_ID_OMNI));
    }
    match text.parse::<u8>() {
        Ok(n @ 1..=16) => Ok(DeviceId(n - 1)),
        _ => Err(format!("device must be 1-16 or 'omni', got '{text}'")),
    }
}

/// The edit buffer the tool talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Part {
    /// Single mode: part byte `0x40`.
    #[default]
    Single,
    /// Multi / Multi-Single part 0..=15 (shown 1..=16).
    Multi(u8),
}

impl Part {
    /// The SysEx part byte (`pp` / `ss`).
    pub fn raw(self) -> u8 {
        match self {
            Self::Single => PART_SINGLE_BUFFER,
            Self::Multi(part) => part & 0x0F,
        }
    }
}

impl fmt::Display for Part {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Single => write!(f, "single (0x40)"),
            Self::Multi(part) => write!(f, "multi part {} (0x{:02X})", part + 1, part),
        }
    }
}

/// Parses `single` or `1`..`16`.
pub fn parse_part(text: &str) -> Result<Part, String> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("single") {
        return Ok(Part::Single);
    }
    match text.parse::<u8>() {
        Ok(n @ 1..=16) => Ok(Part::Multi(n - 1)),
        _ => Err(format!("part must be 'single' or 1-16, got '{text}'")),
    }
}

/// Parses a MIDI channel `1`..`16` into `0..=15`.
pub fn parse_channel(text: &str) -> Result<u8, String> {
    match text.trim().parse::<u8>() {
        Ok(n @ 1..=16) => Ok(n - 1),
        _ => Err(format!("channel must be 1-16, got '{text}'")),
    }
}

/// Everything needed to address one synth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Target {
    pub device: DeviceId,
    /// MIDI channel 0..=15, used to interpret incoming CC / Poly Pressure.
    pub channel: u8,
    pub part: Part,
}

/// Single bank byte `bb` from a letter: A..D = 1..4 (documented), E..H = 5..8 (undocumented, Q2).
pub fn parse_bank_letter(text: &str) -> Result<u8, String> {
    let mut chars = text.trim().chars();
    match (chars.next().map(|c| c.to_ascii_uppercase()), chars.next()) {
        (Some(c @ 'A'..='H'), None) => Ok(c as u8 - b'A' + 1),
        _ => Err(format!("bank must be a letter A-H, got '{text}'")),
    }
}

/// `1` → `A` … `8` → `H`; other values as `bb=0xNN`.
pub fn bank_label(bb: u8) -> String {
    match bb {
        1..=8 => ((b'A' + bb - 1) as char).to_string(),
        other => format!("bb=0x{other:02X}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_parses_one_based_and_omni() {
        assert_eq!(parse_device("1"), Ok(DeviceId(0x00)));
        assert_eq!(parse_device("16"), Ok(DeviceId(0x0F)));
        assert_eq!(parse_device("OMNI"), Ok(DeviceId(0x10)));
        assert!(parse_device("0").is_err());
        assert!(parse_device("17").is_err());
        assert_eq!(DeviceId::default(), DeviceId(0x00));
    }

    #[test]
    fn device_acceptance_honours_omni_on_either_side() {
        assert!(DeviceId(0).accepts(0));
        assert!(!DeviceId(0).accepts(1));
        assert!(DeviceId(0).accepts(0x10));
        assert!(DeviceId(0x10).accepts(5));
    }

    #[test]
    fn part_and_channel_parse() {
        assert_eq!(parse_part("single").map(Part::raw), Ok(0x40));
        assert_eq!(parse_part("1").map(Part::raw), Ok(0x00));
        assert_eq!(parse_part("16").map(Part::raw), Ok(0x0F));
        assert!(parse_part("17").is_err());
        assert_eq!(parse_channel("1"), Ok(0));
        assert_eq!(parse_channel("16"), Ok(15));
        assert!(parse_channel("0").is_err());
    }

    #[test]
    fn bank_letters_map_to_bank_bytes() {
        assert_eq!(parse_bank_letter("a"), Ok(1));
        assert_eq!(parse_bank_letter("D"), Ok(4));
        assert_eq!(parse_bank_letter("H"), Ok(8));
        assert!(parse_bank_letter("I").is_err());
        assert!(parse_bank_letter("AB").is_err());
        assert_eq!(bank_label(2), "B");
        assert_eq!(bank_label(9), "bb=0x09");
    }
}

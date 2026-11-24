//! Parameter addresses like `A40` / `B39` / `C32` and value parsing for them.

use std::fmt;

use virus_syx::{format_value, single_param, Page, ParamClass, ParamDef};

/// A Page A or Page B single parameter (dump byte `number` or `128 + number`), or a Page C
/// parameter (SysEx only: multi, part and global settings, SPEC §4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParamAddress {
    pub page: Page,
    pub number: u8,
}

/// Page C parameters that would change how the synth talks to this tool (device ID, channel,
/// play mode, dump enables, …): `set` refuses them without `--force`.
const DISRUPTIVE_PAGE_C: [u8; 10] = [93, 94, 95, 98, 99, 118, 120, 122, 123, 124];

/// Names of the documented Page C parameters (SPEC §4.3).
fn page_c_name(number: u8) -> Option<&'static str> {
    Some(match number {
        5..=14 => "Multi Name character",
        22 => "Delay Output Select",
        31 => "Part Bank Select",
        32 => "Part Bank Change",
        33 => "Part Program Change",
        34 => "Part MIDI Channel",
        35 => "Part Low Key",
        36 => "Part High Key",
        37 => "Part Transpose",
        38 => "Part Detune",
        39 => "Part Volume",
        40 => "Part MIDI Volume Init",
        41 => "Part Output Select",
        72 => "Part Enable",
        73 => "Part MIDI Volume Enable",
        74 => "Part Hold Enable",
        75 => "Keyboard to MIDI",
        77 => "Note Steal Priority",
        78 => "Part Program Change Enable",
        85 => "Program Change Enable",
        86 => "Multi Program Change Enable",
        87 => "MIDI Volume Enable",
        90 => "Input Thru",
        91 => "Input Boost",
        92 => "Master Tune",
        93 => "Device ID",
        94 => "MIDI Control Low Page",
        95 => "MIDI Control High Page",
        96 => "Arp Send",
        97 => "Knob Display",
        98 => "MIDI Dump Tx",
        99 => "MIDI Dump Rx",
        105 => "Multi Program Change",
        106 => "Clock Rx",
        110..=115 => "Definable",
        116 => "Expert Mode",
        117 => "Knob Mode",
        118 => "Memory Protect",
        120 => "Soft Thru",
        121 => "Panel Destination",
        122 => "Play Mode",
        123 => "Part Number",
        124 => "Global Channel",
        125 => "LED Mode",
        126 => "LCD Contrast",
        127 => "Master Volume",
        _ => return None,
    })
}

impl ParamAddress {
    pub const fn a(number: u8) -> Self {
        Self { page: Page::PageA, number }
    }

    pub const fn b(number: u8) -> Self {
        Self { page: Page::PageB, number }
    }

    pub const fn c(number: u8) -> Self {
        Self { page: Page::PageC, number }
    }

    /// The documented table row, if any (Page A/B only).
    pub fn def(self) -> Option<&'static ParamDef> {
        single_param(self.page, self.number)
    }

    pub fn name(self) -> &'static str {
        let name = match self.page {
            Page::PageC => page_c_name(self.number),
            _ => self.def().map(|d| d.name),
        };
        name.unwrap_or("(undocumented)")
    }

    /// Whether the value is part of the 256-byte single dump (Page A/B), so it can be read back.
    pub fn in_single_dump(self) -> bool {
        self.page != Page::PageC
    }

    /// Offset in the 256-byte patch. Only meaningful for Page A/B (see [`Self::in_single_dump`]).
    pub fn offset(self) -> usize {
        debug_assert!(self.in_single_dump(), "{self} is not part of the single dump");
        match self.page {
            Page::PageB => 128 + self.number as usize,
            _ => self.number as usize,
        }
    }

    /// `"95"` plus the formatted value when the parameter is documented, e.g. `"95 (+31)"`.
    pub fn describe_value(self, raw: u8) -> String {
        match self.def() {
            Some(def) => {
                let text = format_value(def, raw);
                if text == raw.to_string() {
                    text
                } else {
                    format!("{raw} ({text})")
                }
            }
            None => raw.to_string(),
        }
    }

    /// Why a parameter change to this address shouldn't be sent without `--force`, if any.
    pub fn edit_warning(self) -> Option<&'static str> {
        if self.page == Page::PageC {
            return if DISRUPTIVE_PAGE_C.contains(&self.number) {
                Some("a global setting that can break this session (device ID, channel, play mode, dumps, …)")
            } else if page_c_name(self.number).is_none() {
                Some("an undocumented Page C parameter")
            } else {
                None
            };
        }
        match self.def().map(|d| d.class) {
            Some(ParamClass::Sound) => None,
            Some(ParamClass::Performance) => Some("a performance controller (not stored in the sound)"),
            Some(ParamClass::Internal) => Some("internal (A0 Sound Version must never be changed)"),
            None => Some("undocumented"),
        }
    }
}

impl fmt::Display for ParamAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let letter = match self.page {
            Page::PageA => 'A',
            Page::PageB => 'B',
            Page::PageC => 'C',
        };
        write!(f, "{letter}{}", self.number)
    }
}

/// Parses `A0`..`A127` / `B0`..`B127` (case-insensitive): the single's parameters, which can be
/// read back from the edit buffer (probes). Page C is refused here; `set` uses
/// [`parse_set_address`].
pub fn parse_address(text: &str) -> Result<ParamAddress, String> {
    let address = parse_set_address(text)?;
    if address.page == Page::PageC {
        return Err("Page C (multi/global) parameters can't be read back from the single dump; only `set` takes them".to_owned());
    }
    Ok(address)
}

/// Parses `A0`..`A127`, `B0`..`B127` or `C0`..`C127` (case-insensitive), for `set`.
pub fn parse_set_address(text: &str) -> Result<ParamAddress, String> {
    let text = text.trim();
    let mut chars = text.chars();
    let page = match chars.next().map(|c| c.to_ascii_uppercase()) {
        Some('A') => Page::PageA,
        Some('B') => Page::PageB,
        Some('C') => Page::PageC,
        _ => return Err(format!("expected a parameter like A40, B39 or C32, got '{text}'")),
    };
    match chars.as_str().parse::<u8>() {
        Ok(number @ 0..=127) => Ok(ParamAddress { page, number }),
        _ => Err(format!("parameter number must be 0-127, got '{text}'")),
    }
}

/// Parses a raw value `0..=127`, or a signed bipolar value `+n` / `-n` (stored as `64 + n`).
pub fn parse_value(text: &str) -> Result<u8, String> {
    let text = text.trim();
    let error = || format!("value must be 0-127 or a signed bipolar value -64..+63, got '{text}'");
    if text.starts_with('+') || text.starts_with('-') {
        let signed: i16 = text.parse().map_err(|_| error())?;
        return u8::try_from(signed + 64).ok().filter(|v| *v <= 127).ok_or_else(error);
    }
    text.parse::<u8>().ok().filter(|v| *v <= 127).ok_or_else(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_displays_addresses() {
        assert_eq!(parse_address("A40"), Ok(ParamAddress::a(40)));
        assert_eq!(parse_address("b39"), Ok(ParamAddress::b(39)));
        assert_eq!(parse_address("B127"), Ok(ParamAddress::b(127)));
        assert!(parse_address("A128").is_err());
        assert!(parse_address("C93").is_err());
        assert!(parse_address("40").is_err());
        assert_eq!(ParamAddress::b(65).to_string(), "B65");
        assert_eq!(ParamAddress::b(65).offset(), 193);
    }

    #[test]
    fn parses_raw_and_bipolar_values() {
        assert_eq!(parse_value("64"), Ok(64));
        assert_eq!(parse_value("+10"), Ok(74));
        assert_eq!(parse_value("-64"), Ok(0));
        assert_eq!(parse_value("+63"), Ok(127));
        assert!(parse_value("+64").is_err());
        assert!(parse_value("128").is_err());
    }

    #[test]
    fn names_values_and_warnings_come_from_the_table() {
        assert_eq!(ParamAddress::a(40).name(), "Cutoff");
        assert_eq!(ParamAddress::a(40).edit_warning(), None);
        assert!(ParamAddress::a(0).edit_warning().is_some());
        assert!(ParamAddress::a(1).edit_warning().is_some());
        assert!(ParamAddress::a(92).edit_warning().is_some());
        assert_eq!(ParamAddress::a(10).describe_value(74), "74 (+10)");
        assert_eq!(ParamAddress::a(40).describe_value(95), "95");
    }

    #[test]
    fn set_takes_page_c_but_probes_do_not() {
        assert_eq!(parse_set_address("c32"), Ok(ParamAddress::c(32)));
        assert_eq!(parse_set_address("A40"), Ok(ParamAddress::a(40)));
        assert!(parse_set_address("C128").is_err());
        assert!(parse_address("C32").unwrap_err().contains("only `set`"));
        assert_eq!(ParamAddress::c(32).to_string(), "C32");
        assert!(!ParamAddress::c(32).in_single_dump());
        assert!(ParamAddress::b(3).in_single_dump());
    }

    #[test]
    fn page_c_names_and_guards() {
        assert_eq!(ParamAddress::c(32).name(), "Part Bank Change");
        assert_eq!(ParamAddress::c(33).name(), "Part Program Change");
        assert_eq!(ParamAddress::c(32).edit_warning(), None);
        assert_eq!(ParamAddress::c(39).edit_warning(), None);
        for disruptive in [93, 122, 123, 124, 98, 99] {
            let warning = ParamAddress::c(disruptive).edit_warning();
            assert!(warning.is_some_and(|w| w.contains("global")), "C{disruptive}: {warning:?}");
        }
        assert_eq!(ParamAddress::c(60).name(), "(undocumented)");
        assert!(ParamAddress::c(60).edit_warning().unwrap().contains("undocumented"));
        assert_eq!(ParamAddress::c(32).describe_value(1), "1");
    }
}

//! System Exclusive message parsing, framing, and command dispatch for the Access Virus.
//!
//! Verified against official Access Virus Rack Manual (OSR1_Manual_International.pdf, Chapter 20).

use crate::checksum::calculate_checksum;
use std::fmt;

/// Standard MIDI System Exclusive Start byte.
pub const MIDI_SYSEX_START: u8 = 0xF0;
/// Standard MIDI System Exclusive End byte.
pub const MIDI_SYSEX_END: u8 = 0xF7;

/// Access Music Electronics MIDI Manufacturer ID (3 bytes: 0x00, 0x20, 0x33).
pub const ACCESS_MANUFACTURER_ID: [u8; 3] = [0x00, 0x20, 0x33];

/// Product ID for Access Virus A, B, C, Rack, Classic.
pub const PRODUCT_ID_VIRUS: u8 = 0x01;

/// Default Omni Device ID in Virus SysEx (0x10 = Omni, 0x00..0x0F = individual devices 1..16).
pub const DEVICE_ID_OMNI: u8 = 0x10;

/// Part number for Single Edit Buffer in parameter change & dump requests.
pub const PART_SINGLE_BUFFER: u8 = 0x40;

/// Virus SysEx command identifiers as defined in the official manual.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Command {
    /// Single Program dump (256 bytes patch payload).
    SingleProgramDump = 0x10,
    /// Multi Program dump (256 multi bytes payload).
    MultiProgramDump = 0x11,
    /// Request a single program dump from synth.
    SingleProgramRequest = 0x30,
    /// Request a multi program dump from synth.
    MultiProgramRequest = 0x31,
    /// Request a single bank dump (Banks A..D).
    SingleBankRequest = 0x32,
    /// Request a multi bank dump.
    MultiBankRequest = 0x33,
    /// Request an arrangement dump.
    ArrangementRequest = 0x34,
    /// Request global settings.
    GlobalRequest = 0x35,
    /// Total RAM Dump request (Banks A+B, Multis, Globals).
    TotalRequest = 0x36,
    /// Controller Dump Request.
    ControllerDumpRequest = 0x37,
    /// Parameter Change for Page A (0x70).
    ParamChangePageA = 0x70,
    /// Parameter Change for Page B (0x71).
    ParamChangePageB = 0x71,
    /// Parameter Change for Page C: Multi / Global (0x72).
    ParamChangePageC = 0x72,
}

impl Command {
    pub fn from_raw(byte: u8) -> Option<Self> {
        match byte {
            0x10 => Some(Self::SingleProgramDump),
            0x11 => Some(Self::MultiProgramDump),
            0x30 => Some(Self::SingleProgramRequest),
            0x31 => Some(Self::MultiProgramRequest),
            0x32 => Some(Self::SingleBankRequest),
            0x33 => Some(Self::MultiBankRequest),
            0x34 => Some(Self::ArrangementRequest),
            0x35 => Some(Self::GlobalRequest),
            0x36 => Some(Self::TotalRequest),
            0x37 => Some(Self::ControllerDumpRequest),
            0x70 => Some(Self::ParamChangePageA),
            0x71 => Some(Self::ParamChangePageB),
            0x72 => Some(Self::ParamChangePageC),
            _ => None,
        }
    }
}

/// Parameter page address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    /// Page A (Sound parameters 0..127, SysEx command 0x70 / MIDI CC).
    PageA,
    /// Page B (Extended sound / Effects / Matrix parameters 0..127, SysEx command 0x71 / MIDI PolyPressure).
    PageB,
    /// Page C (Multi and Global parameters, SysEx command 0x72).
    PageC,
}

impl Page {
    pub fn to_command(self) -> Command {
        match self {
            Self::PageA => Command::ParamChangePageA,
            Self::PageB => Command::ParamChangePageB,
            Self::PageC => Command::ParamChangePageC,
        }
    }

    pub fn from_command(cmd: Command) -> Option<Self> {
        match cmd {
            Command::ParamChangePageA => Some(Self::PageA),
            Command::ParamChangePageB => Some(Self::PageB),
            Command::ParamChangePageC => Some(Self::PageC),
            _ => None,
        }
    }
}

/// Errors encountered while validating or parsing Virus SysEx messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyxError {
    InvalidFraming,
    NotAccessManufacturer,
    UnsupportedProduct(u8),
    UnknownCommand(u8),
    MessageTooShort,
    ChecksumMismatch { calculated: u8, expected: u8 },
    InvalidPayloadLength { expected: usize, found: usize },
    InvalidPart(u8),
}

impl fmt::Display for SyxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFraming => write!(f, "Invalid SysEx framing (missing 0xF0 or 0xF7)"),
            Self::NotAccessManufacturer => write!(f, "Manufacturer ID does not match Access Music (00 20 33)"),
            Self::UnsupportedProduct(id) => write!(f, "Unsupported Product ID: 0x{:02X}", id),
            Self::UnknownCommand(cmd) => write!(f, "Unknown Virus Command byte: 0x{:02X}", cmd),
            Self::MessageTooShort => write!(f, "SysEx message is too short"),
            Self::ChecksumMismatch { calculated, expected } => {
                write!(f, "Checksum mismatch (calculated: 0x{:02X}, expected: 0x{:02X})", calculated, expected)
            }
            Self::InvalidPayloadLength { expected, found } => {
                write!(f, "Invalid payload length (expected {} bytes, got {})", expected, found)
            }
            Self::InvalidPart(part) => write!(f, "Invalid part address: 0x{:02X}", part),
        }
    }
}

impl std::error::Error for SyxError {}

/// Fully parsed and validated Access Virus System Exclusive message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SysExMessage {
    /// Single Program Dump containing Device ID, Bank, Program, and 256 data bytes.
    SingleProgramDump {
        device_id: u8,
        bank: u8,
        prog: u8,
        data: Box<[u8; 256]>,
    },
    /// Request synth to send a single program dump.
    SingleProgramRequest {
        device_id: u8,
        bank: u8,
        prog: u8,
    },
    /// Request synth to send an entire single bank.
    SingleBankRequest {
        device_id: u8,
        bank: u8,
    },
    /// Total RAM Dump Request.
    TotalRequest {
        device_id: u8,
    },
    /// Parameter change: affects Page A (0x70), Page B (0x71), or Page C (0x72).
    /// `part` is `0x40` (Single Edit Buffer) or `0x00..0x0F` (Multi Part 1..16).
    ParamChange {
        device_id: u8,
        page: Page,
        part: u8,
        param: u8,
        value: u8,
    },
}

/// Parses and validates a raw MIDI byte slice as an Access Virus SysEx message.
pub fn parse_sysex(msg: &[u8]) -> Result<SysExMessage, SyxError> {
    if msg.len() < 7 || msg[0] != MIDI_SYSEX_START || *msg.last().unwrap() != MIDI_SYSEX_END {
        return Err(SyxError::InvalidFraming);
    }

    if &msg[1..4] != ACCESS_MANUFACTURER_ID {
        return Err(SyxError::NotAccessManufacturer);
    }

    if msg[4] != PRODUCT_ID_VIRUS {
        return Err(SyxError::UnsupportedProduct(msg[4]));
    }

    let device_id = msg[5];
    let command_byte = msg[6];

    match Command::from_raw(command_byte) {
        Some(Command::SingleProgramDump) => {
            // Structure: F0 00 20 33 01 dd 10 bb ss [256 bytes] cs F7 = 267 bytes
            if msg.len() != 267 {
                return Err(SyxError::InvalidPayloadLength {
                    expected: 267,
                    found: msg.len(),
                });
            }

            let bank = msg[7];
            let prog = msg[8];
            let data_bytes = &msg[9..265];
            let expected_checksum = msg[265];

            // Official Manual Checksum Formula (p. 207):
            // Checksum is the sum (DeviceID + 10 + BankNumber + ProgramNumber + [256 single bytes]) AND 7F
            // Exactly covering msg[5..265]!
            let checksum_region = &msg[5..265];
            let calculated_checksum = calculate_checksum(checksum_region);

            if calculated_checksum != expected_checksum {
                return Err(SyxError::ChecksumMismatch {
                    calculated: calculated_checksum,
                    expected: expected_checksum,
                });
            }

            let mut arr = [0u8; 256];
            arr.copy_from_slice(data_bytes);

            Ok(SysExMessage::SingleProgramDump {
                device_id,
                bank,
                prog,
                data: Box::new(arr),
            })
        }
        Some(Command::SingleProgramRequest) => {
            // Structure: F0 00 20 33 01 dd 30 bb ss F7 = 10 bytes
            if msg.len() < 10 {
                return Err(SyxError::MessageTooShort);
            }
            let bank = msg[7];
            let prog = msg[8];
            Ok(SysExMessage::SingleProgramRequest { device_id, bank, prog })
        }
        Some(Command::SingleBankRequest) => {
            // Structure: F0 00 20 33 01 dd 32 bb F7 = 9 bytes
            if msg.len() < 9 {
                return Err(SyxError::MessageTooShort);
            }
            let bank = msg[7];
            Ok(SysExMessage::SingleBankRequest { device_id, bank })
        }
        Some(Command::TotalRequest) => {
            // Structure: F0 00 20 33 01 dd 36 F7 = 8 bytes
            if msg.len() < 8 {
                return Err(SyxError::MessageTooShort);
            }
            Ok(SysExMessage::TotalRequest { device_id })
        }
        Some(cmd @ (Command::ParamChangePageA | Command::ParamChangePageB | Command::ParamChangePageC)) => {
            // Structure: F0 00 20 33 01 dd 7x pp nn vv F7 = 11 bytes
            if msg.len() < 11 {
                return Err(SyxError::MessageTooShort);
            }
            let page = Page::from_command(cmd).unwrap();
            let part = msg[7];
            let param = msg[8];
            let value = msg[9];
            Ok(SysExMessage::ParamChange {
                device_id,
                page,
                part,
                param,
                value,
            })
        }
        _ => Err(SyxError::UnknownCommand(command_byte)),
    }
}

/// Encodes a live parameter change SysEx message.
///
/// * `page`: `PageA` (0x70), `PageB` (0x71), `PageC` (0x72)
/// * `part`: `0x40` (Single Edit Buffer) or `0x00..0x0F` (Multi Part 1..16)
/// * `param`: Parameter Number `0..127`
/// * `value`: Parameter Value `0..127`
pub fn encode_param_change(device_id: u8, page: Page, part: u8, param: u8, value: u8) -> Vec<u8> {
    vec![
        MIDI_SYSEX_START,
        ACCESS_MANUFACTURER_ID[0],
        ACCESS_MANUFACTURER_ID[1],
        ACCESS_MANUFACTURER_ID[2],
        PRODUCT_ID_VIRUS,
        device_id,
        page.to_command() as u8,
        part & 0x7F,
        param & 0x7F,
        value & 0x7F,
        MIDI_SYSEX_END,
    ]
}

/// Encodes a Single Program Dump Request (`0x30`).
pub fn encode_dump_request(device_id: u8, bank: u8, prog: u8) -> Vec<u8> {
    vec![
        MIDI_SYSEX_START,
        ACCESS_MANUFACTURER_ID[0],
        ACCESS_MANUFACTURER_ID[1],
        ACCESS_MANUFACTURER_ID[2],
        PRODUCT_ID_VIRUS,
        device_id,
        Command::SingleProgramRequest as u8,
        bank & 0x7F,
        prog & 0x7F,
        MIDI_SYSEX_END,
    ]
}

/// Encodes a Single Bank Dump Request (`0x32`).
pub fn encode_single_bank_request(device_id: u8, bank: u8) -> Vec<u8> {
    vec![
        MIDI_SYSEX_START,
        ACCESS_MANUFACTURER_ID[0],
        ACCESS_MANUFACTURER_ID[1],
        ACCESS_MANUFACTURER_ID[2],
        PRODUCT_ID_VIRUS,
        device_id,
        Command::SingleBankRequest as u8,
        bank & 0x7F,
        MIDI_SYSEX_END,
    ]
}

/// Encodes a Total RAM Dump Request (`0x36`).
pub fn encode_total_request(device_id: u8) -> Vec<u8> {
    vec![
        MIDI_SYSEX_START,
        ACCESS_MANUFACTURER_ID[0],
        ACCESS_MANUFACTURER_ID[1],
        ACCESS_MANUFACTURER_ID[2],
        PRODUCT_ID_VIRUS,
        device_id,
        Command::TotalRequest as u8,
        MIDI_SYSEX_END,
    ]
}

/// Encodes a complete 256-byte Single Program Dump SysEx message with calculated checksum.
///
/// Follows official manual formula:
/// `Checksum = sum(DeviceID + 0x10 + Bank + Prog + data[0..256]) & 0x7F`
pub fn encode_program_dump(device_id: u8, bank: u8, prog: u8, data: &[u8; 256]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(267);
    msg.push(MIDI_SYSEX_START);
    msg.extend_from_slice(&ACCESS_MANUFACTURER_ID);
    msg.push(PRODUCT_ID_VIRUS);
    msg.push(device_id);
    msg.push(Command::SingleProgramDump as u8);

    msg.push(bank & 0x7F);
    msg.push(prog & 0x7F);
    msg.extend_from_slice(data);

    // Checksum covers bytes 5 to 264 (device_id, 0x10, bank, prog, data[256])
    let checksum = calculate_checksum(&msg[5..265]);
    msg.push(checksum);
    msg.push(MIDI_SYSEX_END);

    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manual_example_param_change() {
        // Manual Page 206 Example:
        // F0,00,20,33,01,10,70,05,28,5F,F7
        // Set Cutoff on Part 6 to decimal value 95.
        // 10: device ID omni; 70: page A, 05: part 6; 28: parameter Cutoff (40 dec), 5F: decimal value 95
        let manual_frame = [
            0xF0, 0x00, 0x20, 0x33, 0x01, 0x10, 0x70, 0x05, 0x28, 0x5F, 0xF7,
        ];

        let parsed = parse_sysex(&manual_frame).expect("Manual frame must parse");
        match parsed {
            SysExMessage::ParamChange { device_id, page, part, param, value } => {
                assert_eq!(device_id, 0x10);
                assert_eq!(page, Page::PageA);
                assert_eq!(part, 0x05);
                assert_eq!(param, 0x28);
                assert_eq!(value, 0x5F);
            }
            _ => panic!("Expected ParamChange"),
        }

        let encoded = encode_param_change(0x10, Page::PageA, 0x05, 0x28, 0x5F);
        assert_eq!(encoded, manual_frame);
    }

    #[test]
    fn test_manual_checksum_formula() {
        // DeviceID = 0x10, Command = 0x10, Bank = 0x01, Prog = 0x02, Data = 256 * 0x00
        // Sum = 16 + 16 + 1 + 2 + 0 = 35 (0x23)
        let dummy = [0u8; 256];
        let dump = encode_program_dump(0x10, 0x01, 0x02, &dummy);
        assert_eq!(dump[265], 35);

        let parsed = parse_sysex(&dump).expect("Should parse with correct checksum");
        match parsed {
            SysExMessage::SingleProgramDump { device_id, bank, prog, data } => {
                assert_eq!(device_id, 0x10);
                assert_eq!(bank, 0x01);
                assert_eq!(prog, 0x02);
                assert_eq!(*data, dummy);
            }
            _ => panic!("Expected SingleProgramDump"),
        }
    }
}

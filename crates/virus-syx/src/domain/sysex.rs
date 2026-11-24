//! System Exclusive framing, parsing and encoding for the Access Virus (manual pp. 204–209).

use crate::checksum::calculate_checksum;
use std::fmt;

/// Standard MIDI System Exclusive Start byte.
pub const MIDI_SYSEX_START: u8 = 0xF0;
/// Standard MIDI System Exclusive End byte.
pub const MIDI_SYSEX_END: u8 = 0xF7;

/// Access Music Electronics MIDI Manufacturer ID.
pub const ACCESS_MANUFACTURER_ID: [u8; 3] = [0x00, 0x20, 0x33];

/// Product ID for Access Virus A, B, C, Rack, Classic.
pub const PRODUCT_ID_VIRUS: u8 = 0x01;

/// Omni device ID (`0x00..=0x0F` address devices 1..16).
pub const DEVICE_ID_OMNI: u8 = 0x10;

/// Part number of the Single-mode edit buffer.
pub const PART_SINGLE_BUFFER: u8 = 0x40;

/// Length of the data block in single and multi dumps.
pub const DUMP_DATA_LEN: usize = 256;

/// Total length of a single or multi dump frame: `F0 00 20 33 01 dd cmd bb ss [256] cs F7`.
pub const DUMP_FRAME_LEN: usize = 7 + 2 + DUMP_DATA_LEN + 2;

/// Header length up to and including the command byte: `F0 00 20 33 01 dd cmd`.
const HEADER_LEN: usize = 7;

/// Page C parameter numbers for bank/program change via SysEx (SPEC §3.5).
pub const PAGE_C_PART_BANK_SELECT: u8 = 31;
pub const PAGE_C_PART_BANK_CHANGE: u8 = 32;
pub const PAGE_C_PART_PROGRAM_CHANGE: u8 = 33;
pub const PAGE_C_MULTI_PROGRAM_CHANGE: u8 = 105;

/// Documented Virus SysEx command bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Command {
    SingleProgramDump = 0x10,
    MultiProgramDump = 0x11,
    SingleProgramRequest = 0x30,
    MultiProgramRequest = 0x31,
    SingleBankRequest = 0x32,
    MultiBankRequest = 0x33,
    ArrangementRequest = 0x34,
    GlobalRequest = 0x35,
    TotalRequest = 0x36,
    ControllerDumpRequest = 0x37,
    ParamChangePageA = 0x70,
    ParamChangePageB = 0x71,
    ParamChangePageC = 0x72,
}

impl Command {
    pub fn from_raw(byte: u8) -> Option<Self> {
        Some(match byte {
            0x10 => Self::SingleProgramDump,
            0x11 => Self::MultiProgramDump,
            0x30 => Self::SingleProgramRequest,
            0x31 => Self::MultiProgramRequest,
            0x32 => Self::SingleBankRequest,
            0x33 => Self::MultiBankRequest,
            0x34 => Self::ArrangementRequest,
            0x35 => Self::GlobalRequest,
            0x36 => Self::TotalRequest,
            0x37 => Self::ControllerDumpRequest,
            0x70 => Self::ParamChangePageA,
            0x71 => Self::ParamChangePageB,
            0x72 => Self::ParamChangePageC,
            _ => return None,
        })
    }

    /// Total frame length for commands with a fixed size.
    fn frame_len(self) -> usize {
        match self {
            Self::SingleProgramDump | Self::MultiProgramDump => DUMP_FRAME_LEN,
            Self::SingleProgramRequest | Self::MultiProgramRequest => 10,
            Self::SingleBankRequest | Self::MultiBankRequest => 9,
            Self::ArrangementRequest | Self::GlobalRequest | Self::TotalRequest => 8,
            Self::ControllerDumpRequest => 10,
            Self::ParamChangePageA | Self::ParamChangePageB | Self::ParamChangePageC => 11,
        }
    }
}

/// Parameter page address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Page {
    /// Page A: sound parameters 0..127 (SysEx `0x70`, MIDI CC); dump bytes 0..127.
    PageA,
    /// Page B: sound parameters 0..127 (SysEx `0x71`, Poly Pressure); dump bytes 128..255.
    PageB,
    /// Page C: Multi and Global parameters (SysEx `0x72` only).
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

    /// Offset of this page's parameter 0 in a single dump; `None` for Page C.
    pub fn dump_offset(self) -> Option<usize> {
        match self {
            Self::PageA => Some(0),
            Self::PageB => Some(128),
            Self::PageC => None,
        }
    }
}

/// Errors encountered while validating or parsing Virus SysEx messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyxError {
    /// Missing `F0` / `F7`, or shorter than a minimal Virus header.
    InvalidFraming,
    NotAccessManufacturer,
    UnsupportedProduct(u8),
    /// Kept for API compatibility; `parse_sysex` reports unknown commands as `SysExMessage::Other`.
    UnknownCommand(u8),
    MessageTooShort,
    /// Kept for API compatibility; `parse_sysex` reports checksum status via `checksum_ok`.
    ChecksumMismatch { calculated: u8, expected: u8 },
    /// A message with a known fixed size had the wrong length.
    InvalidPayloadLength { expected: usize, found: usize },
    InvalidPart(u8),
    /// A valid message that is not a single dump was given where one was required.
    NotASingleDump { command: u8 },
    /// Single dump bank byte outside `00..=04` (banks E–H addressing is undocumented).
    UnsupportedBank(u8),
    /// A byte between `F0` and `F7` is 0x80 or higher, which MIDI forbids inside SysEx.
    InvalidDataByte { offset: usize, value: u8 },
    /// A valid message that is not a multi dump was given where one was required.
    NotAMultiDump { command: u8 },
    /// Multi dump bank byte other than `00` (edit buffer) or `01` (multi bank).
    UnsupportedMultiBank(u8),
}

impl fmt::Display for SyxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFraming => write!(f, "Invalid SysEx framing (missing 0xF0 or 0xF7)"),
            Self::NotAccessManufacturer => {
                write!(f, "Manufacturer ID does not match Access Music (00 20 33)")
            }
            Self::UnsupportedProduct(id) => write!(f, "Unsupported Product ID: 0x{id:02X}"),
            Self::UnknownCommand(cmd) => write!(f, "Unknown Virus Command byte: 0x{cmd:02X}"),
            Self::MessageTooShort => write!(f, "SysEx message is too short"),
            Self::ChecksumMismatch { calculated, expected } => write!(
                f,
                "Checksum mismatch (calculated: 0x{calculated:02X}, expected: 0x{expected:02X})"
            ),
            Self::InvalidPayloadLength { expected, found } => {
                write!(f, "Invalid message length (expected {expected} bytes, got {found})")
            }
            Self::InvalidPart(part) => write!(f, "Invalid part address: 0x{part:02X}"),
            Self::NotASingleDump { command } => {
                write!(f, "Not a single dump (command 0x{command:02X})")
            }
            Self::UnsupportedBank(bank) => write!(f, "Unsupported single bank number: 0x{bank:02X}"),
            Self::InvalidDataByte { offset, value } => {
                write!(f, "Invalid SysEx data byte 0x{value:02X} at offset {offset} (must be below 0x80)")
            }
            Self::NotAMultiDump { command } => {
                write!(f, "Not a multi dump (command 0x{command:02X})")
            }
            Self::UnsupportedMultiBank(bank) => write!(f, "Unsupported multi bank number: 0x{bank:02X}"),
        }
    }
}

impl std::error::Error for SyxError {}

/// A parsed Access Virus System Exclusive message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SysExMessage {
    /// Single dump (`0x10`). `bank` 0 = edit buffer (then `program` is the part), 1..4 = banks A..D.
    SingleDump {
        device_id: u8,
        bank: u8,
        program: u8,
        data: Box<[u8; 256]>,
        checksum_ok: bool,
    },
    /// Multi dump (`0x11`). `bank` 0 = multi edit buffer, 1 = multi bank.
    MultiDump {
        device_id: u8,
        bank: u8,
        program: u8,
        data: Box<[u8; 256]>,
        checksum_ok: bool,
    },
    SingleRequest { device_id: u8, bank: u8, program: u8 },
    MultiRequest { device_id: u8, bank: u8, program: u8 },
    SingleBankRequest { device_id: u8, bank: u8 },
    MultiBankRequest { device_id: u8, bank: u8 },
    ArrangementRequest { device_id: u8 },
    GlobalRequest { device_id: u8 },
    TotalRequest { device_id: u8 },
    ControllerDumpRequest { device_id: u8, part: u8 },
    /// Parameter change on Page A (`0x70`), B (`0x71`) or C (`0x72`).
    /// `part` is `0x40` (Single buffer) or `0x00..=0x0F` (Multi part 1..16).
    ParamChange {
        device_id: u8,
        page: Page,
        part: u8,
        param: u8,
        value: u8,
    },
    /// Any other Virus message (e.g. the undocumented `0x12` in `multis.mid`).
    /// `payload` is everything between the command byte and the trailing `F7`.
    Other {
        device_id: u8,
        command: u8,
        payload: Vec<u8>,
    },
}

impl SysExMessage {
    pub fn device_id(&self) -> u8 {
        match self {
            Self::SingleDump { device_id, .. }
            | Self::MultiDump { device_id, .. }
            | Self::SingleRequest { device_id, .. }
            | Self::MultiRequest { device_id, .. }
            | Self::SingleBankRequest { device_id, .. }
            | Self::MultiBankRequest { device_id, .. }
            | Self::ArrangementRequest { device_id }
            | Self::GlobalRequest { device_id }
            | Self::TotalRequest { device_id }
            | Self::ControllerDumpRequest { device_id, .. }
            | Self::ParamChange { device_id, .. }
            | Self::Other { device_id, .. } => *device_id,
        }
    }
}

/// Parses a complete `F0 … F7` frame as an Access Virus SysEx message.
///
/// Errors are returned only for framing, manufacturer/product mismatches and wrong
/// lengths of known fixed-size messages. Dump checksums are reported via `checksum_ok`;
/// unknown commands become [`SysExMessage::Other`].
pub fn parse_sysex(msg: &[u8]) -> Result<SysExMessage, SyxError> {
    if msg.first() != Some(&MIDI_SYSEX_START) || msg.last() != Some(&MIDI_SYSEX_END) {
        return Err(SyxError::InvalidFraming);
    }
    if msg.len() < HEADER_LEN + 1 {
        return Err(SyxError::MessageTooShort);
    }
    if let Some(offset) = msg[1..msg.len() - 1].iter().position(|&b| b >= 0x80) {
        return Err(SyxError::InvalidDataByte { offset: offset + 1, value: msg[offset + 1] });
    }
    if msg[1..4] != ACCESS_MANUFACTURER_ID {
        return Err(SyxError::NotAccessManufacturer);
    }
    if msg[4] != PRODUCT_ID_VIRUS {
        return Err(SyxError::UnsupportedProduct(msg[4]));
    }

    let device_id = msg[5];
    let command_byte = msg[6];
    let Some(command) = Command::from_raw(command_byte) else {
        return Ok(SysExMessage::Other {
            device_id,
            command: command_byte,
            payload: msg[HEADER_LEN..msg.len() - 1].to_vec(),
        });
    };

    let expected = command.frame_len();
    if msg.len() != expected {
        return Err(SyxError::InvalidPayloadLength { expected, found: msg.len() });
    }

    Ok(match command {
        Command::SingleProgramDump | Command::MultiProgramDump => {
            let bank = msg[7];
            let program = msg[8];
            let mut data = Box::new([0u8; DUMP_DATA_LEN]);
            data.copy_from_slice(&msg[9..9 + DUMP_DATA_LEN]);
            let checksum_ok = calculate_checksum(&msg[5..9 + DUMP_DATA_LEN]) == msg[9 + DUMP_DATA_LEN];
            if command == Command::SingleProgramDump {
                SysExMessage::SingleDump { device_id, bank, program, data, checksum_ok }
            } else {
                SysExMessage::MultiDump { device_id, bank, program, data, checksum_ok }
            }
        }
        Command::SingleProgramRequest => {
            SysExMessage::SingleRequest { device_id, bank: msg[7], program: msg[8] }
        }
        Command::MultiProgramRequest => {
            SysExMessage::MultiRequest { device_id, bank: msg[7], program: msg[8] }
        }
        Command::SingleBankRequest => SysExMessage::SingleBankRequest { device_id, bank: msg[7] },
        Command::MultiBankRequest => SysExMessage::MultiBankRequest { device_id, bank: msg[7] },
        Command::ArrangementRequest => SysExMessage::ArrangementRequest { device_id },
        Command::GlobalRequest => SysExMessage::GlobalRequest { device_id },
        Command::TotalRequest => SysExMessage::TotalRequest { device_id },
        Command::ControllerDumpRequest => {
            SysExMessage::ControllerDumpRequest { device_id, part: msg[8] }
        }
        Command::ParamChangePageA | Command::ParamChangePageB | Command::ParamChangePageC => {
            SysExMessage::ParamChange {
                device_id,
                page: Page::from_command(command).expect("param change command"),
                part: msg[7],
                param: msg[8],
                value: msg[9],
            }
        }
    })
}

fn frame(device_id: u8, command: Command, args: &[u8]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(HEADER_LEN + args.len() + 1);
    msg.push(MIDI_SYSEX_START);
    msg.extend_from_slice(&ACCESS_MANUFACTURER_ID);
    msg.push(PRODUCT_ID_VIRUS);
    msg.push(device_id & 0x7F);
    msg.push(command as u8);
    msg.extend(args.iter().map(|b| b & 0x7F));
    msg.push(MIDI_SYSEX_END);
    msg
}

fn encode_dump(device_id: u8, command: Command, bank: u8, program: u8, data: &[u8; 256]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(DUMP_FRAME_LEN);
    msg.push(MIDI_SYSEX_START);
    msg.extend_from_slice(&ACCESS_MANUFACTURER_ID);
    msg.push(PRODUCT_ID_VIRUS);
    msg.push(device_id & 0x7F);
    msg.push(command as u8);
    msg.push(bank & 0x7F);
    msg.push(program & 0x7F);
    msg.extend_from_slice(data);
    let checksum = calculate_checksum(&msg[5..]);
    msg.push(checksum);
    msg.push(MIDI_SYSEX_END);
    msg
}

/// Encodes a parameter change (`7x pp nn vv`).
pub fn encode_param_change(device_id: u8, page: Page, part: u8, param: u8, value: u8) -> Vec<u8> {
    frame(device_id, page.to_command(), &[part, param, value])
}

/// Encodes a Single Request (`30 bb ss`).
pub fn encode_dump_request(device_id: u8, bank: u8, prog: u8) -> Vec<u8> {
    frame(device_id, Command::SingleProgramRequest, &[bank, prog])
}

/// Encodes a Multi Request (`31 bb mm`).
pub fn encode_multi_request(device_id: u8, bank: u8, prog: u8) -> Vec<u8> {
    frame(device_id, Command::MultiProgramRequest, &[bank, prog])
}

/// Encodes a Single Bank Request (`32 bb`, `bb` = 1..4 for banks A..D).
pub fn encode_single_bank_request(device_id: u8, bank: u8) -> Vec<u8> {
    frame(device_id, Command::SingleBankRequest, &[bank])
}

/// Encodes a Multi Bank Request (`33 01`).
pub fn encode_multi_bank_request(device_id: u8) -> Vec<u8> {
    frame(device_id, Command::MultiBankRequest, &[0x01])
}

/// Encodes an Arrangement Request (`34`).
pub fn encode_arrangement_request(device_id: u8) -> Vec<u8> {
    frame(device_id, Command::ArrangementRequest, &[])
}

/// Encodes a Global Request (`35`).
pub fn encode_global_request(device_id: u8) -> Vec<u8> {
    frame(device_id, Command::GlobalRequest, &[])
}

/// Encodes a Total Request (`36`).
pub fn encode_total_request(device_id: u8) -> Vec<u8> {
    frame(device_id, Command::TotalRequest, &[])
}

/// Encodes a Controller Dump Request (`37 00 ss`).
pub fn encode_controller_dump_request(device_id: u8, part: u8) -> Vec<u8> {
    frame(device_id, Command::ControllerDumpRequest, &[0x00, part])
}

/// C31 Part Bank Select: takes effect on the next program change.
pub fn encode_part_bank_select(device_id: u8, part: u8, value: u8) -> Vec<u8> {
    encode_param_change(device_id, Page::PageC, part, PAGE_C_PART_BANK_SELECT, value)
}

/// C32 Part Bank Change: immediate.
pub fn encode_part_bank_change(device_id: u8, part: u8, value: u8) -> Vec<u8> {
    encode_param_change(device_id, Page::PageC, part, PAGE_C_PART_BANK_CHANGE, value)
}

/// C33 Part Program Change.
pub fn encode_part_program_change(device_id: u8, part: u8, value: u8) -> Vec<u8> {
    encode_param_change(device_id, Page::PageC, part, PAGE_C_PART_PROGRAM_CHANGE, value)
}

/// C105 Multi Program Change (part byte `00`).
pub fn encode_multi_program_change(device_id: u8, value: u8) -> Vec<u8> {
    encode_param_change(device_id, Page::PageC, 0x00, PAGE_C_MULTI_PROGRAM_CHANGE, value)
}

/// Encodes a Single Dump (`10 bb ss [256] cs`) with a correct checksum.
/// The data bytes are copied verbatim.
pub fn encode_program_dump(device_id: u8, bank: u8, prog: u8, data: &[u8; 256]) -> Vec<u8> {
    encode_dump(device_id, Command::SingleProgramDump, bank, prog, data)
}

/// Encodes a Multi Dump (`11 bb mm [256] cs`) with a correct checksum.
pub fn encode_multi_dump(device_id: u8, bank: u8, prog: u8, data: &[u8; 256]) -> Vec<u8> {
    encode_dump(device_id, Command::MultiProgramDump, bank, prog, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manual_example_param_change() {
        // Manual p. 206: set Cutoff (A40) of part 6 to 95.
        let manual_frame = [0xF0, 0x00, 0x20, 0x33, 0x01, 0x10, 0x70, 0x05, 0x28, 0x5F, 0xF7];

        let parsed = parse_sysex(&manual_frame).expect("Manual frame must parse");
        assert_eq!(
            parsed,
            SysExMessage::ParamChange { device_id: 0x10, page: Page::PageA, part: 5, param: 40, value: 95 }
        );
        assert_eq!(encode_param_change(0x10, Page::PageA, 0x05, 0x28, 0x5F), manual_frame);
    }

    #[test]
    fn test_manual_checksum_formula() {
        // 0x10 + 0x10 + 0x01 + 0x02 + 0 = 35
        let dummy = [0u8; 256];
        let dump = encode_program_dump(0x10, 0x01, 0x02, &dummy);
        assert_eq!(dump[265], 35);

        match parse_sysex(&dump).unwrap() {
            SysExMessage::SingleDump { device_id, bank, program, data, checksum_ok } => {
                assert_eq!((device_id, bank, program), (0x10, 1, 2));
                assert_eq!(*data, dummy);
                assert!(checksum_ok);
            }
            other => panic!("Expected SingleDump, got {other:?}"),
        }
    }

    #[test]
    fn bad_checksum_is_reported_not_rejected() {
        let mut dump = encode_program_dump(0x10, 0x01, 0x02, &[1u8; 256]);
        dump[265] ^= 0x01;
        match parse_sysex(&dump).unwrap() {
            SysExMessage::SingleDump { checksum_ok, data, .. } => {
                assert!(!checksum_ok);
                assert_eq!(*data, [1u8; 256]);
            }
            other => panic!("Expected SingleDump, got {other:?}"),
        }
    }

    #[test]
    fn unknown_command_is_other() {
        let msg = [0xF0, 0x00, 0x20, 0x33, 0x01, 0x10, 0x12, 0x01, 0x05, 0x06, 0xF7];
        assert_eq!(
            parse_sysex(&msg).unwrap(),
            SysExMessage::Other { device_id: 0x10, command: 0x12, payload: vec![0x01, 0x05, 0x06] }
        );
    }

    #[test]
    fn fixed_size_messages_require_exact_length() {
        // F7 must sit right after the payload (extra byte before F7).
        let long = [0xF0, 0x00, 0x20, 0x33, 0x01, 0x10, 0x70, 0x05, 0x28, 0x5F, 0x00, 0xF7];
        assert!(matches!(parse_sysex(&long), Err(SyxError::InvalidPayloadLength { expected: 11, found: 12 })));
        let short = [0xF0, 0x00, 0x20, 0x33, 0x01, 0x10, 0x30, 0x01, 0xF7];
        assert!(matches!(parse_sysex(&short), Err(SyxError::InvalidPayloadLength { .. })));
        let mut dump = encode_program_dump(0x10, 1, 0, &[0; 256]);
        dump.remove(100);
        assert!(matches!(parse_sysex(&dump), Err(SyxError::InvalidPayloadLength { .. })));
    }

    #[test]
    fn eight_bit_data_is_rejected() {
        let mut dump = encode_program_dump(0x10, 1, 0, &[0; 256]);
        dump[100] = 0x85;
        assert_eq!(parse_sysex(&dump), Err(SyxError::InvalidDataByte { offset: 100, value: 0x85 }));
        let mut param = encode_param_change(0, Page::PageA, 0x40, 40, 1);
        param[9] = 0xFF;
        assert!(matches!(parse_sysex(&param), Err(SyxError::InvalidDataByte { value: 0xFF, .. })));
    }

    #[test]
    fn requests_round_trip_through_parser() {
        let cases = [
            (encode_multi_request(0, 1, 7), SysExMessage::MultiRequest { device_id: 0, bank: 1, program: 7 }),
            (encode_multi_bank_request(0), SysExMessage::MultiBankRequest { device_id: 0, bank: 1 }),
            (encode_arrangement_request(0), SysExMessage::ArrangementRequest { device_id: 0 }),
            (encode_global_request(0), SysExMessage::GlobalRequest { device_id: 0 }),
            (encode_controller_dump_request(0, 0x40), SysExMessage::ControllerDumpRequest { device_id: 0, part: 0x40 }),
            (
                encode_multi_program_change(0, 9),
                SysExMessage::ParamChange { device_id: 0, page: Page::PageC, part: 0, param: 105, value: 9 },
            ),
        ];
        for (encoded, expected) in cases {
            assert_eq!(parse_sysex(&encoded).unwrap(), expected);
        }
        assert_eq!(encode_part_bank_change(0x10, 3, 1)[6..10], [0x72, 0x03, 0x20, 0x01]);
    }
}

//! Outgoing SysEx messages for a [`Target`] (SPEC §3). Pure byte building; nothing is sent here.

use virus_syx::{
    encode_dump_request, encode_global_request, encode_param_change, encode_program_dump,
    encode_single_bank_request,
};

use crate::address::ParamAddress;
use crate::settings::Target;

/// Single request for the target part's edit buffer: `30 00 pp`.
pub fn fetch_request(target: &Target) -> Vec<u8> {
    encode_dump_request(target.device.0, 0x00, target.part.raw())
}

/// Single dump into the target part's edit buffer: `10 00 pp [256] cs`.
pub fn edit_buffer_dump(target: &Target, data: &[u8; 256]) -> Vec<u8> {
    encode_program_dump(target.device.0, 0x00, target.part.raw(), data)
}

/// Parameter change `7x pp nn vv` to the target part.
pub fn param_change(target: &Target, address: ParamAddress, value: u8) -> Vec<u8> {
    encode_param_change(target.device.0, address.page, target.part.raw(), address.number, value)
}

/// Single bank request `32 bb`.
pub fn bank_request(target: &Target, bank: u8) -> Vec<u8> {
    encode_single_bank_request(target.device.0, bank)
}

/// Global request `35` (answer format undocumented, F3).
pub fn global_request(target: &Target) -> Vec<u8> {
    encode_global_request(target.device.0)
}

/// One single dump `10 bb ss` per `(program, data)` pair, data copied verbatim and with a
/// correct checksum.
pub fn bank_dumps(target: &Target, bank: u8, programs: &[(u8, [u8; 256])]) -> Vec<Vec<u8>> {
    programs
        .iter()
        .map(|(program, data)| encode_program_dump(target.device.0, bank, *program, data))
        .collect()
}

/// All Notes Off (CC123) then All Sound Off (CC120) for one channel (0..=15), or for all 16.
pub fn panic_messages(channel: u8, all_channels: bool) -> Vec<Vec<u8>> {
    let channels = if all_channels { (0..16).collect() } else { vec![channel & 0x0F] };
    channels
        .into_iter()
        .flat_map(|ch| [virus_midi::notes::all_notes_off(ch), virus_midi::notes::all_sound_off(ch)])
        .collect()
}

/// Uppercase hex with spaces, e.g. `F0 00 20 33`.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

/// Parses one SysEx message typed as hex for `send-hex`: bytes as two-digit pairs, spaces and
/// commas optional (`F0 00 20 33 …` or `F0002033…`). It must start with `F0`, end with `F7`
/// and carry only 7-bit data bytes in between.
pub fn parse_hex_sysex(text: &str) -> Result<Vec<u8>, String> {
    let digits: String = text.chars().filter(|c| !c.is_whitespace() && *c != ',').collect();
    if digits.is_empty() {
        return Err("no bytes given".to_owned());
    }
    if !digits.len().is_multiple_of(2) {
        return Err(format!("odd number of hex digits ({}); write every byte as two digits, e.g. 0A", digits.len()));
    }
    let bytes = (0..digits.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&digits[i..i + 2], 16).map_err(|_| format!("'{}' is not a hex byte", &digits[i..i + 2])))
        .collect::<Result<Vec<u8>, String>>()?;
    if bytes.len() < 3 || bytes[0] != 0xF0 || bytes[bytes.len() - 1] != 0xF7 {
        return Err("a SysEx message must start with F0, end with F7 and hold at least one data byte".to_owned());
    }
    if let Some(i) = bytes[1..bytes.len() - 1].iter().position(|&b| b > 0x7F) {
        return Err(format!("byte {} is {:02X}: data bytes must be 00-7F", i + 2, bytes[i + 1]));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{DeviceId, Part};
    use virus_syx::{parse_sysex, SysExMessage};

    fn target() -> Target {
        Target { device: DeviceId(0x00), channel: 0, part: Part::Single }
    }

    #[test]
    fn panic_messages_cover_one_channel_or_all_sixteen() {
        assert_eq!(panic_messages(10, false), vec![vec![0xBA, 123, 0], vec![0xBA, 120, 0]]);
        let all = panic_messages(10, true);
        assert_eq!(all.len(), 32);
        assert_eq!(all[0], vec![0xB0, 123, 0]);
        assert_eq!(all[31], vec![0xBF, 120, 0]);
    }

    #[test]
    fn hex_sysex_parses_spaced_and_packed_forms() {
        let expected = vec![0xF0, 0x00, 0x20, 0x33, 0x01, 0x00, 0x72, 0x40, 0x20, 0x01, 0xF7];
        assert_eq!(parse_hex_sysex("F0 00 20 33 01 00 72 40 20 01 F7"), Ok(expected.clone()));
        assert_eq!(parse_hex_sysex("f0002033010072402001f7"), Ok(expected.clone()));
        assert_eq!(parse_hex_sysex(" F0,00,20,33,01,00,72,40,20,01,F7 "), Ok(expected));
    }

    #[test]
    fn hex_sysex_rejects_bad_framing_and_8_bit_data() {
        assert!(parse_hex_sysex("").unwrap_err().contains("no bytes"));
        assert!(parse_hex_sysex("F0 0 F7").unwrap_err().contains("odd number"));
        assert!(parse_hex_sysex("F0 GG F7").unwrap_err().contains("not a hex byte"));
        assert!(parse_hex_sysex("00 20 33 F7").unwrap_err().contains("start with F0"));
        assert!(parse_hex_sysex("F0 00 20 33").unwrap_err().contains("end with F7"));
        assert!(parse_hex_sysex("F0 F7").is_err(), "no data byte");
        let err = parse_hex_sysex("F0 00 80 F7").unwrap_err();
        assert!(err.contains("byte 3 is 80"), "{err}");
        assert!(parse_hex_sysex("F0 00 F7 01 F7").unwrap_err().contains("byte 3 is F7"));
    }

    #[test]
    fn requests_match_the_manual() {
        assert_eq!(hex(&fetch_request(&target())), "F0 00 20 33 01 00 30 00 40 F7");
        assert_eq!(hex(&bank_request(&target(), 1)), "F0 00 20 33 01 00 32 01 F7");
        assert_eq!(hex(&global_request(&target())), "F0 00 20 33 01 00 35 F7");
        let multi = Target { part: Part::Multi(5), device: DeviceId(0x10), ..target() };
        assert_eq!(hex(&param_change(&multi, ParamAddress::a(40), 95)), "F0 00 20 33 01 10 70 05 28 5F F7");
    }

    #[test]
    fn bank_dumps_are_addressed_per_program_with_exact_data() {
        let programs: Vec<(u8, [u8; 256])> = (0..128u8).map(|p| (p, [p; 256])).collect();
        let frames = bank_dumps(&target(), 2, &programs);
        assert_eq!(frames.len(), 128);
        for (n, frame) in frames.iter().enumerate() {
            match parse_sysex(frame).unwrap() {
                SysExMessage::SingleDump { bank, program, data, checksum_ok, device_id } => {
                    assert_eq!((device_id, bank, program as usize), (0, 2, n));
                    assert_eq!(*data, programs[n].1);
                    assert!(checksum_ok);
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
}

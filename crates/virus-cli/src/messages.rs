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

/// Uppercase hex with spaces, e.g. `F0 00 20 33`.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
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

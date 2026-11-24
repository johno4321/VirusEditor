//! Read-only accessors on raw 256-byte multis and Multi Dump framing (SPEC §3.3, §4.4).
//!
//! Like [`crate::patch_data`], kept local so the librarian never re-encodes multi bytes.

/// Length of a multi program.
pub const MULTI_LEN: usize = 256;
/// Multi name, bytes 4..=13.
pub const MULTI_NAME_OFFSET: usize = 4;
pub const MULTI_NAME_LEN: usize = 10;
/// Multi dump `bb`: the Multi edit buffer.
pub const MULTI_EDIT_BUFFER_BANK: u8 = 0x00;
/// Multi dump `bb`: the Multi bank.
pub const MULTI_BANK_BYTE: u8 = 0x01;
/// Length of an encoded Multi Dump frame.
pub const MULTI_DUMP_LEN: usize = 267;

/// Decodes the 10-character multi name (non-printable bytes as `?`, trailing spaces trimmed).
pub fn multi_name(data: &[u8; MULTI_LEN]) -> String {
    data[MULTI_NAME_OFFSET..MULTI_NAME_OFFSET + MULTI_NAME_LEN]
        .iter()
        .map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '?' })
        .collect::<String>()
        .trim_end_matches(' ')
        .to_string()
}

/// `cs = (dd + 11 + bb + mm + Σdata) & 0x7F`.
pub fn multi_dump_checksum(device_id: u8, bank: u8, program: u8, data: &[u8; MULTI_LEN]) -> u8 {
    let header = device_id as u32 + 0x11 + bank as u32 + program as u32;
    let sum = data.iter().fold(header, |acc, &b| acc + b as u32);
    (sum & 0x7F) as u8
}

/// Encodes `F0 00 20 33 01 dd 11 bb mm [256 bytes] cs F7`. Data bytes are copied verbatim.
pub fn encode_multi_dump(device_id: u8, bank: u8, program: u8, data: &[u8; MULTI_LEN]) -> Vec<u8> {
    let (dd, bb, mm) = (device_id & 0x7F, bank & 0x7F, program & 0x7F);
    let mut frame = Vec::with_capacity(MULTI_DUMP_LEN);
    frame.extend_from_slice(&[0xF0, 0x00, 0x20, 0x33, 0x01, dd, 0x11, bb, mm]);
    frame.extend_from_slice(data);
    frame.push(multi_dump_checksum(dd, bb, mm, data));
    frame.push(0xF7);
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_and_frame_layout() {
        let mut data = [0x40u8; MULTI_LEN];
        data[MULTI_NAME_OFFSET..MULTI_NAME_OFFSET + MULTI_NAME_LEN].copy_from_slice(b"Seq\x01      ");
        assert_eq!(multi_name(&data), "Seq?");

        let frame = encode_multi_dump(0x10, 1, 5, &data);
        assert_eq!(frame.len(), MULTI_DUMP_LEN);
        assert_eq!(&frame[..9], &[0xF0, 0x00, 0x20, 0x33, 0x01, 0x10, 0x11, 0x01, 0x05]);
        assert_eq!(&frame[9..265], &data[..]);
        let expected = (frame[5..265].iter().map(|&b| b as u32).sum::<u32>() & 0x7F) as u8;
        assert_eq!((frame[265], frame[266]), (expected, 0xF7));
    }
}

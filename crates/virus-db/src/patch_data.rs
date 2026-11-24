//! Minimal, read-only accessors on raw 256-byte single patches and Single Dump framing.
//!
//! Kept local (instead of using `virus_syx::Patch`) so the librarian never re-encodes or
//! normalises patch bytes. These helpers can later be swapped for `virus-syx` accessors.

use crate::models::SoundCategory;

/// Length of a single patch (Page A + Page B).
pub const PATCH_LEN: usize = 256;
/// Patch name, Page B 112..=121.
pub const NAME_OFFSET: usize = 240;
pub const NAME_LEN: usize = 10;
/// Category 1 / 2, Page B 123 / 124.
pub const CATEGORY1_OFFSET: usize = 251;
pub const CATEGORY2_OFFSET: usize = 252;
/// Length of an encoded Single Dump frame.
pub const SINGLE_DUMP_LEN: usize = 267;
/// Single dump `bb` value addressing the edit buffer.
pub const EDIT_BUFFER_BANK: u8 = 0x00;

/// Decodes the 10-character patch name. Non-printable bytes are shown as `?` and
/// trailing padding spaces are trimmed (inner spaces are kept).
pub fn patch_name(data: &[u8; PATCH_LEN]) -> String {
    data[NAME_OFFSET..NAME_OFFSET + NAME_LEN]
        .iter()
        .map(|&b| {
            if (0x20..0x7F).contains(&b) {
                b as char
            } else {
                '?'
            }
        })
        .collect::<String>()
        .trim_end_matches(' ')
        .to_string()
}

/// Decodes Category 1 and Category 2.
pub fn patch_categories(data: &[u8; PATCH_LEN]) -> (SoundCategory, SoundCategory) {
    (
        SoundCategory::from_raw(data[CATEGORY1_OFFSET]),
        SoundCategory::from_raw(data[CATEGORY2_OFFSET]),
    )
}

/// SysEx single dump bank byte (1..=4) for a bank letter A..D.
pub fn bank_byte_for_letter(letter: &str) -> Option<u8> {
    match letter {
        "A" => Some(1),
        "B" => Some(2),
        "C" => Some(3),
        "D" => Some(4),
        _ => None,
    }
}

/// Bank letter for a SysEx single dump bank byte (1..=4).
pub fn letter_for_bank_byte(bank: u8) -> Option<&'static str> {
    match bank {
        1 => Some("A"),
        2 => Some("B"),
        3 => Some("C"),
        4 => Some("D"),
        _ => None,
    }
}

/// `cs = (dd + 10 + bb + ss + Σdata) & 0x7F` (SPEC §3.2).
pub fn single_dump_checksum(device_id: u8, bank: u8, program: u8, data: &[u8; PATCH_LEN]) -> u8 {
    let header = device_id as u32 + 0x10 + bank as u32 + program as u32;
    let sum = data.iter().fold(header, |acc, &b| acc + b as u32);
    (sum & 0x7F) as u8
}

/// Encodes `F0 00 20 33 01 dd 10 bb ss [256 bytes] cs F7`. Data bytes are copied verbatim.
pub fn encode_single_dump(device_id: u8, bank: u8, program: u8, data: &[u8; PATCH_LEN]) -> Vec<u8> {
    let (dd, bb, ss) = (device_id & 0x7F, bank & 0x7F, program & 0x7F);
    let mut frame = Vec::with_capacity(SINGLE_DUMP_LEN);
    frame.extend_from_slice(&[0xF0, 0x00, 0x20, 0x33, 0x01, dd, 0x10, bb, ss]);
    frame.extend_from_slice(data);
    frame.push(single_dump_checksum(dd, bb, ss, data));
    frame.push(0xF7);
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_trimmed_and_sanitised() {
        let mut data = [0u8; PATCH_LEN];
        data[NAME_OFFSET..NAME_OFFSET + NAME_LEN].copy_from_slice(b"Smoog   OK");
        assert_eq!(patch_name(&data), "Smoog   OK");
        data[NAME_OFFSET..NAME_OFFSET + NAME_LEN].copy_from_slice(b"Pad\x01      ");
        assert_eq!(patch_name(&data), "Pad?");
    }

    #[test]
    fn categories_preserve_unknown_values() {
        let mut data = [0u8; PATCH_LEN];
        data[CATEGORY1_OFFSET] = 5;
        data[CATEGORY2_OFFSET] = 127;
        assert_eq!(
            patch_categories(&data),
            (SoundCategory::Pluck, SoundCategory::Unknown(127))
        );
    }

    #[test]
    fn encoded_frame_layout() {
        let data = [0x40u8; PATCH_LEN];
        let frame = encode_single_dump(0x10, 1, 2, &data);
        assert_eq!(frame.len(), SINGLE_DUMP_LEN);
        assert_eq!(
            &frame[..9],
            &[0xF0, 0x00, 0x20, 0x33, 0x01, 0x10, 0x10, 0x01, 0x02]
        );
        assert_eq!(&frame[9..265], &data[..]);
        let expected = (frame[5..265].iter().map(|&b| b as u32).sum::<u32>() & 0x7F) as u8;
        assert_eq!(frame[265], expected);
        assert_eq!(frame[266], 0xF7);
    }
}

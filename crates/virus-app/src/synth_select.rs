//! "Select on synth" (F7): switch the synth to the stored program a library patch mirrors, with
//! SysEx Part Bank Change (C32) and Part Program Change (C33) (SPEC §3.5). Both are independent
//! of the MIDI channel and of the synth's program-change enables.

use virus_syx::{encode_part_bank_change, encode_part_program_change, part_bank_to_c32, SingleBank};

use crate::session::EditPart;

/// The SysEx to select `program` of library bank `bank_letter` on the edited part: bank
/// change first (immediate), then program change. Empty unless the library bank mirrors a
/// synth bank (letters A–D; user banks have no counterpart) and `program` is 0..=127.
pub fn select_messages(device_id: u8, part: EditPart, bank_letter: Option<&str>, program: u8) -> Vec<Vec<u8>> {
    let Some(bank) = bank_letter.and_then(synth_bank) else {
        return Vec::new();
    };
    if program > 127 {
        return Vec::new();
    }
    // C32 counts banks from PART_BANK_C32_BASE, unlike the dump byte `bb` (A = 1); the shared
    // helper is unverified on hardware (TODO F7).
    let value = part_bank_to_c32(bank.to_raw() - 1);
    vec![
        encode_part_bank_change(device_id, part.raw(), value),
        encode_part_program_change(device_id, part.raw(), program),
    ]
}

fn synth_bank(letter: &str) -> Option<SingleBank> {
    SingleBank::ALL.into_iter().find(|bank| bank.letter().to_string() == letter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use virus_syx::{parse_sysex, Page, SysExMessage};

    fn param(message: &[u8]) -> (Page, u8, u8, u8) {
        match parse_sysex(message).unwrap() {
            SysExMessage::ParamChange { page, part, param, value, .. } => (page, part, param, value),
            other => panic!("not a parameter change: {other:?}"),
        }
    }

    #[test]
    fn bank_change_then_program_change_for_the_single_part() {
        let messages = select_messages(0, EditPart::Single, Some("C"), 17);
        assert_eq!(messages.len(), 2);
        assert_eq!(param(&messages[0]), (Page::PageC, 0x40, 32, 2));
        assert_eq!(param(&messages[1]), (Page::PageC, 0x40, 33, 17));
        assert_eq!(messages[0][6..10], [0x72, 0x40, 0x20, 0x02]);
        assert_eq!(messages[1][6..10], [0x72, 0x40, 0x21, 0x11]);
    }

    #[test]
    fn multi_part_device_id_and_all_banks() {
        for (letter, value) in [("A", 0), ("B", 1), ("C", 2), ("D", 3)] {
            let messages = select_messages(0x0F, EditPart::Multi(5), Some(letter), 127);
            assert_eq!(param(&messages[0]), (Page::PageC, 5, 32, value));
            assert_eq!(param(&messages[1]), (Page::PageC, 5, 33, 127));
            assert_eq!(messages[0][5], 0x0F);
        }
    }

    #[test]
    fn bank_value_comes_from_the_shared_c32_helper() {
        for (index, letter) in ["A", "B", "C", "D"].into_iter().enumerate() {
            let messages = select_messages(0, EditPart::Single, Some(letter), 0);
            assert_eq!(param(&messages[0]).3, part_bank_to_c32(index as u8));
        }
    }

    #[test]
    fn user_banks_and_invalid_programs_send_nothing() {
        assert!(select_messages(0, EditPart::Single, None, 3).is_empty());
        assert!(select_messages(0, EditPart::Single, Some("E"), 3).is_empty());
        assert!(select_messages(0, EditPart::Single, Some("AB"), 3).is_empty());
        assert!(select_messages(0, EditPart::Single, Some("A"), 128).is_empty());
    }
}

//! Strict golden tests against the recovered, undamaged factory banks in `testdata/factory`
//! (see `testdata/factory/README.md`; the committed `.mid` originals are LF→CR damaged).

use virus_syx::{
    parse_sysex, read_sysex_file, Category, DumpTarget, Page, Patch, SingleBank, SysExMessage,
};

macro_rules! clean_bank {
    ($name:literal) => {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../testdata/factory/",
            $name
        )
    };
}

fn frames(path: &str, bank: u8) -> Vec<Vec<u8>> {
    let frames = read_sysex_file(path).expect("recovered bank readable");
    assert_eq!(frames.len(), 128);
    for (program, frame) in frames.iter().enumerate() {
        match parse_sysex(frame).expect("frame parses") {
            SysExMessage::SingleDump {
                bank: b,
                program: p,
                checksum_ok,
                ..
            } => {
                assert_eq!((b, p as usize), (bank, program));
                assert!(checksum_ok, "bank {bank} program {program}");
            }
            other => panic!("expected a single dump, got {other:?}"),
        }
    }
    frames
}

fn assert_byte_identical_round_trip(frames: &[Vec<u8>]) {
    for frame in frames {
        let (patch, target) = Patch::from_sysex(frame).expect("single dump");
        assert_eq!(&patch.to_sysex(0x10, target), frame);
    }
}

#[test]
fn bank_a_round_trips_byte_identically() {
    let frames = frames(clean_bank!("Bank_A.syx"), 1);
    assert_byte_identical_round_trip(&frames);

    // Program 10 (0x0A) is the one the damaged .mid file lost.
    let (patch, target) = Patch::from_sysex(&frames[10]).unwrap();
    assert_eq!(
        target,
        DumpTarget::Bank {
            bank: SingleBank::A,
            program: 10
        }
    );
    assert_eq!(patch.name(), "BASSEY  NH");
    assert_eq!(patch.sound_version(), 6);

    let (smoog, _) = Patch::from_sysex(&frames[1]).unwrap();
    assert_eq!(smoog.name(), "Smoog   OK");
    assert_eq!(smoog.category1(), Category::Bass);
}

#[test]
fn bank_b_round_trips_byte_identically() {
    let frames = frames(clean_bank!("Bank_B.syx"), 2);
    assert_byte_identical_round_trip(&frames);
}

#[test]
fn editing_one_parameter_changes_exactly_one_data_byte() {
    let frames = frames(clean_bank!("Bank_A.syx"), 1);
    for frame in &frames {
        let (mut patch, _) = Patch::from_sysex(frame).unwrap();
        let before = patch.to_bytes();
        let cutoff = patch.get(Page::PageA, 40);
        patch.set(Page::PageA, 40, (cutoff + 1) & 0x7F);
        let after = patch.to_bytes();
        let changed: Vec<usize> = (0..256).filter(|&i| before[i] != after[i]).collect();
        assert_eq!(changed, vec![40]);
    }
}

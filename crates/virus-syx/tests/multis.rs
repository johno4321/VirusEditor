//! Golden tests: the 128 factory multis and the Multi edit buffer in `multis.mid` (SPEC §4.4, §6).
//!
//! The file is LF→CR damaged (SPEC §6): some checksums are wrong and multi 10 reads as `mm = 13`,
//! so round trips compare the header and data bytes and only compare whole frames when the
//! stored checksum is valid.

use virus_syx::{
    parse_sysex, read_sysex_file, Multi, MultiDumpTarget, PartFlag, SysExMessage, MULTI_PARAMS,
};

const MULTIS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../VirusOSR200_INT/OSR200_INT/Goodies/Factory soundset/multis.mid"
);

struct Dump {
    frame: Vec<u8>,
    bank: u8,
    program: u8,
    checksum_ok: bool,
}

fn multi_dumps() -> Vec<Dump> {
    read_sysex_file(MULTIS)
        .expect("multis.mid readable")
        .into_iter()
        .filter_map(|frame| match parse_sysex(&frame).expect("frame parses") {
            SysExMessage::MultiDump { bank, program, checksum_ok, .. } => {
                Some(Dump { frame, bank, program, checksum_ok })
            }
            _ => None,
        })
        .collect()
}

fn bank(dumps: &[Dump]) -> Vec<(Multi, &Dump)> {
    dumps
        .iter()
        .filter(|d| d.bank == 1)
        .map(|d| (Multi::from_sysex(&d.frame).expect("multi dump").0, d))
        .collect()
}

#[test]
fn all_multis_round_trip_byte_identically() {
    let dumps = multi_dumps();
    assert_eq!(dumps.len(), 129);
    assert_eq!(dumps.iter().filter(|d| d.bank == 0).count(), 1);
    let programs: Vec<u8> = dumps.iter().filter(|d| d.bank == 1).map(|d| d.program).collect();
    let expected: Vec<u8> = (0..128u8).map(|i| if i == 0x0A { 0x0D } else { i }).collect();
    assert_eq!(programs, expected);

    let mut bad = 0;
    for dump in &dumps {
        let (multi, target) = Multi::from_sysex(&dump.frame).expect("multi dump");
        let exported = multi.to_sysex(dump.frame[5], target);
        assert_eq!(exported[..265], dump.frame[..265], "bb={} mm={}", dump.bank, dump.program);
        assert_eq!(exported[266], 0xF7);
        if dump.checksum_ok {
            assert_eq!(exported, dump.frame);
        } else {
            bad += 1;
            assert!(matches!(parse_sysex(&exported), Ok(SysExMessage::MultiDump { checksum_ok: true, .. })));
        }
    }
    assert_eq!(bad, 87);
}

#[test]
fn names_decode() {
    let dumps = multi_dumps();
    let edit = dumps.iter().find(|d| d.bank == 0).unwrap();
    let (edit_multi, target) = Multi::from_sysex(&edit.frame).unwrap();
    assert_eq!(target, MultiDumpTarget::EditBuffer);
    assert_eq!(edit_multi.name(), "Sequencer");

    let multis = bank(&dumps);
    let names: Vec<String> = multis.iter().map(|(m, _)| m.name()).collect();
    assert_eq!(names[..6], ["Sequencer", "DrumKit 01", "DrumKit 02", "Virus & RP", "Dream   RP", "Infect  RP"]);
    assert_eq!(names[41], "Romance RP");
    assert_eq!(names[126], "-MultiIni-");
    assert_eq!(names[127], " BAEUMI,-)");
    assert_eq!(names.iter().filter(|n| *n == "Multi").count(), 84);
}

#[test]
fn parts_decode() {
    let dumps = multi_dumps();
    let multis = bank(&dumps);

    let sequencer = &multis[0].0;
    assert_eq!(sequencer.clock_tempo_bpm(), 135);
    for part in 0..16 {
        let p = sequencer.part(part);
        assert_eq!((p.bank, p.low_key, p.high_key, p.volume, p.output), (0, 0, 127, 64, 1));
        // Part 11's program (and channel) 10 reads as 13 in the damaged file.
        assert_eq!(p.program, if part == 10 { 13 } else { part });
        assert_eq!(p.state, 0x47);
        assert!(p.flag(PartFlag::Enable) && p.flag(PartFlag::MidiVolume) && p.flag(PartFlag::HoldPedal));
        assert!(p.flag(PartFlag::ProgramChange) && !p.flag(PartFlag::PriorityHigh));
    }

    let drums = &multis[1].0;
    let lows: Vec<u8> = (0..16).map(|p| drums.part(p).low_key).collect();
    assert_eq!(lows, (36..52).collect::<Vec<u8>>());
    assert_eq!((drums.part(0).bank, drums.part(0).program, drums.part(15).high_key), (1, 70, 120));
    assert!((0..16).all(|p| drums.part_state(p) == 0x01));

    let baeumi = &multis[127].0;
    let outputs: Vec<u8> = (0..6).map(|p| baeumi.part(p).output).collect();
    assert_eq!(outputs, [1, 4, 7, 1, 4, 7]);
    // Bank 2 exceeds the documented 0..1 and must be kept.
    assert_eq!(baeumi.part(6).bank, 2);
    assert_eq!(baeumi.part(0).effect_send, 90);
}

#[test]
fn editing_every_parameter_back_and_forth_is_lossless() {
    let dumps = multi_dumps();
    for (multi, dump) in bank(&dumps) {
        for def in MULTI_PARAMS {
            for part in 0..16 {
                let mut edited = multi.clone();
                let original = edited.get(def, part);
                edited.set(def, part, original ^ 1);
                edited.set(def, part, original);
                assert_eq!(edited, multi, "{} part {part} of mm={}", def.name, dump.program);
            }
        }
    }
}

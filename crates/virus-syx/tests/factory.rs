//! Golden tests against the OS 2.00 factory soundset (SPEC §6).

use virus_syx::{
    calculate_checksum, extract_sysex_frames, parse_sysex, read_sysex_file, Category, DumpTarget,
    Patch, SingleBank, SysExMessage,
};

macro_rules! factory_file {
    ($name:literal) => {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../VirusOSR200_INT/OSR200_INT/Goodies/Factory soundset/",
            $name
        )
    };
}

struct Dump {
    frame: Vec<u8>,
    bank: u8,
    program: u8,
    checksum_ok: bool,
}

fn single_dumps(path: &str) -> Vec<Dump> {
    read_sysex_file(path)
        .expect("factory file readable")
        .into_iter()
        .filter_map(
            |frame| match parse_sysex(&frame).expect("factory frame parses") {
                SysExMessage::SingleDump {
                    bank,
                    program,
                    checksum_ok,
                    ..
                } => Some(Dump {
                    frame,
                    bank,
                    program,
                    checksum_ok,
                }),
                _ => None,
            },
        )
        .collect()
}

/// Asserts the bank layout and returns the dumps.
///
/// The committed factory `.mid` files are damaged by a line-ending conversion: they contain no
/// `0x0A` byte at all, every one was replaced by `0x0D`. Hence program 10 (`0x0A`) reads as 13,
/// SMF SysEx lengths are 3 too long (`82 0D` instead of `82 0A`), and dumps that had a `0x0A`
/// data byte fail the checksum by a multiple of 3. The affected programs are pinned here.
fn assert_bank(path: &str, bank: u8, bad_checksums: &[u8]) -> Vec<Dump> {
    let dumps = single_dumps(path);
    assert_eq!(dumps.len(), 128);
    for (i, dump) in dumps.iter().enumerate() {
        assert_eq!(dump.bank, bank);
        let expected = if i == 0x0A { 0x0D } else { i as u8 };
        assert_eq!(dump.program, expected);
    }
    let bad: Vec<u8> = dumps
        .iter()
        .filter(|d| !d.checksum_ok)
        .map(|d| d.program)
        .collect();
    assert_eq!(bad, bad_checksums);
    for dump in dumps.iter().filter(|d| !d.checksum_ok) {
        assert!(
            explained_by_lf_to_cr(&dump.frame),
            "program {}",
            dump.program
        );
    }
    dumps
}

/// True if turning some `0x0D` bytes (data or checksum) back into `0x0A` makes the checksum valid.
fn explained_by_lf_to_cr(frame: &[u8]) -> bool {
    let region = &frame[5..265];
    let calculated = calculate_checksum(region) as i32;
    let crs = region.iter().filter(|&&b| b == 0x0D).count() as i32;
    let stored = frame[265] as i32;
    let candidates: &[i32] = if stored == 0x0D {
        &[0x0D, 0x0A]
    } else {
        &[stored]
    };
    candidates
        .iter()
        .any(|&cs| (0..=crs).any(|k| (calculated - 3 * k).rem_euclid(128) == cs))
}

/// Import → export is byte-identical, except that a wrong stored checksum is replaced by the
/// correct one (the editor always sends correct checksums, SPEC §3.2).
fn assert_round_trip(dumps: &[Dump]) {
    for dump in dumps {
        let (patch, target) = Patch::from_sysex(&dump.frame).expect("single dump");
        let exported = patch.to_sysex(0x10, target);
        if dump.checksum_ok {
            assert_eq!(exported, dump.frame, "program {}", dump.program);
        } else {
            assert_eq!(
                exported[..265],
                dump.frame[..265],
                "program {}",
                dump.program
            );
            assert_eq!(exported[266..], dump.frame[266..]);
            assert!(matches!(
                parse_sysex(&exported),
                Ok(SysExMessage::SingleDump {
                    checksum_ok: true,
                    ..
                })
            ));
        }
    }
}

#[test]
fn bank_a_decodes_and_round_trips() {
    let dumps = assert_bank(
        factory_file!("OSR_FactoryPatches_Bank_A.mid"),
        1,
        &[
            3, 5, 13, 23, 26, 33, 35, 36, 42, 49, 53, 54, 62, 64, 66, 92, 94, 100, 116, 117,
        ],
    );
    let patches: Vec<(Patch, DumpTarget)> = dumps
        .iter()
        .map(|d| Patch::from_sysex(&d.frame).unwrap())
        .collect();

    assert!(patches.iter().all(|(p, _)| p.sound_version() == 6));
    let names: Vec<String> = patches[..3].iter().map(|(p, _)| p.name()).collect();
    assert_eq!(names, ["OvertureCK", "Smoog   OK", "JunoArp JS"]);
    assert_eq!(patches[1].0.category1(), Category::Bass);
    assert_eq!(patches[2].0.category1(), Category::Arpeggiator);
    assert_eq!(patches[2].0.category2(), Category::Acid);
    assert_eq!(
        patches[5].1,
        DumpTarget::Bank {
            bank: SingleBank::A,
            program: 5
        }
    );

    assert_round_trip(&dumps);
}

#[test]
fn bank_b_decodes_and_round_trips() {
    let dumps = assert_bank(
        factory_file!("OSR_FactoryPatches_Bank_B.mid"),
        2,
        &[
            2, 5, 6, 8, 13, 16, 17, 27, 30, 36, 64, 69, 70, 71, 72, 73, 74, 76, 77, 78, 79, 80, 84,
            85, 86, 87, 88, 89, 90, 92, 93, 115,
        ],
    );
    assert_round_trip(&dumps);
}

#[test]
fn total_dump_message_counts() {
    let bytes = std::fs::read(factory_file!("multis.mid")).unwrap();
    let mut singles = [0usize; 3];
    let mut multis = [0usize; 2];
    let mut other_0x12 = 0;
    let mut bad_checksums = 0;
    for frame in extract_sysex_frames(&bytes) {
        match parse_sysex(&frame).expect("frame parses") {
            SysExMessage::SingleDump {
                bank, checksum_ok, ..
            } => {
                singles[bank as usize] += 1;
                bad_checksums += usize::from(!checksum_ok);
            }
            SysExMessage::MultiDump {
                bank, checksum_ok, ..
            } => {
                multis[bank as usize] += 1;
                bad_checksums += usize::from(!checksum_ok);
            }
            SysExMessage::Other { command: 0x12, .. } => other_0x12 += 1,
            other => panic!("unexpected message {other:?}"),
        }
    }
    assert_eq!(singles, [33, 128, 128]);
    assert_eq!(multis, [1, 128]);
    assert_eq!(other_0x12, 1);
    assert_eq!(bad_checksums, 170);
}

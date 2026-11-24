use super::*;
use crate::patch_data::{
    single_dump_checksum, CATEGORY1_OFFSET, CATEGORY2_OFFSET, NAME_LEN, NAME_OFFSET,
    SINGLE_DUMP_LEN,
};

/// Patch with non-zero "undocumented" bytes everywhere, the given name and raw categories.
fn synthetic_patch(name: &str, cat1: u8, cat2: u8) -> [u8; PATCH_LEN] {
    let mut data = [0u8; PATCH_LEN];
    for (i, b) in data.iter_mut().enumerate() {
        *b = ((i * 37 + 11) % 128) as u8;
    }
    let mut padded = [b' '; NAME_LEN];
    padded[..name.len()].copy_from_slice(name.as_bytes());
    data[NAME_OFFSET..NAME_OFFSET + NAME_LEN].copy_from_slice(&padded);
    data[CATEGORY1_OFFSET] = cat1;
    data[CATEGORY2_OFFSET] = cat2;
    data
}

fn dump(bank: u8, program: u8, name: &str) -> IncomingDump {
    IncomingDump {
        bank,
        program,
        data: synthetic_patch(name, 1, 0),
        checksum_ok: true,
    }
}

fn bank_id(db: &PatchDatabase, letter: &str) -> i64 {
    db.get_banks()
        .unwrap()
        .into_iter()
        .find(|b| b.bank_letter.as_deref() == Some(letter))
        .unwrap()
        .id
}

fn patch_count(db: &PatchDatabase) -> i64 {
    db.conn
        .query_row("SELECT COUNT(*) FROM patches", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn fresh_db_is_v2_with_seeded_banks() {
    let db = PatchDatabase::open_in_memory().unwrap();
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    let banks = db.get_banks().unwrap();
    let summary: Vec<_> = banks
        .iter()
        .map(|b| (b.name.as_str(), b.bank_letter.as_deref(), b.is_rom))
        .collect();
    assert_eq!(
        summary,
        [
            ("Bank A", Some("A"), false),
            ("Bank B", Some("B"), false),
            ("Bank C", Some("C"), true),
            ("Bank D", Some("D"), true),
        ]
    );
    let user = db.create_bank("My Bank").unwrap();
    let user = db.get_bank_by_id(user).unwrap().unwrap();
    assert_eq!((user.bank_letter, user.is_rom), (None, false));
}

#[test]
fn save_and_load_is_byte_exact() {
    let db = PatchDatabase::open_in_memory().unwrap();
    let a = bank_id(&db, "A");
    let data = synthetic_patch("Smoog   OK", 2, 127);
    let meta = PatchMeta {
        tags: "warm".into(),
        favorite: true,
    };
    let id = db.save_patch(a, 5, &data, &meta).unwrap();

    let rec = db.get_patch_by_id(id).unwrap().unwrap();
    assert_eq!(rec.data, data);
    assert_eq!(rec.name, "Smoog   OK");
    assert_eq!(rec.category1, SoundCategory::Bass);
    assert_eq!(rec.category2, SoundCategory::Unknown(127));
    assert_eq!(
        (rec.tags.as_str(), rec.favorite, rec.checksum_ok),
        ("warm", true, true)
    );
    assert_eq!(db.get_patch_by_slot(a, 5).unwrap().unwrap(), rec);

    // Upsert into the same slot keeps the ID and replaces data and derived columns.
    let data2 = synthetic_patch("Other", 3, 0);
    assert_eq!(
        db.save_patch(a, 5, &data2, &PatchMeta::default()).unwrap(),
        id
    );
    let rec = db.get_patch_by_id(id).unwrap().unwrap();
    assert_eq!(
        (rec.data, rec.name.as_str(), rec.category1),
        (data2, "Other", SoundCategory::Pad)
    );

    assert!(matches!(
        db.save_patch(a, 128, &data, &meta),
        Err(DbError::InvalidProgram(128))
    ));
    assert!(matches!(
        db.save_patch(9999, 0, &data, &meta),
        Err(DbError::BankNotFound(9999))
    ));
}

#[test]
fn favourite_and_tags_do_not_touch_data() {
    let db = PatchDatabase::open_in_memory().unwrap();
    let a = bank_id(&db, "A");
    let data = synthetic_patch("Keep Me", 1, 6);
    let id = db.save_patch(a, 0, &data, &PatchMeta::default()).unwrap();

    assert!(db.set_favorite(id, true).unwrap());
    assert!(db.set_tags(id, "acid, squelch").unwrap());
    let rec = db.get_patch_by_id(id).unwrap().unwrap();
    assert_eq!(rec.data, data);
    assert_eq!(rec.name, "Keep Me");
    assert!(rec.favorite);
    assert_eq!(rec.tags, "acid, squelch");
    assert!(!db.set_favorite(id + 100, true).unwrap());
}

#[test]
fn bank_slots_have_128_entries_with_gaps() {
    let db = PatchDatabase::open_in_memory().unwrap();
    let b = bank_id(&db, "B");
    db.save_patch(b, 0, &synthetic_patch("First", 0, 0), &PatchMeta::default())
        .unwrap();
    db.save_patch(
        b,
        127,
        &synthetic_patch("Last", 0, 0),
        &PatchMeta::default(),
    )
    .unwrap();

    let slots = db.get_bank_slots(b).unwrap();
    assert_eq!(slots.len(), 128);
    assert_eq!(slots[0].as_ref().unwrap().name, "First");
    assert_eq!(slots[127].as_ref().unwrap().name, "Last");
    assert_eq!(slots.iter().filter(|s| s.is_none()).count(), 126);
    assert!(db
        .get_bank_slots(bank_id(&db, "C"))
        .unwrap()
        .iter()
        .all(Option::is_none));
}

#[test]
fn query_filters_and_like_escaping() {
    let db = PatchDatabase::open_in_memory().unwrap();
    let a = bank_id(&db, "A");
    let fav = PatchMeta {
        tags: "fat".into(),
        favorite: true,
    };
    db.save_patch(
        a,
        0,
        &synthetic_patch("100% Acid", 6, 2),
        &PatchMeta::default(),
    )
    .unwrap();
    db.save_patch(a, 1, &synthetic_patch("Moog_Bass", 2, 7), &fav)
        .unwrap();
    db.save_patch(
        a,
        2,
        &synthetic_patch("HyperPad", 3, 127),
        &PatchMeta::default(),
    )
    .unwrap();

    let names = |filter: PatchFilter| -> Vec<String> {
        db.query_patches(&filter)
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect()
    };
    let search = |text: &str| PatchFilter {
        search_text: Some(text.into()),
        ..Default::default()
    };

    assert_eq!(names(search("100%")), ["100% Acid"]);
    assert_eq!(names(search("%")), ["100% Acid"]);
    assert_eq!(names(search("_")), ["Moog_Bass"]);
    assert_eq!(names(search("moog")), ["Moog_Bass"]);
    assert_eq!(names(search("FAT")), ["Moog_Bass"]);
    assert_eq!(names(search(r"\")), Vec::<String>::new());
    assert_eq!(
        names(PatchFilter {
            category: Some(SoundCategory::Bass),
            ..Default::default()
        }),
        ["100% Acid", "Moog_Bass"]
    );
    assert_eq!(
        names(PatchFilter {
            category: Some(SoundCategory::Unknown(127)),
            ..Default::default()
        }),
        ["HyperPad"]
    );
    assert_eq!(
        names(PatchFilter {
            favorite_only: true,
            ..Default::default()
        }),
        ["Moog_Bass"]
    );
}

#[test]
fn import_original_banks_routes_by_bank_byte() {
    let db = PatchDatabase::open_in_memory().unwrap();
    let mut bad = dump(2, 7, "BadSum");
    bad.checksum_ok = false;
    let edit = dump(0, 0x40, "EditBuf");
    let dumps = vec![
        dump(1, 0, "InA"),
        dump(2, 5, "InB"),
        edit.clone(),
        dump(7, 1, "BankH"),
        bad,
    ];

    let report = db
        .import_single_dumps(&dumps, ImportTarget::OriginalBanks)
        .unwrap();
    assert_eq!(report.imported, 3);
    assert_eq!(
        report.per_bank,
        [("Bank A".to_string(), 1), ("Bank B".to_string(), 2)]
    );
    assert_eq!(report.loose, [edit]);
    assert_eq!((report.bad_checksum, report.skipped), (1, 1));

    let (a, b) = (bank_id(&db, "A"), bank_id(&db, "B"));
    assert_eq!(db.get_patch_by_slot(a, 0).unwrap().unwrap().name, "InA");
    assert_eq!(db.get_patch_by_slot(b, 5).unwrap().unwrap().name, "InB");
    assert!(!db.get_patch_by_slot(b, 7).unwrap().unwrap().checksum_ok);
    // The edit-buffer dump must not land in slot 0x40 (= 64) of any bank.
    assert_eq!(patch_count(&db), 3);
}

#[test]
fn import_recreates_missing_letter_bank() {
    let db = PatchDatabase::open_in_memory().unwrap();
    db.delete_bank(bank_id(&db, "D")).unwrap();
    let report = db
        .import_single_dumps(&[dump(4, 3, "RomSound")], ImportTarget::OriginalBanks)
        .unwrap();
    assert_eq!(report.per_bank, [("Bank D".to_string(), 1)]);
    let d = db.get_bank_by_id(bank_id(&db, "D")).unwrap().unwrap();
    assert!(d.is_rom);
}

#[test]
fn import_into_forced_bank() {
    let db = PatchDatabase::open_in_memory().unwrap();
    let user = db.create_bank("Mine").unwrap();

    let mixed = [dump(1, 0, "A0"), dump(2, 0, "B0")];
    assert!(matches!(
        db.import_single_dumps(&mixed, ImportTarget::Bank(user)),
        Err(DbError::MixedSourceBanks(ref b)) if b == &[1, 2]
    ));
    assert_eq!(patch_count(&db), 0);

    let single = [dump(2, 0, "B0"), dump(2, 9, "B9"), dump(0, 0x40, "Edit")];
    let report = db
        .import_single_dumps(&single, ImportTarget::Bank(user))
        .unwrap();
    assert_eq!(report.per_bank, [("Mine".to_string(), 2)]);
    assert_eq!(report.loose.len(), 1);
    assert_eq!(db.get_patch_by_slot(user, 9).unwrap().unwrap().name, "B9");
}

#[test]
fn import_keeps_metadata_only_for_same_name() {
    let db = PatchDatabase::open_in_memory().unwrap();
    let a = bank_id(&db, "A");
    let meta = PatchMeta {
        tags: "mine".into(),
        favorite: true,
    };
    db.save_patch(a, 0, &synthetic_patch("Same", 1, 0), &meta)
        .unwrap();
    db.save_patch(a, 1, &synthetic_patch("Old", 1, 0), &meta)
        .unwrap();

    let mut same = dump(1, 0, "Same");
    same.data[10] = 99;
    db.import_single_dumps(
        &[same.clone(), dump(1, 1, "New")],
        ImportTarget::OriginalBanks,
    )
    .unwrap();

    let kept = db.get_patch_by_slot(a, 0).unwrap().unwrap();
    assert_eq!(
        (kept.data, kept.tags.as_str(), kept.favorite),
        (same.data, "mine", true)
    );
    let reset = db.get_patch_by_slot(a, 1).unwrap().unwrap();
    assert_eq!(
        (reset.name.as_str(), reset.tags.as_str(), reset.favorite),
        ("New", "", false)
    );
}

#[test]
fn import_rolls_back_on_failure() {
    let db = PatchDatabase::open_in_memory().unwrap();
    db.conn
        .execute_batch(
            "CREATE TRIGGER fail_on_5 BEFORE INSERT ON patches WHEN NEW.program_number = 5
             BEGIN SELECT RAISE(ABORT, 'boom'); END;",
        )
        .unwrap();
    let before = db.data_version();
    let dumps: Vec<_> = (0..10).map(|p| dump(1, p, "X")).collect();
    assert!(matches!(
        db.import_single_dumps(&dumps, ImportTarget::OriginalBanks),
        Err(DbError::Sqlite(_))
    ));
    assert_eq!(patch_count(&db), 0);
    assert_eq!(db.data_version(), before);
}

#[test]
fn export_frames_use_bank_byte_and_exact_data() {
    let db = PatchDatabase::open_in_memory().unwrap();
    let a = bank_id(&db, "A");
    let p0 = synthetic_patch("Zero", 1, 127);
    let p9 = synthetic_patch("Nine", 2, 0);
    db.save_patch(a, 9, &p9, &PatchMeta::default()).unwrap();
    db.save_patch(a, 0, &p0, &PatchMeta::default()).unwrap();

    let frames = db.export_bank_dumps(a, 0x10, None).unwrap();
    assert_eq!(frames.len(), 2);
    for (frame, (prog, data)) in frames.iter().zip([(0u8, &p0), (9, &p9)]) {
        assert_eq!(frame.len(), SINGLE_DUMP_LEN);
        assert_eq!(
            &frame[..9],
            &[0xF0, 0x00, 0x20, 0x33, 0x01, 0x10, 0x10, 0x01, prog]
        );
        assert_eq!(&frame[9..265], &data[..]);
        assert_eq!(frame[265], single_dump_checksum(0x10, 1, prog, data));
        assert_eq!(frame[266], 0xF7);
    }
    assert_eq!(db.export_bank_syx(a, 0x10, None).unwrap(), frames.concat());

    let user = db.create_bank("Mine").unwrap();
    db.save_patch(user, 3, &p0, &PatchMeta::default()).unwrap();
    assert!(matches!(
        db.export_bank_dumps(user, 0, None),
        Err(DbError::NoBankLetter(_))
    ));
    assert!(matches!(
        db.export_bank_dumps(user, 0, Some(0)),
        Err(DbError::InvalidTargetBank(0))
    ));
    assert_eq!(db.export_bank_dumps(user, 0, Some(2)).unwrap()[0][7], 0x02);
    let d = bank_id(&db, "D");
    db.save_patch(d, 0, &p0, &PatchMeta::default()).unwrap();
    assert_eq!(db.export_bank_dumps(d, 0, None).unwrap()[0][7], 0x04);
}

#[test]
fn corrupt_blob_is_an_error() {
    let db = PatchDatabase::open_in_memory().unwrap();
    let a = bank_id(&db, "A");
    let id = db
        .save_patch(a, 0, &synthetic_patch("Ok", 0, 0), &PatchMeta::default())
        .unwrap();
    db.conn
        .execute(
            "UPDATE patches SET raw_data = zeroblob(100) WHERE id = ?1",
            params![id],
        )
        .unwrap();
    assert!(matches!(
        db.get_patch_by_id(id),
        Err(DbError::CorruptPatch { len: 100, .. })
    ));
    assert!(matches!(
        db.export_bank_dumps(a, 0, None),
        Err(DbError::CorruptPatch { len: 100, .. })
    ));
}

#[test]
fn data_version_changes_on_writes_only() {
    let db = PatchDatabase::open_in_memory().unwrap();
    let a = bank_id(&db, "A");
    let v0 = db.data_version();
    db.query_patches(&PatchFilter::default()).unwrap();
    assert_eq!(db.data_version(), v0);
    let id = db
        .save_patch(a, 0, &synthetic_patch("V", 0, 0), &PatchMeta::default())
        .unwrap();
    let v1 = db.data_version();
    assert_ne!(v1, v0);
    db.set_favorite(id, true).unwrap();
    assert_ne!(db.data_version(), v1);
}

#[test]
fn migrates_v1_database() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(crate::schema::SCHEMA_V1_SQL).unwrap();
    for (name, letter, factory) in [
        ("Bank A", "A", 1),
        ("Bank B", "B", 1),
        ("Bank C (User)", "C", 0),
        ("Bank D (User)", "D", 0),
    ] {
        conn.execute(
            "INSERT INTO banks (name, bank_letter, is_factory) VALUES (?1, ?2, ?3)",
            params![name, letter, factory],
        )
        .unwrap();
    }
    let data = synthetic_patch("JunoArp JS", 8, 127);
    conn.execute(
        "INSERT INTO patches (bank_id, program_number, name, category1, category2, raw_data,
                              checksum, tags, favorite)
         VALUES (1, 2, 'stale name', 'Lead', 'Off', ?1, 0, 'arp', 1)",
        params![&data[..]],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO patches (bank_id, program_number, name, category1, category2, raw_data,
                              checksum, tags, favorite)
         VALUES (2, 0, 'Broken', 'Pad', 'Off', zeroblob(10), 0, '', 0)",
        [],
    )
    .unwrap();

    let db = PatchDatabase::with_connection(conn, None).unwrap();
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);

    let banks = db.get_banks().unwrap();
    let flags: Vec<_> = banks.iter().map(|b| (b.name.as_str(), b.is_rom)).collect();
    assert_eq!(
        flags,
        [
            ("Bank A", false),
            ("Bank B", false),
            ("Bank C", true),
            ("Bank D", true)
        ]
    );

    let rec = db.get_patch_by_slot(1, 2).unwrap().unwrap();
    assert_eq!(rec.data, data);
    assert_eq!(rec.name, "JunoArp JS");
    assert_eq!(
        (rec.category1, rec.category2),
        (SoundCategory::Arpeggiator, SoundCategory::Unknown(127))
    );
    assert_eq!((rec.tags.as_str(), rec.favorite), ("arp", true));
    assert!(!rec.checksum_ok, "migrated v1 rows must not look byte-exact");
    assert!(matches!(
        db.get_patch_by_slot(2, 0),
        Err(DbError::CorruptPatch { len: 10, .. })
    ));

    // Indexes were recreated on the new table and the old table is gone.
    assert!(!db.table_exists("patches_v1").unwrap());
    // v1 → v2 → v3: the multis table exists too.
    assert!(db.table_exists("multis").unwrap());
    let idx: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND tbl_name = 'patches'
             AND name LIKE 'idx_patches_%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(idx, 4);
}

/// Test-only scanner: finds `00 20 33 01 dd 10 bb ss [256] cs F7` and returns the full
/// frames (with `F0`) alongside the parsed dumps.
fn scan_single_dumps(bytes: &[u8]) -> Vec<(Vec<u8>, IncomingDump)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + SINGLE_DUMP_LEN - 1 <= bytes.len() {
        let h = &bytes[i..];
        if h[..4] == [0x00, 0x20, 0x33, 0x01] && h[5] == 0x10 && h[SINGLE_DUMP_LEN - 2] == 0xF7 {
            let (dd, bb, ss) = (h[4], h[6], h[7]);
            let data: [u8; PATCH_LEN] = h[8..8 + PATCH_LEN].try_into().unwrap();
            let cs = h[8 + PATCH_LEN];
            let mut frame = vec![0xF0];
            frame.extend_from_slice(&h[..SINGLE_DUMP_LEN - 1]);
            let checksum_ok = single_dump_checksum(dd, bb, ss, &data) == cs;
            out.push((
                frame,
                IncomingDump {
                    bank: bb,
                    program: ss,
                    data,
                    checksum_ok,
                },
            ));
            i += SINGLE_DUMP_LEN - 1;
        } else {
            i += 1;
        }
    }
    out
}

#[test]
fn factory_bank_a_round_trip() {
    let file = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../VirusOSR200_INT/OSR200_INT/Goodies/Factory soundset/OSR_FactoryPatches_Bank_A.mid"
    ))
    .unwrap();
    let scanned = scan_single_dumps(&file);
    assert_eq!(scanned.len(), 128);
    let dumps: Vec<_> = scanned.iter().map(|(_, d)| d.clone()).collect();
    let bad = dumps.iter().filter(|d| !d.checksum_ok).count();
    assert!(dumps.iter().all(|d| d.bank == 1));

    let db = PatchDatabase::open_in_memory().unwrap();
    let report = db
        .import_single_dumps(&dumps, ImportTarget::OriginalBanks)
        .unwrap();
    // Program 10 is stored as 13 in this damaged file, so 127 distinct slots are written.
    assert_eq!(report.imported, 127);
    assert_eq!(report.per_bank, [("Bank A".to_string(), 127)]);
    assert_eq!(report.duplicates.len(), 1);
    assert_eq!(report.duplicates[0].program, 13);
    assert_eq!(
        (report.bad_checksum, report.skipped, report.loose.len()),
        (bad, 0, 0)
    );
    let plan = PatchDatabase::open_in_memory()
        .unwrap()
        .plan_import(&dumps, ImportTarget::OriginalBanks)
        .unwrap();
    assert_eq!((plan.slots, plan.duplicates.len(), plan.overwrites.len()), (127, 1, 0));

    let a = bank_id(&db, "A");
    let slots = db.get_bank_slots(a).unwrap();
    let names: Vec<_> = slots[..3]
        .iter()
        .map(|s| s.as_ref().unwrap().name.clone())
        .collect();
    assert_eq!(names, ["OvertureCK", "Smoog   OK", "JunoArp JS"]);
    assert_eq!(slots[1].as_ref().unwrap().category1, SoundCategory::Bass);

    let device_id = scanned[0].0[5];
    // By default flagged patches are not exported at all.
    let default_frames = db.export_bank_dumps(a, device_id, None).unwrap();
    assert!(default_frames.iter().all(|f| f[265] == single_dump_checksum(device_id, 1, f[8], f[9..265].try_into().unwrap())));
    let export = db.export_bank(a, device_id, None, BadChecksumPolicy::Skip).unwrap();
    assert!(export.skipped_flagged > 0);
    assert_eq!(export.frames.len() + export.skipped_flagged, 127);
    let frames = db
        .export_bank(a, device_id, None, BadChecksumPolicy::Rechecksum)
        .unwrap()
        .frames;
    // The file holds two dumps with ss = 13 (none with ss = 10); the later one wins.
    let mut expected: Vec<Option<&(Vec<u8>, IncomingDump)>> = vec![None; BANK_SLOTS];
    for entry in &scanned {
        expected[entry.1.program as usize] = Some(entry);
    }
    let expected: Vec<_> = expected.into_iter().flatten().collect();
    assert_eq!(frames.len(), expected.len());
    for (exported, (original, dump)) in frames.iter().zip(expected) {
        assert_eq!(&exported[9..265], &dump.data[..]);
        // Frames are identical except that a wrong stored checksum is corrected on export.
        if dump.checksum_ok {
            assert_eq!(exported, original);
        } else {
            assert_eq!(&exported[..265], &original[..265]);
            assert_ne!(exported[265], original[265]);
        }
    }
}

#[test]
fn plan_import_lists_overwrites_without_writing() {
    let db = PatchDatabase::open_in_memory().unwrap();
    let a = bank_id(&db, "A");
    db.save_patch(a, 5, &synthetic_patch("Mine", 1, 0), &PatchMeta::default())
        .unwrap();
    let dumps = [dump(1, 5, "Incoming"), dump(1, 6, "Fresh"), dump(0, 0, "Loose")];
    let before = db.data_version();

    let plan = db.plan_import(&dumps, ImportTarget::OriginalBanks).unwrap();
    assert_eq!(plan.slots, 2);
    assert_eq!(plan.overwrites.len(), 1);
    let o = &plan.overwrites[0];
    assert_eq!((o.slot.program, o.old_name.as_str(), o.new_name.as_str()), (5, "Mine", "Incoming"));
    assert_eq!(db.data_version(), before);
    assert_eq!(db.get_patch_by_slot(a, 5).unwrap().unwrap().name, "Mine");

    // A new bank has nothing to overwrite; a forced bank reports its own occupants.
    let user = db.create_bank("User").unwrap();
    db.plan_import(&dumps, ImportTarget::Bank(user)).unwrap();
    assert!(db.plan_import(&[dump(1, 5, "X")], ImportTarget::Bank(user)).unwrap().overwrites.is_empty());
    assert_eq!(db.plan_import(&[dump(1, 5, "X")], ImportTarget::Bank(a)).unwrap().overwrites.len(), 1);

    let report = db.import_single_dumps(&dumps, ImportTarget::OriginalBanks).unwrap();
    assert_eq!(report.imported, plan.slots);
}

#[test]
fn duplicate_slots_in_one_import_are_reported() {
    let db = PatchDatabase::open_in_memory().unwrap();
    let dumps = [dump(1, 13, "First"), dump(1, 13, "Second"), dump(1, 14, "Other")];
    let plan = db.plan_import(&dumps, ImportTarget::OriginalBanks).unwrap();
    assert_eq!((plan.slots, plan.duplicates.len()), (2, 1));
    let report = db.import_single_dumps(&dumps, ImportTarget::OriginalBanks).unwrap();
    assert_eq!((report.imported, report.duplicates.len()), (2, 1));
    assert_eq!(report.duplicates[0].program, 13);
    let a = bank_id(&db, "A");
    assert_eq!(db.get_patch_by_slot(a, 13).unwrap().unwrap().name, "Second");
}

#[test]
fn eight_bit_data_is_rejected_on_import_and_save() {
    let db = PatchDatabase::open_in_memory().unwrap();
    let mut bad = dump(1, 0, "Bad");
    bad.data[100] = 0x85;
    let report = db
        .import_single_dumps(&[bad.clone(), dump(1, 1, "Good")], ImportTarget::OriginalBanks)
        .unwrap();
    assert_eq!((report.imported, report.skipped), (1, 1));
    let a = bank_id(&db, "A");
    assert!(matches!(
        db.save_patch(a, 2, &bad.data, &PatchMeta::default()),
        Err(DbError::NotSevenBit { offset: 100 })
    ));
}

#[test]
fn flagged_patches_follow_the_export_policy() {
    let db = PatchDatabase::open_in_memory().unwrap();
    let mut flagged = dump(1, 1, "Damaged");
    flagged.checksum_ok = false;
    db.import_single_dumps(&[dump(1, 0, "Fine"), flagged], ImportTarget::OriginalBanks)
        .unwrap();
    let a = bank_id(&db, "A");

    let skip = db.export_bank(a, 0x10, None, BadChecksumPolicy::Skip).unwrap();
    assert_eq!((skip.frames.len(), skip.skipped_flagged, skip.included_flagged), (1, 1, 0));
    assert_eq!(db.export_bank_dumps(a, 0x10, None).unwrap().len(), 1);

    let correct = single_dump_checksum(0x10, 1, 1, &synthetic_patch("Damaged", 1, 0));
    let keep = db.export_bank(a, 0x10, None, BadChecksumPolicy::KeepInvalid).unwrap();
    assert_eq!((keep.frames.len(), keep.included_flagged), (2, 1));
    assert_ne!(keep.frames[1][265], correct);
    assert!(keep.frames[1][265] < 0x80);

    let fix = db.export_bank(a, 0x10, None, BadChecksumPolicy::Rechecksum).unwrap();
    assert_eq!(fix.frames[1][265], correct);
}

#[test]
fn on_disk_v1_database_is_backed_up_including_wal() {
    let dir = std::env::temp_dir().join(format!("virus-db-v1-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("virus_library.db");

    // Keep this connection open so its changes stay in the -wal file.
    let v1 = Connection::open(&path).unwrap();
    v1.execute_batch("PRAGMA journal_mode = WAL;").unwrap();
    v1.execute_batch(crate::schema::SCHEMA_V1_SQL).unwrap();
    v1.execute(
        "INSERT INTO banks (name, bank_letter, is_factory) VALUES ('Bank A', 'A', 1)",
        [],
    )
    .unwrap();
    let data = synthetic_patch("Walled", 1, 0);
    v1.execute(
        "INSERT INTO patches (bank_id, program_number, name, raw_data, checksum)
         VALUES (1, 0, 'Walled', ?1, 0)",
        params![&data[..]],
    )
    .unwrap();

    let db = PatchDatabase::open(&path).unwrap();
    assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
    let backup = db.v1_backup().expect("a backup was made").to_path_buf();
    assert_eq!(backup, dir.join("virus_library.v1.bak"));
    drop(db);
    drop(v1);

    let old = Connection::open(&backup).unwrap();
    let (version, name): (i64, String) = old
        .query_row(
            "SELECT (SELECT user_version FROM pragma_user_version), name FROM patches",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((version, name.as_str()), (0, "Walled"));
    drop(old);
    std::fs::remove_dir_all(&dir).unwrap();
}

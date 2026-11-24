//! Multi storage: the Multi bank's 128 slots plus unslotted multis (schema v3).

use rusqlite::{params, OptionalExtension, Row, ToSql};

use super::{check_program, PatchDatabase, BANK_SLOTS};
use crate::error::{DbError, Result};
use crate::models::{IncomingMultiDump, MultiImportReport, MultiRecord, MultiSummary, PatchMeta};
use crate::multi_data::{encode_multi_dump, multi_name, MULTI_BANK_BYTE, MULTI_EDIT_BUFFER_BANK, MULTI_LEN};

const MULTI_SUMMARY_COLUMNS: &str = "id, slot, name, tags, favorite, checksum_ok";
const MULTI_RECORD_COLUMNS: &str = "id, slot, name, tags, favorite, checksum_ok, raw_data";

const SAVE_SLOTTED_SQL: &str = r#"
INSERT INTO multis (slot, name, raw_data, checksum_ok, tags, favorite, updated_at)
VALUES (?1, ?2, ?3, 1, ?4, ?5, CURRENT_TIMESTAMP)
ON CONFLICT(slot) WHERE slot IS NOT NULL DO UPDATE SET
    name = excluded.name,
    raw_data = excluded.raw_data,
    checksum_ok = excluded.checksum_ok,
    tags = excluded.tags,
    favorite = excluded.favorite,
    updated_at = CURRENT_TIMESTAMP
RETURNING id
"#;

/// Import keeps the slot's tags/favourite only if the incoming multi has the same name.
const IMPORT_SLOTTED_SQL: &str = r#"
INSERT INTO multis (slot, name, raw_data, checksum_ok, tags, favorite, updated_at)
VALUES (?1, ?2, ?3, ?4, '', 0, CURRENT_TIMESTAMP)
ON CONFLICT(slot) WHERE slot IS NOT NULL DO UPDATE SET
    tags = CASE WHEN multis.name = excluded.name THEN multis.tags ELSE '' END,
    favorite = CASE WHEN multis.name = excluded.name THEN multis.favorite ELSE 0 END,
    name = excluded.name,
    raw_data = excluded.raw_data,
    checksum_ok = excluded.checksum_ok,
    updated_at = CURRENT_TIMESTAMP
"#;

impl PatchDatabase {
    /// Stores `data` byte-exact. `Some(slot)` (0..=127) replaces the Multi bank slot; `None`
    /// adds an unslotted multi. The name is derived from `data`. Returns the multi ID.
    pub fn save_multi(&self, slot: Option<u8>, data: &[u8; MULTI_LEN], meta: &PatchMeta) -> Result<i64> {
        let name = multi_name(data);
        let id = match slot {
            Some(slot) => {
                check_program(slot)?;
                self.conn.query_row(
                    SAVE_SLOTTED_SQL,
                    params![slot, name, &data[..], meta.tags, meta.favorite],
                    |r| r.get(0),
                )?
            }
            None => self.conn.query_row(
                "INSERT INTO multis (slot, name, raw_data, checksum_ok, tags, favorite)
                 VALUES (NULL, ?1, ?2, 1, ?3, ?4) RETURNING id",
                params![name, &data[..], meta.tags, meta.favorite],
                |r| r.get(0),
            )?,
        };
        self.bump();
        Ok(id)
    }

    pub fn get_multi_by_id(&self, id: i64) -> Result<Option<MultiRecord>> {
        let sql = format!("SELECT {MULTI_RECORD_COLUMNS} FROM multis WHERE id = ?1");
        self.conn
            .query_row(&sql, params![id], row_to_multi_parts)
            .optional()?
            .map(into_multi_record)
            .transpose()
    }

    pub fn get_multi_by_slot(&self, slot: u8) -> Result<Option<MultiRecord>> {
        let sql = format!("SELECT {MULTI_RECORD_COLUMNS} FROM multis WHERE slot = ?1");
        self.conn
            .query_row(&sql, params![slot], row_to_multi_parts)
            .optional()?
            .map(into_multi_record)
            .transpose()
    }

    /// Exactly 128 entries, index = Multi bank program; `None` marks an empty slot.
    pub fn get_multi_slots(&self) -> Result<Vec<Option<MultiSummary>>> {
        let sql = format!("SELECT {MULTI_SUMMARY_COLUMNS} FROM multis WHERE slot IS NOT NULL");
        let mut stmt = self.conn.prepare(&sql)?;
        let mut slots = vec![None; BANK_SLOTS];
        for summary in stmt.query_map([], row_to_multi_summary)? {
            let summary = summary?;
            if let Some(slot) = summary.slot.filter(|&s| (s as usize) < BANK_SLOTS) {
                slots[slot as usize] = Some(summary);
            }
        }
        Ok(slots)
    }

    /// Unslotted multis, oldest first.
    pub fn get_loose_multis(&self) -> Result<Vec<MultiSummary>> {
        let sql = format!("SELECT {MULTI_SUMMARY_COLUMNS} FROM multis WHERE slot IS NULL ORDER BY id ASC");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], row_to_multi_summary)?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn delete_multi(&self, id: i64) -> Result<bool> {
        let n = self.conn.execute("DELETE FROM multis WHERE id = ?1", params![id])?;
        if n > 0 {
            self.bump();
        }
        Ok(n > 0)
    }

    /// Updates only the favourite flag. Returns `false` if the multi doesn't exist.
    pub fn set_multi_favorite(&self, id: i64, favorite: bool) -> Result<bool> {
        self.update_multi_meta("favorite", &favorite, id)
    }

    /// Updates only the tags. Returns `false` if the multi doesn't exist.
    pub fn set_multi_tags(&self, id: i64, tags: &str) -> Result<bool> {
        self.update_multi_meta("tags", &tags, id)
    }

    fn update_multi_meta(&self, column: &str, value: &dyn ToSql, id: i64) -> Result<bool> {
        let sql = format!("UPDATE multis SET {column} = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2");
        let n = self.conn.execute(&sql, params![value, id])?;
        if n > 0 {
            self.bump();
        }
        Ok(n > 0)
    }

    /// Writes parsed multi dumps in one transaction (all or nothing).
    ///
    /// * `bb == 1` → Multi bank slot `mm` (replacing what's there; tags/favourite are kept only
    ///   if the name is unchanged).
    /// * `bb == 0` (edit buffer) dumps are never written; they are returned in `loose`.
    /// * Other `bb` values and `mm > 127` are counted as `skipped`.
    /// * Dumps with bad checksums are still imported, flagged and counted in `bad_checksum`.
    pub fn import_multi_dumps(&self, dumps: &[IncomingMultiDump]) -> Result<MultiImportReport> {
        let tx = self.conn.unchecked_transaction()?;
        let mut report = MultiImportReport::default();
        {
            let mut insert = tx.prepare(IMPORT_SLOTTED_SQL)?;
            for dump in dumps {
                if !dump.checksum_ok {
                    report.bad_checksum += 1;
                }
                match dump.bank {
                    MULTI_EDIT_BUFFER_BANK => report.loose.push(dump.clone()),
                    MULTI_BANK_BYTE if (dump.program as usize) < BANK_SLOTS => {
                        insert.execute(params![
                            dump.program,
                            multi_name(&dump.data),
                            &dump.data[..],
                            dump.checksum_ok
                        ])?;
                        report.imported += 1;
                    }
                    _ => report.skipped += 1,
                }
            }
        }
        tx.commit()?;
        if report.imported > 0 {
            self.bump();
        }
        Ok(report)
    }

    /// One 267-byte Multi Dump frame (`bb = 01`) per stored Multi bank slot, in slot order.
    /// Data bytes are sent exactly as stored.
    pub fn export_multi_bank_dumps(&self, device_id: u8) -> Result<Vec<Vec<u8>>> {
        let mut stmt =
            self.conn.prepare("SELECT id, slot, raw_data FROM multis WHERE slot IS NOT NULL ORDER BY slot ASC")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, u8>(1)?, r.get::<_, Vec<u8>>(2)?)))?;
        let mut frames = Vec::new();
        for row in rows {
            let (id, slot, raw) = row?;
            let data = to_multi_bytes(id, &raw)?;
            frames.push(encode_multi_dump(device_id, MULTI_BANK_BYTE, slot, &data));
        }
        Ok(frames)
    }

    /// Concatenation of [`Self::export_multi_bank_dumps`], suitable for a `.syx` file.
    pub fn export_multi_bank_syx(&self, device_id: u8) -> Result<Vec<u8>> {
        Ok(self.export_multi_bank_dumps(device_id)?.concat())
    }
}

fn row_to_multi_summary(row: &Row) -> rusqlite::Result<MultiSummary> {
    Ok(MultiSummary {
        id: row.get(0)?,
        slot: row.get(1)?,
        name: row.get(2)?,
        tags: row.get(3)?,
        favorite: row.get(4)?,
        checksum_ok: row.get(5)?,
    })
}

fn row_to_multi_parts(row: &Row) -> rusqlite::Result<(MultiSummary, Vec<u8>)> {
    Ok((row_to_multi_summary(row)?, row.get(6)?))
}

fn to_multi_bytes(id: i64, raw: &[u8]) -> Result<[u8; MULTI_LEN]> {
    raw.try_into().map_err(|_| DbError::CorruptMulti { id, len: raw.len() })
}

fn into_multi_record((s, raw): (MultiSummary, Vec<u8>)) -> Result<MultiRecord> {
    Ok(MultiRecord {
        data: to_multi_bytes(s.id, &raw)?,
        id: s.id,
        slot: s.slot,
        name: s.name,
        tags: s.tags,
        favorite: s.favorite,
        checksum_ok: s.checksum_ok,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multi_data::{multi_dump_checksum, MULTI_DUMP_LEN, MULTI_NAME_LEN, MULTI_NAME_OFFSET};
    use crate::schema::{CREATE_TABLES_SQL, SCHEMA_VERSION};
    use rusqlite::Connection;

    /// Multi with non-zero internal bytes everywhere and the given name.
    fn synthetic_multi(name: &str) -> [u8; MULTI_LEN] {
        let mut data = [0u8; MULTI_LEN];
        for (i, b) in data.iter_mut().enumerate() {
            *b = ((i * 29 + 5) % 128) as u8;
        }
        let mut padded = [b' '; MULTI_NAME_LEN];
        padded[..name.len()].copy_from_slice(name.as_bytes());
        data[MULTI_NAME_OFFSET..MULTI_NAME_OFFSET + MULTI_NAME_LEN].copy_from_slice(&padded);
        data
    }

    fn dump(bank: u8, program: u8, name: &str) -> IncomingMultiDump {
        IncomingMultiDump { bank, program, data: synthetic_multi(name), checksum_ok: true }
    }

    fn multi_count(db: &PatchDatabase) -> i64 {
        db.conn.query_row("SELECT COUNT(*) FROM multis", [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn save_and_load_is_byte_exact() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let data = synthetic_multi("Sequencer");
        let meta = PatchMeta { tags: "live".into(), favorite: true };
        let id = db.save_multi(Some(3), &data, &meta).unwrap();
        let rec = db.get_multi_by_id(id).unwrap().unwrap();
        assert_eq!((rec.data, rec.slot, rec.name.as_str()), (data, Some(3), "Sequencer"));
        assert_eq!((rec.tags.as_str(), rec.favorite, rec.checksum_ok), ("live", true, true));
        assert_eq!(db.get_multi_by_slot(3).unwrap().unwrap(), rec);

        // Upsert into the same slot keeps the ID.
        let other = synthetic_multi("Other");
        assert_eq!(db.save_multi(Some(3), &other, &PatchMeta::default()).unwrap(), id);
        assert_eq!(db.get_multi_by_id(id).unwrap().unwrap().name, "Other");

        // Unslotted multis never collide.
        let a = db.save_multi(None, &data, &PatchMeta::default()).unwrap();
        let b = db.save_multi(None, &data, &PatchMeta::default()).unwrap();
        assert_ne!(a, b);
        let loose: Vec<_> = db.get_loose_multis().unwrap().into_iter().map(|m| (m.id, m.slot)).collect();
        assert_eq!(loose, [(a, None), (b, None)]);

        assert!(matches!(db.save_multi(Some(128), &data, &meta), Err(DbError::InvalidProgram(128))));
        assert!(db.set_multi_favorite(a, true).unwrap());
        assert!(db.set_multi_tags(a, "x").unwrap());
        assert_eq!(db.get_multi_by_id(a).unwrap().unwrap().data, data);
        assert!(db.delete_multi(a).unwrap());
        assert!(!db.delete_multi(a).unwrap());
    }

    #[test]
    fn slots_have_128_entries() {
        let db = PatchDatabase::open_in_memory().unwrap();
        db.save_multi(Some(0), &synthetic_multi("First"), &PatchMeta::default()).unwrap();
        db.save_multi(Some(127), &synthetic_multi("Last"), &PatchMeta::default()).unwrap();
        db.save_multi(None, &synthetic_multi("Loose"), &PatchMeta::default()).unwrap();
        let slots = db.get_multi_slots().unwrap();
        assert_eq!(slots.len(), 128);
        assert_eq!(slots[0].as_ref().unwrap().name, "First");
        assert_eq!(slots[127].as_ref().unwrap().name, "Last");
        assert_eq!(slots.iter().flatten().count(), 2);
    }

    #[test]
    fn import_routes_by_bank_byte() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let mut bad = dump(1, 7, "BadSum");
        bad.checksum_ok = false;
        let edit = dump(0, 0, "EditBuf");
        let dumps = [dump(1, 0, "Sequencer"), edit.clone(), dump(2, 1, "Bank2"), dump(1, 0x7F, "Last"), bad];
        let before = db.data_version();
        let report = db.import_multi_dumps(&dumps).unwrap();
        assert_eq!(report.imported, 3);
        assert_eq!(report.loose, [edit]);
        assert_eq!((report.bad_checksum, report.skipped), (1, 1));
        assert_ne!(db.data_version(), before);
        assert_eq!(multi_count(&db), 3);
        assert!(!db.get_multi_by_slot(7).unwrap().unwrap().checksum_ok);
        assert_eq!(db.get_multi_by_slot(0).unwrap().unwrap().data, synthetic_multi("Sequencer"));
    }

    #[test]
    fn import_keeps_metadata_only_for_same_name() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let meta = PatchMeta { tags: "mine".into(), favorite: true };
        db.save_multi(Some(0), &synthetic_multi("Same"), &meta).unwrap();
        db.save_multi(Some(1), &synthetic_multi("Old"), &meta).unwrap();
        let mut same = dump(1, 0, "Same");
        same.data[200] = 99;
        db.import_multi_dumps(&[same.clone(), dump(1, 1, "New")]).unwrap();
        let kept = db.get_multi_by_slot(0).unwrap().unwrap();
        assert_eq!((kept.data, kept.tags.as_str(), kept.favorite), (same.data, "mine", true));
        let reset = db.get_multi_by_slot(1).unwrap().unwrap();
        assert_eq!((reset.tags.as_str(), reset.favorite), ("", false));
    }

    #[test]
    fn import_rolls_back_on_failure() {
        let db = PatchDatabase::open_in_memory().unwrap();
        db.conn
            .execute_batch(
                "CREATE TRIGGER fail_on_5 BEFORE INSERT ON multis WHEN NEW.slot = 5
                 BEGIN SELECT RAISE(ABORT, 'boom'); END;",
            )
            .unwrap();
        let dumps: Vec<_> = (0..10).map(|p| dump(1, p, "X")).collect();
        assert!(matches!(db.import_multi_dumps(&dumps), Err(DbError::Sqlite(_))));
        assert_eq!(multi_count(&db), 0);
    }

    #[test]
    fn export_frames_are_exact_and_in_slot_order() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let (m0, m9) = (synthetic_multi("Zero"), synthetic_multi("Nine"));
        db.save_multi(Some(9), &m9, &PatchMeta::default()).unwrap();
        db.save_multi(Some(0), &m0, &PatchMeta::default()).unwrap();
        db.save_multi(None, &m0, &PatchMeta::default()).unwrap();
        let frames = db.export_multi_bank_dumps(0x10).unwrap();
        assert_eq!(frames.len(), 2);
        for (frame, (slot, data)) in frames.iter().zip([(0u8, &m0), (9, &m9)]) {
            assert_eq!(frame.len(), MULTI_DUMP_LEN);
            assert_eq!(&frame[..9], &[0xF0, 0x00, 0x20, 0x33, 0x01, 0x10, 0x11, 0x01, slot]);
            assert_eq!(&frame[9..265], &data[..]);
            assert_eq!(frame[265], multi_dump_checksum(0x10, 1, slot, data));
        }
        assert_eq!(db.export_multi_bank_syx(0x10).unwrap(), frames.concat());
    }

    #[test]
    fn corrupt_multi_blob_is_an_error() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let id = db.save_multi(Some(0), &synthetic_multi("Ok"), &PatchMeta::default()).unwrap();
        db.conn.execute("UPDATE multis SET raw_data = zeroblob(9) WHERE id = ?1", params![id]).unwrap();
        assert!(matches!(db.get_multi_by_id(id), Err(DbError::CorruptMulti { len: 9, .. })));
        assert!(matches!(db.export_multi_bank_dumps(0), Err(DbError::CorruptMulti { len: 9, .. })));
    }

    #[test]
    fn factory_multis_import_and_export_byte_exact() {
        let file = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../VirusOSR200_INT/OSR200_INT/Goodies/Factory soundset/multis.mid"
        ))
        .unwrap();
        // SMF SysEx events omit F0; find `00 20 33 01 dd 11 bb mm [256] cs F7`.
        let mut scanned = Vec::new();
        let mut i = 0;
        while i + MULTI_DUMP_LEN - 1 <= file.len() {
            let h = &file[i..];
            if h[..4] == [0x00, 0x20, 0x33, 0x01] && h[5] == 0x11 && h[MULTI_DUMP_LEN - 2] == 0xF7 {
                let (dd, bb, mm) = (h[4], h[6], h[7]);
                let data: [u8; MULTI_LEN] = h[8..8 + MULTI_LEN].try_into().unwrap();
                let checksum_ok = multi_dump_checksum(dd, bb, mm, &data) == h[8 + MULTI_LEN];
                scanned.push((dd, IncomingMultiDump { bank: bb, program: mm, data, checksum_ok }));
                i += MULTI_DUMP_LEN - 1;
            } else {
                i += 1;
            }
        }
        assert_eq!(scanned.len(), 129);
        let dumps: Vec<_> = scanned.iter().map(|(_, d)| d.clone()).collect();

        let db = PatchDatabase::open_in_memory().unwrap();
        let report = db.import_multi_dumps(&dumps).unwrap();
        // Multi 10 reads as mm = 13 in the damaged file, so slot 13 is written twice.
        assert_eq!((report.imported, report.loose.len(), report.skipped), (128, 1, 0));
        let slots = db.get_multi_slots().unwrap();
        assert_eq!(slots.iter().flatten().count(), 127);
        assert_eq!(slots[1].as_ref().unwrap().name, "DrumKit 01");
        assert_eq!(report.loose[0].data, dumps[0].data);

        let mut expected: Vec<Option<&IncomingMultiDump>> = vec![None; BANK_SLOTS];
        for dump in dumps.iter().filter(|d| d.bank == 1) {
            expected[dump.program as usize] = Some(dump);
        }
        let frames = db.export_multi_bank_dumps(scanned[0].0).unwrap();
        for (frame, dump) in frames.iter().zip(expected.into_iter().flatten()) {
            assert_eq!(frame[8], dump.program);
            assert_eq!(&frame[9..265], &dump.data[..]);
        }
    }

    fn v2_connection() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(CREATE_TABLES_SQL).unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();
        conn.execute("INSERT INTO banks (name, bank_letter, is_rom) VALUES ('Bank A', 'A', 0)", []).unwrap();
        conn.execute(
            "INSERT INTO patches (bank_id, program_number, name, raw_data) VALUES (1, 4, 'Kept', ?1)",
            params![&[7u8; 256][..]],
        )
        .unwrap();
        conn
    }

    #[test]
    fn migrates_v2_to_v3_keeping_patches() {
        let db = PatchDatabase::with_connection(v2_connection(), None).unwrap();
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
        assert_eq!(db.get_patch_by_slot(1, 4).unwrap().unwrap().data, [7u8; 256]);
        assert_eq!(db.get_banks().unwrap().len(), 1);
        db.save_multi(Some(0), &synthetic_multi("New"), &PatchMeta::default()).unwrap();
        assert_eq!(multi_count(&db), 1);
    }

    #[test]
    fn failed_v2_to_v3_migration_rolls_back() {
        let dir = std::env::temp_dir().join(format!("virus-db-v3-rollback-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("library.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(CREATE_TABLES_SQL).unwrap();
            conn.pragma_update(None, "user_version", 2).unwrap();
            // A view of the same name makes `CREATE TABLE IF NOT EXISTS` a no-op and the index fail.
            conn.execute_batch("CREATE VIEW multis AS SELECT 1 AS slot, 'x' AS name;").unwrap();
        }
        assert!(matches!(PatchDatabase::open(&path), Err(DbError::Sqlite(_))));
        {
            let conn = Connection::open(&path).unwrap();
            let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
            assert_eq!(version, 2);
            let indexes: i64 = conn
                .query_row("SELECT COUNT(*) FROM sqlite_master WHERE name LIKE 'idx_multis_%'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(indexes, 0);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

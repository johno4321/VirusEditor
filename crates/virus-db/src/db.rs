//! SQLite operations for the Access Virus patch librarian.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension, Row, ToSql};

use crate::error::{DbError, Result};
use crate::models::{
    BankRecord, ImportReport, ImportTarget, IncomingDump, PatchFilter, PatchMeta, PatchRecord,
    PatchSummary, SoundCategory,
};
use crate::patch_data::{
    bank_byte_for_letter, encode_single_dump, letter_for_bank_byte, patch_categories, patch_name,
    EDIT_BUFFER_BANK, PATCH_LEN,
};
use crate::schema::{CREATE_MULTI_TABLES_SQL, CREATE_TABLES_SQL, MIGRATE_V1_TO_V2_SQL, SCHEMA_VERSION};

/// Number of program slots in a bank.
pub const BANK_SLOTS: usize = 128;

const SUMMARY_COLUMNS: &str =
    "id, bank_id, program_number, name, category1, category2, tags, favorite, checksum_ok";
const RECORD_COLUMNS: &str =
    "id, bank_id, program_number, name, category1, category2, tags, favorite, checksum_ok, raw_data";

const SAVE_PATCH_SQL: &str = r#"
INSERT INTO patches (bank_id, program_number, name, category1, category2, raw_data,
                     checksum_ok, tags, favorite, updated_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?8, CURRENT_TIMESTAMP)
ON CONFLICT(bank_id, program_number) DO UPDATE SET
    name = excluded.name,
    category1 = excluded.category1,
    category2 = excluded.category2,
    raw_data = excluded.raw_data,
    checksum_ok = excluded.checksum_ok,
    tags = excluded.tags,
    favorite = excluded.favorite,
    updated_at = CURRENT_TIMESTAMP
RETURNING id
"#;

/// Import keeps the slot's tags/favourite only if the incoming patch has the same name.
const IMPORT_PATCH_SQL: &str = r#"
INSERT INTO patches (bank_id, program_number, name, category1, category2, raw_data,
                     checksum_ok, tags, favorite, updated_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, '', 0, CURRENT_TIMESTAMP)
ON CONFLICT(bank_id, program_number) DO UPDATE SET
    tags = CASE WHEN patches.name = excluded.name THEN patches.tags ELSE '' END,
    favorite = CASE WHEN patches.name = excluded.name THEN patches.favorite ELSE 0 END,
    name = excluded.name,
    category1 = excluded.category1,
    category2 = excluded.category2,
    raw_data = excluded.raw_data,
    checksum_ok = excluded.checksum_ok,
    updated_at = CURRENT_TIMESTAMP
"#;

/// SQLite-backed library of banks and 256-byte single patches.
pub struct PatchDatabase {
    conn: Connection,
    /// Incremented on every successful write through this handle (see [`Self::data_version`]).
    writes: Cell<u64>,
}

impl PatchDatabase {
    /// Opens or creates a database file, migrating older schemas.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::with_connection(Connection::open(path)?)
    }

    /// Creates an in-memory database (tests, ephemeral sessions).
    pub fn open_in_memory() -> Result<Self> {
        Self::with_connection(Connection::open_in_memory()?)
    }

    fn with_connection(conn: Connection) -> Result<Self> {
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
        let db = Self {
            conn,
            writes: Cell::new(0),
        };
        db.migrate()?;
        db.seed_default_banks()?;
        Ok(db)
    }

    /// Current `PRAGMA user_version` of the database.
    pub fn schema_version(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    fn migrate(&self) -> Result<()> {
        match self.schema_version()? {
            0 if !self.table_exists("patches")? => {
                let tx = self.conn.unchecked_transaction()?;
                tx.execute_batch(CREATE_TABLES_SQL)?;
                tx.execute_batch(CREATE_MULTI_TABLES_SQL)?;
                tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
                tx.commit()?;
            }
            // Pre-versioning databases (user_version 0 with tables) are v1.
            0 | 1 => {
                self.migrate_v1_to_v2()?;
                self.migrate_v2_to_v3()?;
            }
            2 => self.migrate_v2_to_v3()?,
            SCHEMA_VERSION => {}
            v => return Err(DbError::UnsupportedSchemaVersion(v)),
        }
        Ok(())
    }

    fn table_exists(&self, name: &str) -> Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            params![name],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// v1 → v2 in one transaction: integer categories re-derived from the patch bytes,
    /// `is_rom` replaces `is_factory`. Blobs that aren't 256 bytes are copied untouched.
    fn migrate_v1_to_v2(&self) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch(MIGRATE_V1_TO_V2_SQL)?;
        tx.execute_batch(CREATE_TABLES_SQL)?;
        tx.execute(
            "INSERT INTO patches (id, bank_id, program_number, name, category1, category2, raw_data,
                                  checksum_ok, tags, favorite, created_at, updated_at)
             SELECT id, bank_id, program_number, name, 0, 0, raw_data, 1, tags, favorite,
                    created_at, updated_at
             FROM patches_v1",
            [],
        )?;
        {
            let mut select = tx.prepare("SELECT id, raw_data FROM patches")?;
            let mut update = tx.prepare(
                "UPDATE patches SET name = ?2, category1 = ?3, category2 = ?4 WHERE id = ?1",
            )?;
            let rows =
                select.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))?;
            for row in rows {
                let (id, raw) = row?;
                if let Ok(data) = <[u8; PATCH_LEN]>::try_from(raw.as_slice()) {
                    let (c1, c2) = patch_categories(&data);
                    update.execute(params![id, patch_name(&data), c1.to_raw(), c2.to_raw()])?;
                }
            }
        }
        tx.execute_batch("DROP TABLE patches_v1;")?;
        tx.pragma_update(None, "user_version", 2)?;
        tx.commit()?;
        Ok(())
    }

    /// v2 → v3 in one transaction: adds the `multis` table. Nothing else changes.
    fn migrate_v2_to_v3(&self) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch(CREATE_MULTI_TABLES_SQL)?;
        tx.pragma_update(None, "user_version", 3)?;
        tx.commit()?;
        Ok(())
    }

    /// Seeds banks A, B (RAM) and C, D (ROM) when the library has no banks at all.
    fn seed_default_banks(&self) -> Result<()> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM banks", [], |r| r.get(0))?;
        if count == 0 {
            let tx = self.conn.unchecked_transaction()?;
            for letter in ["A", "B", "C", "D"] {
                insert_letter_bank(&tx, letter)?;
            }
            tx.commit()?;
        }
        Ok(())
    }

    /// Cheap change counter for UI caching: changes whenever this handle writes or another
    /// connection commits to the same database file.
    pub fn data_version(&self) -> u64 {
        let external: i64 = self
            .conn
            .query_row("PRAGMA data_version", [], |r| r.get(0))
            .unwrap_or(0);
        self.writes.get().wrapping_add(external as u64)
    }

    fn bump(&self) {
        self.writes.set(self.writes.get().wrapping_add(1));
    }

    // ------------------------------------------------------------------------
    // Banks
    // ------------------------------------------------------------------------

    /// Creates a user bank (no synth bank letter, not ROM) and returns its ID.
    pub fn create_bank(&self, name: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO banks (name, bank_letter, is_rom) VALUES (?1, NULL, 0)",
            params![name],
        )?;
        self.bump();
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_banks(&self) -> Result<Vec<BankRecord>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, bank_letter, is_rom FROM banks ORDER BY id ASC")?;
        let banks = stmt
            .query_map([], row_to_bank)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(banks)
    }

    pub fn get_bank_by_id(&self, id: i64) -> Result<Option<BankRecord>> {
        Ok(bank_by_id(&self.conn, id)?)
    }

    /// Deletes a bank and all its patches.
    pub fn delete_bank(&self, id: i64) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM banks WHERE id = ?1", params![id])?;
        if n > 0 {
            self.bump();
        }
        Ok(n > 0)
    }

    // ------------------------------------------------------------------------
    // Patches
    // ------------------------------------------------------------------------

    /// Stores `data` byte-exact in slot `program` (0..=127) of a bank, replacing any patch
    /// there. Name and categories are derived from `data`. Returns the patch ID.
    pub fn save_patch(
        &self,
        bank_id: i64,
        program: u8,
        data: &[u8; PATCH_LEN],
        meta: &PatchMeta,
    ) -> Result<i64> {
        check_program(program)?;
        self.require_bank(bank_id)?;
        let (c1, c2) = patch_categories(data);
        let id = self.conn.query_row(
            SAVE_PATCH_SQL,
            params![
                bank_id,
                program,
                patch_name(data),
                c1.to_raw(),
                c2.to_raw(),
                &data[..],
                meta.tags,
                meta.favorite,
            ],
            |r| r.get(0),
        )?;
        self.bump();
        Ok(id)
    }

    /// Updates only the favourite flag. Returns `false` if the patch doesn't exist.
    pub fn set_favorite(&self, patch_id: i64, favorite: bool) -> Result<bool> {
        self.update_meta("favorite", &favorite, patch_id)
    }

    /// Updates only the tags. Returns `false` if the patch doesn't exist.
    pub fn set_tags(&self, patch_id: i64, tags: &str) -> Result<bool> {
        self.update_meta("tags", &tags, patch_id)
    }

    fn update_meta(&self, column: &str, value: &dyn ToSql, patch_id: i64) -> Result<bool> {
        let sql = format!(
            "UPDATE patches SET {column} = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2"
        );
        let n = self.conn.execute(&sql, params![value, patch_id])?;
        if n > 0 {
            self.bump();
        }
        Ok(n > 0)
    }

    pub fn get_patch_by_id(&self, id: i64) -> Result<Option<PatchRecord>> {
        let sql = format!("SELECT {RECORD_COLUMNS} FROM patches WHERE id = ?1");
        self.conn
            .query_row(&sql, params![id], row_to_record_parts)
            .optional()?
            .map(into_record)
            .transpose()
    }

    pub fn get_patch_by_slot(&self, bank_id: i64, program: u8) -> Result<Option<PatchRecord>> {
        let sql = format!(
            "SELECT {RECORD_COLUMNS} FROM patches WHERE bank_id = ?1 AND program_number = ?2"
        );
        self.conn
            .query_row(&sql, params![bank_id, program], row_to_record_parts)
            .optional()?
            .map(into_record)
            .transpose()
    }

    pub fn delete_patch(&self, id: i64) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM patches WHERE id = ?1", params![id])?;
        if n > 0 {
            self.bump();
        }
        Ok(n > 0)
    }

    /// Exactly 128 entries, index = program number; `None` marks an empty slot.
    pub fn get_bank_slots(&self, bank_id: i64) -> Result<Vec<Option<PatchSummary>>> {
        self.require_bank(bank_id)?;
        let sql = format!(
            "SELECT {SUMMARY_COLUMNS} FROM patches WHERE bank_id = ?1 AND program_number < {BANK_SLOTS}"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut slots = vec![None; BANK_SLOTS];
        for summary in stmt.query_map(params![bank_id], row_to_summary)? {
            let summary = summary?;
            let index = summary.program_number as usize;
            slots[index] = Some(summary);
        }
        Ok(slots)
    }

    /// Searches patches. `search_text` is matched literally (`%`, `_` and `\` are escaped).
    pub fn query_patches(&self, filter: &PatchFilter) -> Result<Vec<PatchSummary>> {
        let mut sql = format!("SELECT {SUMMARY_COLUMNS} FROM patches WHERE 1=1");
        let mut args: Vec<Box<dyn ToSql>> = Vec::new();

        if let Some(bank_id) = filter.bank_id {
            sql.push_str(" AND bank_id = ?");
            args.push(Box::new(bank_id));
        }
        if filter.favorite_only {
            sql.push_str(" AND favorite = 1");
        }
        if let Some(category) = filter.category {
            sql.push_str(" AND (category1 = ? OR category2 = ?)");
            args.push(Box::new(category.to_raw()));
            args.push(Box::new(category.to_raw()));
        }
        if let Some(search) = filter.search_text.as_deref().map(str::trim) {
            if !search.is_empty() {
                sql.push_str(r" AND (name LIKE ? ESCAPE '\' OR tags LIKE ? ESCAPE '\')");
                let pattern = format!("%{}%", escape_like(search));
                args.push(Box::new(pattern.clone()));
                args.push(Box::new(pattern));
            }
        }
        sql.push_str(" ORDER BY bank_id ASC, program_number ASC");

        let mut stmt = self.conn.prepare(&sql)?;
        let arg_refs: Vec<&dyn ToSql> = args.iter().map(AsRef::as_ref).collect();
        let rows = stmt
            .query_map(arg_refs.as_slice(), row_to_summary)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    // ------------------------------------------------------------------------
    // Import / export
    // ------------------------------------------------------------------------

    /// Writes parsed single dumps into bank slots in one transaction (all or nothing).
    ///
    /// * `bb == 0` (edit buffer) dumps are never written; they are returned in `loose`.
    /// * `OriginalBanks`: `bb` 1..=4 → library bank with letter A..D (created if missing);
    ///   other `bb` values are counted as `skipped`.
    /// * `Bank(id)`: all non-edit-buffer dumps go into that bank by program number. If they
    ///   come from more than one `bb`, nothing is written and `MixedSourceBanks` is returned.
    /// * Dumps with bad checksums are still imported (the Virus accepts them), flagged with
    ///   `checksum_ok = false` and counted in `bad_checksum`.
    /// * An overwritten slot keeps its tags/favourite only if the patch name is unchanged.
    pub fn import_single_dumps(
        &self,
        dumps: &[IncomingDump],
        target: ImportTarget,
    ) -> Result<ImportReport> {
        if let ImportTarget::Bank(_) = target {
            let mut sources: Vec<u8> = dumps
                .iter()
                .map(|d| d.bank)
                .filter(|&b| b != EDIT_BUFFER_BANK)
                .collect();
            sources.sort_unstable();
            sources.dedup();
            if sources.len() > 1 {
                return Err(DbError::MixedSourceBanks(sources));
            }
        }

        let tx = self.conn.unchecked_transaction()?;
        let forced = match target {
            ImportTarget::Bank(id) => Some(bank_by_id(&tx, id)?.ok_or(DbError::BankNotFound(id))?),
            ImportTarget::OriginalBanks => None,
        };
        let mut bank_cache: HashMap<u8, (i64, String)> = HashMap::new();
        let mut report = ImportReport::default();
        let mut insert = tx.prepare(IMPORT_PATCH_SQL)?;

        for dump in dumps {
            if !dump.checksum_ok {
                report.bad_checksum += 1;
            }
            if dump.bank == EDIT_BUFFER_BANK {
                report.loose.push(dump.clone());
                continue;
            }
            if dump.program as usize >= BANK_SLOTS {
                report.skipped += 1;
                continue;
            }
            let (bank_id, bank_name) = match &forced {
                Some(bank) => (bank.id, bank.name.clone()),
                None => match bank_cache.get(&dump.bank) {
                    Some(entry) => entry.clone(),
                    None => {
                        let Some(letter) = letter_for_bank_byte(dump.bank) else {
                            report.skipped += 1;
                            continue;
                        };
                        let bank = match bank_by_letter(&tx, letter)? {
                            Some(bank) => bank,
                            None => {
                                let id = insert_letter_bank(&tx, letter)?;
                                bank_by_id(&tx, id)?.ok_or(DbError::BankNotFound(id))?
                            }
                        };
                        bank_cache.insert(dump.bank, (bank.id, bank.name.clone()));
                        (bank.id, bank.name)
                    }
                },
            };

            let (c1, c2) = patch_categories(&dump.data);
            insert.execute(params![
                bank_id,
                dump.program,
                patch_name(&dump.data),
                c1.to_raw(),
                c2.to_raw(),
                &dump.data[..],
                dump.checksum_ok,
            ])?;
            report.imported += 1;
            match report
                .per_bank
                .iter_mut()
                .find(|(name, _)| *name == bank_name)
            {
                Some((_, count)) => *count += 1,
                None => report.per_bank.push((bank_name, 1)),
            }
        }

        drop(insert);
        tx.commit()?;
        if report.imported > 0 {
            self.bump();
        }
        Ok(report)
    }

    /// One 267-byte Single Dump frame per stored program, in program order. Data bytes are
    /// sent exactly as stored. `target_bank` is the SysEx `bb` (1..=4 = A..D); `None` uses the
    /// bank's letter and fails with `NoBankLetter` for user banks.
    pub fn export_bank_dumps(
        &self,
        bank_id: i64,
        device_id: u8,
        target_bank: Option<u8>,
    ) -> Result<Vec<Vec<u8>>> {
        let bank = self.require_bank(bank_id)?;
        let bb = match target_bank {
            Some(bb @ 1..=4) => bb,
            Some(bb) => return Err(DbError::InvalidTargetBank(bb)),
            None => bank
                .bank_letter
                .as_deref()
                .and_then(bank_byte_for_letter)
                .ok_or(DbError::NoBankLetter(bank_id))?,
        };

        let mut stmt = self.conn.prepare(
            "SELECT id, program_number, raw_data FROM patches
             WHERE bank_id = ?1 AND program_number < 128 ORDER BY program_number ASC",
        )?;
        let rows = stmt.query_map(params![bank_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, u8>(1)?,
                r.get::<_, Vec<u8>>(2)?,
            ))
        })?;
        let mut frames = Vec::new();
        for row in rows {
            let (id, program, raw) = row?;
            let data = to_patch_bytes(id, &raw)?;
            frames.push(encode_single_dump(device_id, bb, program, &data));
        }
        Ok(frames)
    }

    /// Concatenation of [`Self::export_bank_dumps`], suitable for a `.syx` file.
    pub fn export_bank_syx(
        &self,
        bank_id: i64,
        device_id: u8,
        target_bank: Option<u8>,
    ) -> Result<Vec<u8>> {
        Ok(self
            .export_bank_dumps(bank_id, device_id, target_bank)?
            .concat())
    }

    fn require_bank(&self, bank_id: i64) -> Result<BankRecord> {
        bank_by_id(&self.conn, bank_id)?.ok_or(DbError::BankNotFound(bank_id))
    }
}

fn check_program(program: u8) -> Result<()> {
    if (program as usize) < BANK_SLOTS {
        Ok(())
    } else {
        Err(DbError::InvalidProgram(program))
    }
}

fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn insert_letter_bank(conn: &Connection, letter: &str) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO banks (name, bank_letter, is_rom) VALUES (?1, ?2, ?3)",
        params![
            format!("Bank {letter}"),
            letter,
            matches!(letter, "C" | "D")
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

fn bank_by_id(conn: &Connection, id: i64) -> rusqlite::Result<Option<BankRecord>> {
    conn.query_row(
        "SELECT id, name, bank_letter, is_rom FROM banks WHERE id = ?1",
        params![id],
        row_to_bank,
    )
    .optional()
}

fn bank_by_letter(conn: &Connection, letter: &str) -> rusqlite::Result<Option<BankRecord>> {
    conn.query_row(
        "SELECT id, name, bank_letter, is_rom FROM banks WHERE bank_letter = ?1 ORDER BY id LIMIT 1",
        params![letter],
        row_to_bank,
    )
    .optional()
}

fn row_to_bank(row: &Row) -> rusqlite::Result<BankRecord> {
    Ok(BankRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        bank_letter: row.get(2)?,
        is_rom: row.get(3)?,
    })
}

fn row_to_summary(row: &Row) -> rusqlite::Result<PatchSummary> {
    Ok(PatchSummary {
        id: row.get(0)?,
        bank_id: row.get(1)?,
        program_number: row.get(2)?,
        name: row.get(3)?,
        category1: SoundCategory::from_raw(row.get(4)?),
        category2: SoundCategory::from_raw(row.get(5)?),
        tags: row.get(6)?,
        favorite: row.get(7)?,
        checksum_ok: row.get(8)?,
    })
}

fn row_to_record_parts(row: &Row) -> rusqlite::Result<(PatchSummary, Vec<u8>)> {
    Ok((row_to_summary(row)?, row.get(9)?))
}

fn to_patch_bytes(id: i64, raw: &[u8]) -> Result<[u8; PATCH_LEN]> {
    raw.try_into()
        .map_err(|_| DbError::CorruptPatch { id, len: raw.len() })
}

fn into_record((s, raw): (PatchSummary, Vec<u8>)) -> Result<PatchRecord> {
    Ok(PatchRecord {
        data: to_patch_bytes(s.id, &raw)?,
        id: s.id,
        bank_id: s.bank_id,
        program_number: s.program_number,
        name: s.name,
        category1: s.category1,
        category2: s.category2,
        tags: s.tags,
        favorite: s.favorite,
        checksum_ok: s.checksum_ok,
    })
}

#[cfg(test)]
mod tests;

mod multis;

//! SQLite operations for the Access Virus patch librarian.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension, Row, ToSql};

use crate::error::{DbError, Result};
use crate::models::{
    BadChecksumPolicy, BankExport, BankRecord, ImportPlan, ImportReport, ImportTarget,
    IncomingDump, PatchFilter, PatchMeta, PatchRecord, PatchSummary, SlotOverwrite, SlotRef,
    SoundCategory,
};
use crate::patch_data::{
    bank_byte_for_letter, encode_single_dump, first_eight_bit_offset, letter_for_bank_byte,
    patch_categories, patch_name, EDIT_BUFFER_BANK, PATCH_LEN, SINGLE_DUMP_CHECKSUM_INDEX,
};
use crate::schema::{CREATE_MULTI_TABLES_SQL, CREATE_TABLES_SQL, MIGRATE_V1_TO_V2_SQL, SCHEMA_VERSION};

/// Number of program slots in a bank.
pub const BANK_SLOTS: usize = 128;

/// How long a statement waits for another connection to the same file (e.g. the CLI's
/// `get-bank --to-library` while the GUI is open) before failing with "database is locked".
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(2);

const SUMMARY_COLUMNS: &str =
    "id, bank_id, program_number, name, category1, category2, tags, favorite, checksum_ok";
const RECORD_COLUMNS: &str =
    "id, bank_id, program_number, name, category1, category2, tags, favorite, checksum_ok, raw_data";

const SAVE_PATCH_SQL: &str = r#"
INSERT INTO patches (bank_id, program_number, name, category1, category2, raw_data,
                     checksum_ok, tags, favorite, updated_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?9, ?7, ?8, CURRENT_TIMESTAMP)
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
    /// Where the pre-migration copy of a v1 database file was written, if one was migrated.
    v1_backup: Option<PathBuf>,
}

impl PatchDatabase {
    /// Opens or creates a database file, migrating older schemas. A v1 file is first copied
    /// to `<name>.v1.bak` next to it (see [`Self::v1_backup`]).
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        Self::with_connection(Connection::open(path)?, Some(path))
    }

    /// Creates an in-memory database (tests, ephemeral sessions).
    pub fn open_in_memory() -> Result<Self> {
        Self::with_connection(Connection::open_in_memory()?, None)
    }

    /// The backup written when this database was migrated from v1 by [`Self::open`].
    pub fn v1_backup(&self) -> Option<&Path> {
        self.v1_backup.as_deref()
    }

    /// Writes a consistent copy of the database file `src` (including changes still in its
    /// WAL) to `dest`, which must not exist. `src` is opened but not migrated or modified.
    pub fn copy_file(src: impl AsRef<Path>, dest: impl AsRef<Path>) -> Result<()> {
        let conn = Connection::open_with_flags(
            src,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.execute(
            "VACUUM INTO ?1",
            params![dest.as_ref().to_string_lossy()],
        )?;
        Ok(())
    }

    /// Number of stored patches in all banks.
    pub fn patch_count(&self) -> Result<usize> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM patches", [], |r| r.get::<_, i64>(0))? as usize)
    }

    /// Whether the library holds nothing worth keeping: no patches, no multis and no banks
    /// other than the letter banks A–D every new library is seeded with.
    pub fn is_empty_library(&self) -> Result<bool> {
        let (patches, multis, other_banks): (i64, i64, i64) = self.conn.query_row(
            "SELECT (SELECT COUNT(*) FROM patches),
                    (SELECT COUNT(*) FROM multis),
                    (SELECT COUNT(*) FROM banks
                     WHERE bank_letter IS NULL OR bank_letter NOT IN ('A', 'B', 'C', 'D'))",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        Ok(patches == 0 && multis == 0 && other_banks == 0)
    }

    fn with_connection(conn: Connection, file: Option<&Path>) -> Result<Self> {
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
        let mut db = Self {
            conn,
            writes: Cell::new(0),
            v1_backup: None,
        };
        db.migrate(file)?;
        db.seed_default_banks()?;
        Ok(db)
    }

    /// Current `PRAGMA user_version` of the database.
    pub fn schema_version(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    fn migrate(&mut self, file: Option<&Path>) -> Result<()> {
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
                // The migration is one-way (the v1 binary can't open a v2 file).
                if let Some(file) = file {
                    let backup = free_backup_path(file);
                    self.copy_to(&backup)?;
                    self.v1_backup = Some(backup);
                }
                self.migrate_v1_to_v2()?;
                self.migrate_v2_to_v3()?;
            }
            2 => self.migrate_v2_to_v3()?,
            SCHEMA_VERSION => {}
            v => return Err(DbError::UnsupportedSchemaVersion(v)),
        }
        Ok(())
    }

    /// Writes a consistent copy of the database (including changes still in the WAL) to
    /// `dest`, which must not exist.
    fn copy_to(&self, dest: &Path) -> Result<()> {
        self.conn
            .execute("VACUUM INTO ?1", params![dest.to_string_lossy()])?;
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
    ///
    /// v1 stored bytes from a lossy patch model and placed imports in whatever bank was
    /// selected, so migrated rows are flagged `checksum_ok = 0`: the librarian shows ⚠ and
    /// bank send/export skip them by default. Re-import from the original files, or
    /// [`Self::mark_checked`] / [`Self::mark_bank_checked`] after checking the sounds, clears it.
    fn migrate_v1_to_v2(&self) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute_batch(MIGRATE_V1_TO_V2_SQL)?;
        tx.execute_batch(CREATE_TABLES_SQL)?;
        tx.execute(
            "INSERT INTO patches (id, bank_id, program_number, name, category1, category2, raw_data,
                                  checksum_ok, tags, favorite, created_at, updated_at)
             SELECT id, bank_id, program_number, name, 0, 0, raw_data, 0, tags, favorite,
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
    ///
    /// `checksum_ok = false` stores the patch flagged ⚠ (it came from a flagged slot or from a
    /// dump with a wrong checksum). Storing never clears a flag by itself; only
    /// [`Self::mark_checked`] and [`Self::mark_bank_checked`] do.
    pub fn save_patch(
        &self,
        bank_id: i64,
        program: u8,
        data: &[u8; PATCH_LEN],
        meta: &PatchMeta,
        checksum_ok: bool,
    ) -> Result<i64> {
        check_program(program)?;
        check_seven_bit(data)?;
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
                checksum_ok,
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

    /// Clears the ⚠ flag (`checksum_ok = 0`, e.g. a patch migrated from v1) after the user has
    /// checked the sound. The data bytes are not touched. Returns `false` if the patch doesn't
    /// exist.
    pub fn mark_checked(&self, patch_id: i64) -> Result<bool> {
        self.update_meta("checksum_ok", &true, patch_id)
    }

    /// [`Self::mark_checked`] for every flagged patch of a bank. Returns how many were cleared.
    pub fn mark_bank_checked(&self, bank_id: i64) -> Result<usize> {
        self.require_bank(bank_id)?;
        let n = self.conn.execute(
            "UPDATE patches SET checksum_ok = 1, updated_at = CURRENT_TIMESTAMP
             WHERE bank_id = ?1 AND checksum_ok = 0",
            params![bank_id],
        )?;
        if n > 0 {
            self.bump();
        }
        Ok(n)
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
    /// * Dumps with a wrong checksum are still imported (the Virus accepts them), flagged with
    ///   `checksum_ok = false` and counted in `bad_checksum`.
    /// * Dumps with data bytes ≥ 0x80 (not valid SysEx) or program > 127 are `skipped`.
    /// * If several dumps address the same slot, the later one wins and the slot is listed in
    ///   `duplicates` (counted once in `imported`).
    /// * An overwritten slot keeps its tags/favourite only if the patch name is unchanged.
    ///   Use [`Self::plan_import`] first to find out which slots that affects.
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
        let forced = forced_bank(&tx, target)?;
        let mut bank_cache: HashMap<u8, (i64, String)> = HashMap::new();
        let mut written: HashSet<(i64, u8)> = HashSet::new();
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
            if dump.program as usize >= BANK_SLOTS || first_eight_bit_offset(&dump.data).is_some() {
                report.skipped += 1;
                continue;
            }
            let Some((bank_id, bank_name)) = place(&tx, &forced, &mut bank_cache, dump.bank, true)?
            else {
                report.skipped += 1;
                continue;
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
            if !written.insert((bank_id, dump.program)) {
                report.duplicates.push(SlotRef {
                    bank_id,
                    bank_name,
                    program: dump.program,
                });
                continue;
            }
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

    /// Predicts [`Self::import_single_dumps`] without writing anything: which occupied slots
    /// would be replaced (and by what), which slots several dumps address, and how many dumps
    /// would be skipped. Use it to ask for confirmation before overwriting library data.
    pub fn plan_import(&self, dumps: &[IncomingDump], target: ImportTarget) -> Result<ImportPlan> {
        let forced = forced_bank(&self.conn, target)?;
        let mut bank_cache: HashMap<u8, (i64, String)> = HashMap::new();
        // Later dumps win, so keep the last new name per slot.
        let mut last: Vec<((i64, u8), SlotRef, String, bool)> = Vec::new();
        let mut plan = ImportPlan::default();
        for dump in dumps {
            if dump.bank == EDIT_BUFFER_BANK {
                continue;
            }
            if dump.program as usize >= BANK_SLOTS || first_eight_bit_offset(&dump.data).is_some() {
                plan.skipped += 1;
                continue;
            }
            // A bank that doesn't exist yet gets a negative key: nothing there to overwrite.
            let (key, bank_name, exists) =
                match place(&self.conn, &forced, &mut bank_cache, dump.bank, false)? {
                    Some((id, name)) => (id, name, true),
                    None if letter_for_bank_byte(dump.bank).is_some() => {
                        (-i64::from(dump.bank), format!("Bank {}", letter_for_bank_byte(dump.bank).unwrap_or("?")), false)
                    }
                    None => {
                        plan.skipped += 1;
                        continue;
                    }
                };
            let slot = SlotRef {
                bank_id: key,
                bank_name,
                program: dump.program,
            };
            let new_name = patch_name(&dump.data);
            match last.iter_mut().find(|(k, ..)| *k == (key, dump.program)) {
                Some(entry) => {
                    plan.duplicates.push(slot.clone());
                    entry.2 = new_name;
                }
                None => last.push(((key, dump.program), slot, new_name, exists)),
            }
        }
        plan.slots = last.len();
        for (_, slot, new_name, exists) in last {
            if !exists {
                continue;
            }
            if let Some(old) = self.slot_name(slot.bank_id, slot.program)? {
                plan.overwrites.push(SlotOverwrite {
                    slot,
                    old_name: old,
                    new_name,
                });
            }
        }
        Ok(plan)
    }

    fn slot_name(&self, bank_id: i64, program: u8) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT name FROM patches WHERE bank_id = ?1 AND program_number = ?2",
                params![bank_id, program],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// One 267-byte Single Dump frame per stored program, in program order, with flagged
    /// (`checksum_ok = false`) patches left out. See [`Self::export_bank`] to choose otherwise.
    pub fn export_bank_dumps(
        &self,
        bank_id: i64,
        device_id: u8,
        target_bank: Option<u8>,
    ) -> Result<Vec<Vec<u8>>> {
        Ok(self
            .export_bank(bank_id, device_id, target_bank, BadChecksumPolicy::Skip)?
            .frames)
    }

    /// Single Dump frames of a bank in program order; data bytes are sent exactly as stored.
    /// `target_bank` is the SysEx `bb` (1..=4 = A..D); `None` uses the bank's letter and fails
    /// with `NoBankLetter` for user banks. Patches flagged `checksum_ok = false` are handled
    /// according to `policy`; patches with bytes ≥ 0x80 are always left out.
    pub fn export_bank(
        &self,
        bank_id: i64,
        device_id: u8,
        target_bank: Option<u8>,
        policy: BadChecksumPolicy,
    ) -> Result<BankExport> {
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
            "SELECT id, program_number, raw_data, checksum_ok FROM patches
             WHERE bank_id = ?1 AND program_number < 128 ORDER BY program_number ASC",
        )?;
        let rows = stmt.query_map(params![bank_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, u8>(1)?,
                r.get::<_, Vec<u8>>(2)?,
                r.get::<_, bool>(3)?,
            ))
        })?;
        let mut export = BankExport::default();
        for row in rows {
            let (id, program, raw, checksum_ok) = row?;
            let data = to_patch_bytes(id, &raw)?;
            if first_eight_bit_offset(&data).is_some() {
                export.skipped_invalid += 1;
                continue;
            }
            let mut frame = encode_single_dump(device_id, bb, program, &data);
            if !checksum_ok {
                match policy {
                    BadChecksumPolicy::Skip => {
                        export.skipped_flagged += 1;
                        continue;
                    }
                    BadChecksumPolicy::KeepInvalid => {
                        let checksum = &mut frame[SINGLE_DUMP_CHECKSUM_INDEX];
                        *checksum = (*checksum + 1) & 0x7F;
                    }
                    BadChecksumPolicy::Rechecksum => {}
                }
                export.included_flagged += 1;
            }
            export.frames.push(frame);
        }
        Ok(export)
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

fn forced_bank(conn: &Connection, target: ImportTarget) -> Result<Option<BankRecord>> {
    match target {
        ImportTarget::Bank(id) => Ok(Some(
            bank_by_id(conn, id)?.ok_or(DbError::BankNotFound(id))?,
        )),
        ImportTarget::OriginalBanks => Ok(None),
    }
}

/// Resolves the library bank a dump with SysEx bank byte `bb` goes to. `None` means the
/// dump can't be placed: unknown `bb`, or (with `create == false`) the letter bank is missing.
fn place(
    conn: &Connection,
    forced: &Option<BankRecord>,
    cache: &mut HashMap<u8, (i64, String)>,
    bb: u8,
    create: bool,
) -> Result<Option<(i64, String)>> {
    if let Some(bank) = forced {
        return Ok(Some((bank.id, bank.name.clone())));
    }
    if let Some(entry) = cache.get(&bb) {
        return Ok(Some(entry.clone()));
    }
    let Some(letter) = letter_for_bank_byte(bb) else {
        return Ok(None);
    };
    let bank = match bank_by_letter(conn, letter)? {
        Some(bank) => bank,
        None if create => {
            let id = insert_letter_bank(conn, letter)?;
            bank_by_id(conn, id)?.ok_or(DbError::BankNotFound(id))?
        }
        None => return Ok(None),
    };
    let entry = (bank.id, bank.name);
    cache.insert(bb, entry.clone());
    Ok(Some(entry))
}

fn check_program(program: u8) -> Result<()> {
    if (program as usize) < BANK_SLOTS {
        Ok(())
    } else {
        Err(DbError::InvalidProgram(program))
    }
}

fn check_seven_bit(data: &[u8; PATCH_LEN]) -> Result<()> {
    match first_eight_bit_offset(data) {
        Some(offset) => Err(DbError::NotSevenBit { offset }),
        None => Ok(()),
    }
}

/// `<file>.v1.bak`, or `<file>.v1.bak.2`, `.3`… if earlier backups exist.
fn free_backup_path(file: &Path) -> PathBuf {
    let base = file.with_extension("v1.bak");
    let mut candidate = base.clone();
    let mut n = 1;
    while candidate.exists() {
        n += 1;
        let mut name = base.clone().into_os_string();
        name.push(format!(".{n}"));
        candidate = PathBuf::from(name);
    }
    candidate
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

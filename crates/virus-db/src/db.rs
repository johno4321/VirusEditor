//! SQLite database operations for the Access Virus patch librarian.

use std::path::Path;
use rusqlite::{params, Connection, OptionalExtension, ToSql};
use virus_syx::domain::checksum::calculate_checksum;
use virus_syx::domain::patch::Patch;

use crate::models::{BankRecord, PatchFilter, PatchRecord, PatchSummary, SoundCategory};
use crate::schema::CREATE_TABLES_SQL;

/// Thread-safe or local SQLite database manager for banks and patches.
pub struct PatchDatabase {
    conn: Connection,
}

impl PatchDatabase {
    /// Opens or creates a SQLite database at the specified path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.init()?;
        Ok(db)
    }

    /// Creates an in-memory SQLite database, ideal for testing and ephemeral sessions.
    pub fn open_in_memory() -> Result<Self, rusqlite::Error> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn };
        db.init()?;
        Ok(db)
    }

    /// Initializes PRAGMAs, creates schema tables and seeds default banks.
    fn init(&self) -> Result<(), rusqlite::Error> {
        self.conn.execute_batch("
            PRAGMA foreign_keys = ON;
            PRAGMA journal_mode = WAL;
        ")?;
        self.conn.execute_batch(CREATE_TABLES_SQL)?;
        self.seed_default_banks()?;
        Ok(())
    }

    /// Seeds standard Access Virus banks (A, B, C, D) if no banks exist.
    pub fn seed_default_banks(&self) -> Result<(), rusqlite::Error> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM banks", [], |row| row.get(0))?;

        if count == 0 {
            self.create_bank("Bank A", Some("A"), true)?;
            self.create_bank("Bank B", Some("B"), true)?;
            self.create_bank("Bank C (User)", Some("C"), false)?;
            self.create_bank("Bank D (User)", Some("D"), false)?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------------
    // Bank Operations
    // ------------------------------------------------------------------------

    /// Creates a new bank record and returns its ID.
    pub fn create_bank(
        &self,
        name: &str,
        bank_letter: Option<&str>,
        is_factory: bool,
    ) -> Result<i64, rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO banks (name, bank_letter, is_factory) VALUES (?1, ?2, ?3)",
            params![name, bank_letter, is_factory as i32],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Retrieves all banks ordered by ID.
    pub fn get_banks(&self) -> Result<Vec<BankRecord>, rusqlite::Error> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, bank_letter, is_factory FROM banks ORDER BY id ASC")?;

        let rows = stmt.query_map([], |row| {
            let is_factory_int: i32 = row.get(3)?;
            Ok(BankRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                bank_letter: row.get(2)?,
                is_factory: is_factory_int != 0,
            })
        })?;

        let mut banks = Vec::new();
        for row in rows {
            banks.push(row?);
        }
        Ok(banks)
    }

    /// Retrieves a bank by its ID.
    pub fn get_bank_by_id(&self, id: i64) -> Result<Option<BankRecord>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT id, name, bank_letter, is_factory FROM banks WHERE id = ?1",
                params![id],
                |row| {
                    let is_factory_int: i32 = row.get(3)?;
                    Ok(BankRecord {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        bank_letter: row.get(2)?,
                        is_factory: is_factory_int != 0,
                    })
                },
            )
            .optional()
    }

    /// Deletes a bank and all its associated patches (via CASCADE).
    pub fn delete_bank(&self, id: i64) -> Result<bool, rusqlite::Error> {
        let count = self
            .conn
            .execute("DELETE FROM banks WHERE id = ?1", params![id])?;
        Ok(count > 0)
    }

    // ------------------------------------------------------------------------
    // Patch Operations
    // ------------------------------------------------------------------------

    /// Inserts or updates a patch in the given bank slot (0..=127).
    pub fn save_patch(
        &self,
        bank_id: i64,
        program_number: u8,
        patch: &mut Patch,
        category1: SoundCategory,
        category2: SoundCategory,
        tags: &str,
        favorite: bool,
    ) -> Result<i64, rusqlite::Error> {
        let name = patch.name();
        let raw_data = patch.to_bytes();
        let checksum = calculate_checksum(&raw_data);

        self.conn.execute(
            r#"
            INSERT INTO patches (
                bank_id, program_number, name, category1, category2,
                raw_data, checksum, tags, favorite, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, CURRENT_TIMESTAMP)
            ON CONFLICT(bank_id, program_number) DO UPDATE SET
                name = excluded.name,
                category1 = excluded.category1,
                category2 = excluded.category2,
                raw_data = excluded.raw_data,
                checksum = excluded.checksum,
                tags = excluded.tags,
                favorite = excluded.favorite,
                updated_at = CURRENT_TIMESTAMP
            "#,
            params![
                bank_id,
                program_number as i32,
                name,
                category1.as_str(),
                category2.as_str(),
                &raw_data[..],
                checksum as i32,
                tags,
                favorite as i32,
            ],
        )?;

        // Return the rowid of the inserted/updated record
        let id: i64 = self.conn.query_row(
            "SELECT id FROM patches WHERE bank_id = ?1 AND program_number = ?2",
            params![bank_id, program_number as i32],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    /// Retrieves a patch by its database ID.
    pub fn get_patch_by_id(&self, id: i64) -> Result<Option<PatchRecord>, rusqlite::Error> {
        self.conn
            .query_row(
                r#"
                SELECT id, bank_id, program_number, name, category1, category2,
                       raw_data, checksum, tags, favorite
                FROM patches WHERE id = ?1
                "#,
                params![id],
                |row| Self::row_to_patch_record(row),
            )
            .optional()
    }

    /// Retrieves a patch by its bank ID and program number (0..=127).
    pub fn get_patch_by_slot(
        &self,
        bank_id: i64,
        program_number: u8,
    ) -> Result<Option<PatchRecord>, rusqlite::Error> {
        self.conn
            .query_row(
                r#"
                SELECT id, bank_id, program_number, name, category1, category2,
                       raw_data, checksum, tags, favorite
                FROM patches WHERE bank_id = ?1 AND program_number = ?2
                "#,
                params![bank_id, program_number as i32],
                |row| Self::row_to_patch_record(row),
            )
            .optional()
    }

    /// Deletes a patch by database ID.
    pub fn delete_patch(&self, id: i64) -> Result<bool, rusqlite::Error> {
        let count = self
            .conn
            .execute("DELETE FROM patches WHERE id = ?1", params![id])?;
        Ok(count > 0)
    }

    /// Queries patch summaries matching the specified filter criteria.
    pub fn query_patches(&self, filter: &PatchFilter) -> Result<Vec<PatchSummary>, rusqlite::Error> {
        let mut query = String::from(
            "SELECT id, bank_id, program_number, name, category1, category2, tags, favorite FROM patches WHERE 1=1"
        );
        let mut params_vec: Vec<Box<dyn ToSql>> = Vec::new();

        if let Some(bank_id) = filter.bank_id {
            query.push_str(" AND bank_id = ?");
            params_vec.push(Box::new(bank_id));
        }

        if filter.favorite_only {
            query.push_str(" AND favorite = 1");
        }

        if let Some(category) = filter.category {
            if category != SoundCategory::Off {
                query.push_str(" AND (category1 = ? OR category2 = ?)");
                let cat_str = category.as_str().to_string();
                params_vec.push(Box::new(cat_str.clone()));
                params_vec.push(Box::new(cat_str));
            }
        }

        if let Some(search) = &filter.search_text {
            let trimmed = search.trim();
            if !trimmed.is_empty() {
                query.push_str(" AND (name LIKE ? OR tags LIKE ?)");
                let pattern = format!("%{}%", trimmed);
                params_vec.push(Box::new(pattern.clone()));
                params_vec.push(Box::new(pattern));
            }
        }

        query.push_str(" ORDER BY bank_id ASC, program_number ASC");

        let mut stmt = self.conn.prepare(&query)?;
        let param_refs: Vec<&dyn ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();

        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            let cat1_str: String = row.get(4)?;
            let cat2_str: String = row.get(5)?;
            let fav_int: i32 = row.get(7)?;

            Ok(PatchSummary {
                id: row.get(0)?,
                bank_id: row.get(1)?,
                program_number: row.get::<_, i32>(2)? as u8,
                name: row.get(3)?,
                category1: SoundCategory::from_str(&cat1_str),
                category2: SoundCategory::from_str(&cat2_str),
                tags: row.get(6)?,
                favorite: fav_int != 0,
            })
        })?;

        let mut summaries = Vec::new();
        for r in rows {
            summaries.push(r?);
        }
        Ok(summaries)
    }

    /// Imports a `.syx` or `.mid` bank file containing one or more Single Program Dump messages.
    /// Stores the patches in the designated `bank_id`. Returns the count of imported patches.
    pub fn import_syx_bank(&self, bank_id: i64, file_bytes: &[u8]) -> Result<usize, String> {
        let mut imported_count = 0;
        let mut idx = 0;
        let sig = [0x00, 0x20, 0x33, 0x01]; // Access Virus signature

        while idx + 4 <= file_bytes.len() {
            if file_bytes[idx..].starts_with(&sig) {
                if let Some(end_rel) = file_bytes[idx..].iter().position(|&b| b == 0xF7) {
                    let end = idx + end_rel + 1;
                    let payload = &file_bytes[idx..end];
                    let mut frame = Vec::with_capacity(payload.len() + 1);
                    frame.push(0xF0);
                    frame.extend_from_slice(payload);

                    if let Ok((mut patch, _bank, prog)) = Patch::from_sysex(&frame) {
                        self.save_patch(
                            bank_id,
                            prog,
                            &mut patch,
                            SoundCategory::Off,
                            SoundCategory::Off,
                            "",
                            false,
                        )
                        .map_err(|e| format!("Database save error: {}", e))?;
                        imported_count += 1;
                    }
                    idx = end;
                    continue;
                }
            }
            idx += 1;
        }

        Ok(imported_count)
    }

    /// Exports all patches in the specified bank as a concatenated `.syx` byte stream.
    pub fn export_syx_bank(&self, bank_id: i64, device_id: u8) -> Result<Vec<u8>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT raw_data, program_number FROM patches WHERE bank_id = ?1 ORDER BY program_number ASC",
            )
            .map_err(|e| format!("DB prepare error: {}", e))?;

        let rows = stmt
            .query_map(params![bank_id], |row| {
                let raw: Vec<u8> = row.get(0)?;
                let prog: i32 = row.get(1)?;
                Ok((raw, prog as u8))
            })
            .map_err(|e| format!("DB query error: {}", e))?;

        let mut syx_out = Vec::new();

        // Bank letter mapping or index
        let bank_record = self
            .get_bank_by_id(bank_id)
            .map_err(|e| format!("DB error: {}", e))?
            .ok_or_else(|| "Bank not found".to_string())?;

        let bank_num: u8 = match bank_record.bank_letter.as_deref() {
            Some("A") => 0,
            Some("B") => 1,
            Some("C") => 2,
            Some("D") => 3,
            _ => 0,
        };

        for item in rows {
            let (raw, prog) = item.map_err(|e| format!("Row error: {}", e))?;
            if raw.len() == 256 {
                let mut bytes_256 = [0u8; 256];
                bytes_256.copy_from_slice(&raw);
                let mut patch = Patch::from_bytes(&bytes_256);
                let dump = patch.to_sysex(device_id, bank_num, prog);
                syx_out.extend_from_slice(&dump);
            }
        }

        Ok(syx_out)
    }

    // Helper to map a row to PatchRecord
    fn row_to_patch_record(row: &rusqlite::Row) -> Result<PatchRecord, rusqlite::Error> {
        let raw_data: Vec<u8> = row.get(6)?;
        let mut patch_bytes = [0u8; 256];
        if raw_data.len() == 256 {
            patch_bytes.copy_from_slice(&raw_data);
        }
        let patch = Patch::from_bytes(&patch_bytes);

        let cat1_str: String = row.get(4)?;
        let cat2_str: String = row.get(5)?;
        let fav_int: i32 = row.get(9)?;

        Ok(PatchRecord {
            id: row.get(0)?,
            bank_id: row.get(1)?,
            program_number: row.get::<_, i32>(2)? as u8,
            name: row.get(3)?,
            category1: SoundCategory::from_str(&cat1_str),
            category2: SoundCategory::from_str(&cat2_str),
            patch,
            checksum: row.get::<_, i32>(7)? as u8,
            tags: row.get(8)?,
            favorite: fav_int != 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_init_and_seed() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let banks = db.get_banks().unwrap();
        assert_eq!(banks.len(), 4);
        assert_eq!(banks[0].name, "Bank A");
        assert_eq!(banks[0].bank_letter.as_deref(), Some("A"));
        assert!(banks[0].is_factory);
        assert_eq!(banks[2].name, "Bank C (User)");
        assert!(!banks[2].is_factory);
    }

    #[test]
    fn test_patch_save_and_retrieve() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let bank_a_id = db.get_banks().unwrap()[0].id;

        let mut patch = Patch::new("AnalogLead");
        let patch_id = db
            .save_patch(
                bank_a_id,
                5,
                &mut patch,
                SoundCategory::Lead,
                SoundCategory::Classic,
                "warm, analog, solo",
                true,
            )
            .unwrap();

        let retrieved = db.get_patch_by_id(patch_id).unwrap().unwrap();
        assert_eq!(retrieved.name, "AnalogLead");
        assert_eq!(retrieved.program_number, 5);
        assert_eq!(retrieved.category1, SoundCategory::Lead);
        assert_eq!(retrieved.category2, SoundCategory::Classic);
        assert_eq!(retrieved.tags, "warm, analog, solo");
        assert!(retrieved.favorite);

        // Retrieve by slot
        let slot_retrieved = db.get_patch_by_slot(bank_a_id, 5).unwrap().unwrap();
        assert_eq!(slot_retrieved.id, patch_id);
    }

    #[test]
    fn test_patch_query_filters() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let bank_id = db.get_banks().unwrap()[0].id;

        let mut p1 = Patch::new("Acid 303");
        db.save_patch(bank_id, 0, &mut p1, SoundCategory::Acid, SoundCategory::Bass, "tb303, squelch", false).unwrap();

        let mut p2 = Patch::new("Moog Bass");
        db.save_patch(bank_id, 1, &mut p2, SoundCategory::Bass, SoundCategory::Classic, "fat, 24db", true).unwrap();

        let mut p3 = Patch::new("HyperPad");
        db.save_patch(bank_id, 2, &mut p3, SoundCategory::Pad, SoundCategory::Off, "ambient, lush", false).unwrap();

        // Filter by category Bass
        let bass_filter = PatchFilter {
            category: Some(SoundCategory::Bass),
            ..Default::default()
        };
        let bass_results = db.query_patches(&bass_filter).unwrap();
        assert_eq!(bass_results.len(), 2);

        // Filter by favorites only
        let fav_filter = PatchFilter {
            favorite_only: true,
            ..Default::default()
        };
        let fav_results = db.query_patches(&fav_filter).unwrap();
        assert_eq!(fav_results.len(), 1);
        assert_eq!(fav_results[0].name, "Moog Bass");

        // Search text "303"
        let search_filter = PatchFilter {
            search_text: Some("303".to_string()),
            ..Default::default()
        };
        let search_results = db.query_patches(&search_filter).unwrap();
        assert_eq!(search_results.len(), 1);
        assert_eq!(search_results[0].name, "Acid 303");
    }

    #[test]
    fn test_sysex_import_and_export() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let banks = db.get_banks().unwrap();
        let bank_a_id = banks[0].id;
        let bank_b_id = banks[1].id;

        let mut p0 = Patch::new("PatchZero");
        let mut p1 = Patch::new("PatchOne");

        db.save_patch(bank_a_id, 0, &mut p0, SoundCategory::Lead, SoundCategory::Off, "", false).unwrap();
        db.save_patch(bank_a_id, 1, &mut p1, SoundCategory::Pad, SoundCategory::Off, "", false).unwrap();

        // Export Bank A to SysEx bytes
        let syx_bytes = db.export_syx_bank(bank_a_id, 0x01).unwrap();
        assert_eq!(syx_bytes.len(), 267 * 2);

        // Import SysEx into Bank B
        let count = db.import_syx_bank(bank_b_id, &syx_bytes).unwrap();
        assert_eq!(count, 2);

        let imported_p0 = db.get_patch_by_slot(bank_b_id, 0).unwrap().unwrap();
        assert_eq!(imported_p0.name, "PatchZero");

        let imported_p1 = db.get_patch_by_slot(bank_b_id, 1).unwrap().unwrap();
        assert_eq!(imported_p1.name, "PatchOne");
    }

    #[test]
    fn test_import_midi_file_wrapped_sysex() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let bank_id = db.get_banks().unwrap()[0].id;

        let mut p = Patch::new("SmfPatch");
        let raw_syx = p.to_sysex(0x01, 0, 10);

        // Wrap in MIDI File chunk style: [Delta, 0xF0, VarLen, data..., 0xF7]
        let mut smf_bytes = vec![0x00, 0xF0, 0x82, 0x0B];
        smf_bytes.extend_from_slice(&raw_syx[1..]); // Skip the leading 0xF0 since SMF has it before varlen

        let count = db.import_syx_bank(bank_id, &smf_bytes).unwrap();
        assert_eq!(count, 1);

        let retrieved = db.get_patch_by_slot(bank_id, 10).unwrap().unwrap();
        assert_eq!(retrieved.name, "SmfPatch");
    }
}

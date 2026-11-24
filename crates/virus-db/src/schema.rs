//! SQLite DDL for the patch librarian. The version is tracked in `PRAGMA user_version`.
//!
//! * v1: original schema (text categories, `is_factory`), `user_version` was left at 0.
//! * v2: integer categories, `checksum_ok` flag, `is_rom` per bank.
//! * v3: `multis` table (the Multi bank's 128 slots plus unslotted multis).

pub const SCHEMA_VERSION: i64 = 3;

/// Banks and single patches (unchanged since v2).
pub const CREATE_TABLES_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS banks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    bank_letter TEXT,
    is_rom INTEGER NOT NULL DEFAULT 0,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS patches (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    bank_id INTEGER NOT NULL REFERENCES banks(id) ON DELETE CASCADE,
    program_number INTEGER NOT NULL,
    name TEXT NOT NULL,
    category1 INTEGER NOT NULL DEFAULT 0,
    category2 INTEGER NOT NULL DEFAULT 0,
    raw_data BLOB NOT NULL,
    checksum_ok INTEGER NOT NULL DEFAULT 1,
    tags TEXT NOT NULL DEFAULT '',
    favorite INTEGER NOT NULL DEFAULT 0,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(bank_id, program_number)
);

CREATE INDEX IF NOT EXISTS idx_patches_name ON patches(name COLLATE NOCASE);
CREATE INDEX IF NOT EXISTS idx_patches_cat1 ON patches(category1);
CREATE INDEX IF NOT EXISTS idx_patches_cat2 ON patches(category2);
CREATE INDEX IF NOT EXISTS idx_patches_fav ON patches(favorite);
"#;

/// v3: multis. `slot` 0..=127 is the Multi bank program; `NULL` = an unslotted multi (e.g. an
/// edit-buffer dump the user chose to keep). Applied by the v2 → v3 migration and on new databases.
pub const CREATE_MULTI_TABLES_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS multis (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    slot INTEGER CHECK (slot IS NULL OR (slot >= 0 AND slot <= 127)),
    name TEXT NOT NULL,
    raw_data BLOB NOT NULL,
    checksum_ok INTEGER NOT NULL DEFAULT 1,
    tags TEXT NOT NULL DEFAULT '',
    favorite INTEGER NOT NULL DEFAULT 0,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_multis_slot ON multis(slot) WHERE slot IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_multis_name ON multis(name COLLATE NOCASE);
"#;

/// First half of the v1 → v2 migration. Patch rows are then copied from `patches_v1`
/// in Rust (to re-derive name/categories from the bytes) and `patches_v1` is dropped.
pub(crate) const MIGRATE_V1_TO_V2_SQL: &str = r#"
DROP INDEX IF EXISTS idx_patches_bank_prog;
DROP INDEX IF EXISTS idx_patches_name;
DROP INDEX IF EXISTS idx_patches_cat1;
DROP INDEX IF EXISTS idx_patches_cat2;
DROP INDEX IF EXISTS idx_patches_fav;
ALTER TABLE patches RENAME TO patches_v1;

ALTER TABLE banks ADD COLUMN is_rom INTEGER NOT NULL DEFAULT 0;
UPDATE banks SET is_rom = CASE WHEN bank_letter IN ('C', 'D') THEN 1 ELSE 0 END;
UPDATE banks SET name = 'Bank C' WHERE name = 'Bank C (User)' AND bank_letter = 'C';
UPDATE banks SET name = 'Bank D' WHERE name = 'Bank D (User)' AND bank_letter = 'D';
ALTER TABLE banks DROP COLUMN is_factory;
"#;

#[cfg(test)]
pub(crate) const SCHEMA_V1_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS banks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    bank_letter TEXT,
    is_factory INTEGER NOT NULL DEFAULT 0,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS patches (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    bank_id INTEGER NOT NULL REFERENCES banks(id) ON DELETE CASCADE,
    program_number INTEGER NOT NULL,
    name TEXT NOT NULL,
    category1 TEXT NOT NULL DEFAULT 'Off',
    category2 TEXT NOT NULL DEFAULT 'Off',
    raw_data BLOB NOT NULL,
    checksum INTEGER NOT NULL,
    tags TEXT NOT NULL DEFAULT '',
    favorite INTEGER NOT NULL DEFAULT 0,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(bank_id, program_number)
);

CREATE INDEX IF NOT EXISTS idx_patches_bank_prog ON patches(bank_id, program_number);
CREATE INDEX IF NOT EXISTS idx_patches_name ON patches(name COLLATE NOCASE);
CREATE INDEX IF NOT EXISTS idx_patches_cat1 ON patches(category1);
CREATE INDEX IF NOT EXISTS idx_patches_cat2 ON patches(category2);
CREATE INDEX IF NOT EXISTS idx_patches_fav ON patches(favorite);
"#;

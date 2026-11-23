//! SQLite DDL schema definitions for the Access Virus patch librarian.

pub const CREATE_TABLES_SQL: &str = r#"
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

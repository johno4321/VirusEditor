//! Opens the patch library in the user's data directory (D8).

use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use virus_db::PatchDatabase;

pub const DB_FILE_NAME: &str = "virus_library.db";

/// Where the library lives for this session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryLocation {
    File(PathBuf),
    /// The file couldn't be opened; nothing will be saved.
    InMemory { reason: String },
}

/// `…/VirusEditor/data` in the platform's per-user data directory (library and settings).
pub fn data_dir() -> Result<PathBuf, String> {
    ProjectDirs::from("", "", "VirusEditor")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .ok_or_else(|| "no user data directory could be determined".to_owned())
}

/// `…/VirusEditor/data/virus_library.db`.
pub fn default_db_path() -> Result<PathBuf, String> {
    data_dir().map(|dir| dir.join(DB_FILE_NAME))
}

pub fn open_library() -> (PatchDatabase, LibraryLocation) {
    open_library_at(default_db_path())
}

/// Opens (creating its folder if needed) the database at `path`, falling back to an
/// in-memory database with the reason when that fails.
pub fn open_library_at(path: Result<PathBuf, String>) -> (PatchDatabase, LibraryLocation) {
    match path.and_then(|p| open_file(&p).map(|db| (db, p))) {
        Ok((db, path)) => (db, LibraryLocation::File(path)),
        Err(reason) => {
            let db = PatchDatabase::open_in_memory().expect("an in-memory SQLite database can always be opened");
            (db, LibraryLocation::InMemory { reason })
        }
    }
}

fn open_file(path: &Path) -> Result<PatchDatabase, String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    PatchDatabase::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))
}

/// Where the v1 app may have left its `virus_library.db`: it opened the file relative to the
/// working directory, so look there and next to the executable.
pub fn legacy_candidates() -> Vec<PathBuf> {
    let mut found = Vec::new();
    if let Ok(dir) = std::env::current_dir() {
        found.push(dir.join(DB_FILE_NAME));
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
        found.push(dir.join(DB_FILE_NAME));
    }
    found
}

/// The first existing candidate that isn't `current` itself.
pub fn find_legacy_library(current: &Path, candidates: &[PathBuf]) -> Option<PathBuf> {
    let current = std::fs::canonicalize(current).unwrap_or_else(|_| current.to_path_buf());
    candidates
        .iter()
        .filter(|c| c.is_file())
        .find(|c| std::fs::canonicalize(c).map_or(true, |c| c != current))
        .cloned()
}

/// A legacy library worth offering: only while the new library is still empty.
pub fn pending_legacy_library(db: &PatchDatabase, location: &LibraryLocation) -> Option<PathBuf> {
    let LibraryLocation::File(path) = location else { return None };
    if db.patch_count().ok()? > 0 {
        return None;
    }
    find_legacy_library(path, &legacy_candidates())
}

/// Replaces the (empty) library at `target` with a copy of `legacy` and opens it, which
/// migrates it (and backs the copy up as `.v1.bak`). `legacy` itself is never modified, so
/// it stays as the user's backup. Close any open handle on `target` first.
pub fn adopt_legacy_library(legacy: &Path, target: &Path) -> Result<PatchDatabase, String> {
    if target.exists() {
        let existing = PatchDatabase::open(target).map_err(|e| format!("cannot open {}: {e}", target.display()))?;
        let count = existing.patch_count().map_err(|e| e.to_string())?;
        drop(existing);
        if count > 0 {
            return Err(format!("the library at {} already contains {count} patch(es)", target.display()));
        }
        for suffix in ["", "-wal", "-shm"] {
            let mut name = target.as_os_str().to_owned();
            name.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(name));
        }
    }
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    PatchDatabase::copy_file(legacy, target).map_err(|e| format!("cannot copy {}: {e}", legacy.display()))?;
    PatchDatabase::open(target).map_err(|e| format!("cannot open {}: {e}", target.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("virus-app-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn finds_and_adopts_a_legacy_v1_library() {
        let dir = temp_dir("storage-legacy");
        std::fs::create_dir_all(&dir).unwrap();
        let legacy = dir.join("old").join(DB_FILE_NAME);
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        {
            let conn = rusqlite::Connection::open(&legacy).unwrap();
            conn.execute_batch(
                "CREATE TABLE banks (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, bank_letter TEXT,
                    is_factory INTEGER NOT NULL DEFAULT 0, created_at DATETIME);
                 CREATE TABLE patches (id INTEGER PRIMARY KEY AUTOINCREMENT, bank_id INTEGER NOT NULL,
                    program_number INTEGER NOT NULL, name TEXT NOT NULL, category1 TEXT NOT NULL DEFAULT 'Off',
                    category2 TEXT NOT NULL DEFAULT 'Off', raw_data BLOB NOT NULL, checksum INTEGER NOT NULL,
                    tags TEXT NOT NULL DEFAULT '', favorite INTEGER NOT NULL DEFAULT 0,
                    created_at DATETIME, updated_at DATETIME, UNIQUE(bank_id, program_number));
                 INSERT INTO banks (name, bank_letter) VALUES ('Bank A', 'A');",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO patches (bank_id, program_number, name, raw_data, checksum) VALUES (1, 3, 'Old', ?1, 0)",
                [vec![0u8; 256]],
            )
            .unwrap();
        }

        let target = dir.join("new").join(DB_FILE_NAME);
        let (db, location) = open_library_at(Ok(target.clone()));
        assert_eq!(db.patch_count().unwrap(), 0);
        assert_eq!(find_legacy_library(&target, std::slice::from_ref(&legacy)), Some(legacy.clone()));
        assert_eq!(find_legacy_library(&legacy, std::slice::from_ref(&legacy)), None, "the library itself is not legacy");
        assert_eq!(location, LibraryLocation::File(target.clone()));
        drop(db);

        let adopted = adopt_legacy_library(&legacy, &target).unwrap();
        assert_eq!(adopted.patch_count().unwrap(), 1);
        assert_eq!(adopted.schema_version().unwrap(), virus_db::SCHEMA_VERSION);
        assert!(adopted.v1_backup().is_some_and(Path::exists));
        assert!(legacy.exists(), "the original stays as a backup");
        drop(adopted);

        // A library that already has data is never replaced.
        let err = adopt_legacy_library(&legacy, &target).err().expect("must refuse a non-empty library");
        assert!(err.contains("already contains"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn opens_file_and_creates_folder() {
        let dir = temp_dir("storage-ok");
        let path = dir.join("nested").join(DB_FILE_NAME);
        let (db, location) = open_library_at(Ok(path.clone()));
        assert_eq!(location, LibraryLocation::File(path.clone()));
        assert_eq!(db.get_banks().unwrap().len(), 4);
        drop(db);
        assert!(path.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn falls_back_to_memory_with_reason() {
        let dir = temp_dir("storage-bad");
        std::fs::create_dir_all(&dir).unwrap();
        let blocker = dir.join("not-a-folder");
        std::fs::write(&blocker, b"x").unwrap();
        let (db, location) = open_library_at(Ok(blocker.join(DB_FILE_NAME)));
        assert!(matches!(location, LibraryLocation::InMemory { ref reason } if reason.contains("cannot create")));
        assert_eq!(db.get_banks().unwrap().len(), 4);

        let (_, location) = open_library_at(Err("no home".into()));
        assert_eq!(location, LibraryLocation::InMemory { reason: "no home".into() });
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

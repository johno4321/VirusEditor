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

/// A legacy library worth offering: only while the new library is still empty (no patches, no
/// multis, no user banks), and not the file the user already answered "Not now" for.
pub fn pending_legacy_library(db: &PatchDatabase, location: &LibraryLocation, dismissed: Option<&str>) -> Option<PathBuf> {
    let LibraryLocation::File(path) = location else { return None };
    if !db.is_empty_library().ok()? {
        return None;
    }
    find_legacy_library(path, &legacy_candidates()).filter(|found| !is_dismissed(found, dismissed))
}

/// Whether `legacy` is the file stored by "Not now" (compared as written and canonicalised).
pub fn is_dismissed(legacy: &Path, dismissed: Option<&str>) -> bool {
    let Some(dismissed) = dismissed.map(Path::new) else { return false };
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    legacy == dismissed || canonical(legacy) == canonical(dismissed)
}

/// `<db>.pre-adopt.bak`, or `.pre-adopt.bak.2`, `.3`… if earlier backups exist.
fn free_pre_adopt_path(target: &Path) -> PathBuf {
    let mut base = target.as_os_str().to_owned();
    base.push(".pre-adopt.bak");
    let base = PathBuf::from(base);
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

/// A legacy library copied into place by [`adopt_legacy_library`].
pub struct Adopted {
    pub db: PatchDatabase,
    /// Copy of the library that was replaced, if there was one.
    pub pre_adopt_backup: Option<PathBuf>,
}

/// Replaces the (empty) library at `target` with a copy of `legacy` and opens it, which
/// migrates it (and backs the copy up as `.v1.bak`). `legacy` itself is never modified, so
/// it stays as the user's backup. The replaced library is first copied to
/// `<target>.pre-adopt.bak`, and a library with patches, multis or user banks is never
/// replaced (CR-M5). Close any open handle on `target` first.
pub fn adopt_legacy_library(legacy: &Path, target: &Path) -> Result<Adopted, String> {
    let mut pre_adopt_backup = None;
    if target.exists() {
        let existing = PatchDatabase::open(target).map_err(|e| format!("cannot open {}: {e}", target.display()))?;
        let empty = existing.is_empty_library().map_err(|e| e.to_string())?;
        drop(existing);
        if !empty {
            return Err(format!(
                "the library at {} already contains patches, multis or user banks",
                target.display()
            ));
        }
        let backup = free_pre_adopt_path(target);
        PatchDatabase::copy_file(target, &backup)
            .map_err(|e| format!("cannot back up {} to {}: {e}", target.display(), backup.display()))?;
        pre_adopt_backup = Some(backup);
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
    let db = PatchDatabase::open(target).map_err(|e| format!("cannot open {}: {e}", target.display()))?;
    Ok(Adopted { db, pre_adopt_backup })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("virus-app-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// An old v1 library file with one patch in bank A.
    fn write_v1_library(legacy: &Path) {
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        let conn = rusqlite::Connection::open(legacy).unwrap();
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

    #[test]
    fn finds_and_adopts_a_legacy_v1_library() {
        let dir = temp_dir("storage-legacy");
        std::fs::create_dir_all(&dir).unwrap();
        let legacy = dir.join("old").join(DB_FILE_NAME);
        write_v1_library(&legacy);

        let target = dir.join("new").join(DB_FILE_NAME);
        let (db, location) = open_library_at(Ok(target.clone()));
        assert_eq!(db.patch_count().unwrap(), 0);
        assert_eq!(find_legacy_library(&target, std::slice::from_ref(&legacy)), Some(legacy.clone()));
        assert_eq!(find_legacy_library(&legacy, std::slice::from_ref(&legacy)), None, "the library itself is not legacy");
        assert_eq!(location, LibraryLocation::File(target.clone()));
        drop(db);

        let adopted = adopt_legacy_library(&legacy, &target).unwrap();
        assert_eq!(adopted.db.patch_count().unwrap(), 1);
        assert_eq!(adopted.db.schema_version().unwrap(), virus_db::SCHEMA_VERSION);
        assert!(adopted.db.v1_backup().is_some_and(Path::exists));
        assert!(legacy.exists(), "the original stays as a backup");
        drop(adopted);

        // A library that already has data is never replaced.
        let err = adopt_legacy_library(&legacy, &target).err().expect("must refuse a non-empty library");
        assert!(err.contains("already contains"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_replaced_library_is_backed_up_before_adopting() {
        let dir = temp_dir("storage-pre-adopt");
        let legacy = dir.join("old").join(DB_FILE_NAME);
        write_v1_library(&legacy);
        let target = dir.join("new").join(DB_FILE_NAME);
        drop(open_library_at(Ok(target.clone())));

        let adopted = adopt_legacy_library(&legacy, &target).unwrap();
        let backup = adopted.pre_adopt_backup.clone().expect("a backup of the replaced library");
        assert_eq!(backup, dir.join("new").join("virus_library.db.pre-adopt.bak"));
        drop(adopted);
        let old = PatchDatabase::open(&backup).unwrap();
        assert!(old.is_empty_library().unwrap(), "the backup is the library that was replaced");
        drop(old);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_library_with_only_multis_is_never_replaced() {
        let dir = temp_dir("storage-multis-only");
        let legacy = dir.join("old").join(DB_FILE_NAME);
        write_v1_library(&legacy);
        let target = dir.join("new").join(DB_FILE_NAME);
        let (db, _) = open_library_at(Ok(target.clone()));
        db.save_multi(Some(0), &[0u8; virus_db::MULTI_LEN], &virus_db::PatchMeta::default(), true).unwrap();
        drop(db);

        assert!(adopt_legacy_library(&legacy, &target).is_err());
        let kept = PatchDatabase::open(&target).unwrap();
        assert!(kept.get_multi_by_slot(0).unwrap().is_some(), "the multi survives");
        drop(kept);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_library_with_only_a_user_bank_is_never_replaced() {
        let dir = temp_dir("storage-user-bank-only");
        let legacy = dir.join("old").join(DB_FILE_NAME);
        write_v1_library(&legacy);
        let target = dir.join("new").join(DB_FILE_NAME);
        let (db, _) = open_library_at(Ok(target.clone()));
        db.create_bank("From synth: Bank A").unwrap();
        drop(db);

        assert!(adopt_legacy_library(&legacy, &target).is_err());
        let kept = PatchDatabase::open(&target).unwrap();
        assert_eq!(kept.get_banks().unwrap().len(), 5);
        drop(kept);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn not_now_is_remembered_for_that_file_only() {
        let legacy = Path::new("C:\\old\\virus_library.db");
        assert!(is_dismissed(legacy, Some("C:\\old\\virus_library.db")));
        assert!(!is_dismissed(legacy, Some("C:\\other\\virus_library.db")));
        assert!(!is_dismissed(legacy, None));
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

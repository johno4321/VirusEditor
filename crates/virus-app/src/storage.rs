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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("virus-app-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
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

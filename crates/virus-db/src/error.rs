//! Error type for the patch librarian.

use std::fmt;

/// Errors returned by [`crate::PatchDatabase`].
#[derive(Debug)]
pub enum DbError {
    /// Underlying SQLite failure.
    Sqlite(rusqlite::Error),
    /// A stored patch blob is not exactly 256 bytes long.
    CorruptPatch { id: i64, len: usize },
    /// A stored multi blob is not exactly 256 bytes long.
    CorruptMulti { id: i64, len: usize },
    /// No bank with this ID exists.
    BankNotFound(i64),
    /// Program numbers must be 0..=127.
    InvalidProgram(u8),
    /// The bank has no letter A..D, so a target bank byte must be supplied for export.
    NoBankLetter(i64),
    /// A SysEx target bank byte outside 1..=4 (Single Bank A..D) was requested.
    InvalidTargetBank(u8),
    /// `ImportTarget::Bank` was used with dumps coming from several source banks.
    MixedSourceBanks(Vec<u8>),
    /// The database was written by a newer version of the application.
    UnsupportedSchemaVersion(i64),
}

pub type Result<T> = std::result::Result<T, DbError>;

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(e) => write!(f, "database error: {e}"),
            Self::CorruptPatch { id, len } => {
                write!(f, "patch {id} is corrupt: stored data is {len} bytes, expected 256")
            }
            Self::CorruptMulti { id, len } => {
                write!(f, "multi {id} is corrupt: stored data is {len} bytes, expected 256")
            }
            Self::BankNotFound(id) => write!(f, "bank {id} not found"),
            Self::InvalidProgram(p) => write!(f, "invalid program number {p} (must be 0..=127)"),
            Self::NoBankLetter(id) => write!(
                f,
                "bank {id} is not one of the synth banks A..D; choose a target bank for export"
            ),
            Self::InvalidTargetBank(b) => {
                write!(f, "invalid target bank byte {b:#04x} (must be 1..=4 for banks A..D)")
            }
            Self::MixedSourceBanks(banks) => write!(
                f,
                "the dumps come from several source banks {banks:?}; import them into their original banks instead"
            ),
            Self::UnsupportedSchemaVersion(v) => {
                write!(f, "database schema version {v} is newer than this application supports")
            }
        }
    }
}

impl std::error::Error for DbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(e) => Some(e),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for DbError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

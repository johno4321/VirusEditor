//! SQLite patch librarian and bank manager for the Access Virus Rack.
//!
//! Patches are stored as byte-exact 256-byte single dumps; name and categories are
//! derived from the bytes and cached only as a search index. Parsing SysEx files is the
//! job of `virus-syx`; this crate takes already-parsed [`IncomingDump`]s.

pub mod db;
pub mod error;
pub mod models;
pub mod multi_data;
pub mod patch_data;
pub mod schema;

pub use db::{PatchDatabase, BANK_SLOTS};
pub use error::{DbError, Result};
pub use models::*;
pub use multi_data::{multi_name, MULTI_LEN};
pub use patch_data::{patch_categories, patch_name, PATCH_LEN};
pub use schema::SCHEMA_VERSION;

//! SQLite Patch Librarian and Bank Manager for Access Virus synthesizers.
//!
//! Provides persistence, full-text and category indexing, and SysEx bank (.syx)
//! import/export functions using an embedded SQLite database engine.

pub mod db;
pub mod models;
pub mod schema;

pub use db::PatchDatabase;
pub use models::*;
pub use schema::CREATE_TABLES_SQL;

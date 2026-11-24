//! Librarian glue between files, the synth and the database: SysEx frames → `IncomingDump`s,
//! import/export with readable reports, storing the edited patch, and a query cache that only
//! refreshes when the database changes (U1).

use std::path::Path;

use virus_db::{
    BankRecord, DbError, ImportReport, ImportTarget, IncomingDump, PatchDatabase, PatchFilter,
    PatchMeta, PatchSummary, SoundCategory,
};
use virus_syx::{extract_sysex_frames, parse_sysex, Patch, SysExMessage};

/// What a set of SysEx frames contained.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedFrames {
    /// Every single dump (`10`), in file order, whatever its bank byte.
    pub dumps: Vec<IncomingDump>,
    /// Multi dumps (`11`): not imported yet (F5).
    pub multi_dumps: usize,
    /// Other Virus messages (requests, parameter changes, the undocumented `12`, …).
    pub other_messages: usize,
    /// Frames that aren't valid Virus SysEx (other manufacturers, damaged lengths).
    pub invalid_frames: usize,
}

pub fn parse_frames(frames: &[Vec<u8>]) -> ParsedFrames {
    let mut parsed = ParsedFrames::default();
    for frame in frames {
        match parse_sysex(frame) {
            Ok(SysExMessage::SingleDump { bank, program, data, checksum_ok, .. }) => {
                parsed.dumps.push(IncomingDump { bank, program, data: *data, checksum_ok });
            }
            Ok(SysExMessage::MultiDump { .. }) => parsed.multi_dumps += 1,
            Ok(_) => parsed.other_messages += 1,
            Err(_) => parsed.invalid_frames += 1,
        }
    }
    parsed
}

/// Result of importing a file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportOutcome {
    pub parsed_multi_dumps: usize,
    pub parsed_other_messages: usize,
    pub parsed_invalid_frames: usize,
    pub report: ImportReport,
}

impl ImportOutcome {
    /// One-paragraph description for the status bar and the import panel.
    pub fn summary(&self) -> String {
        let r = &self.report;
        let mut parts = vec![format!("Imported {} patch(es)", r.imported)];
        if !r.per_bank.is_empty() {
            let banks: Vec<String> = r.per_bank.iter().map(|(name, n)| format!("{name}: {n}")).collect();
            parts[0].push_str(&format!(" ({})", banks.join(", ")));
        }
        if r.bad_checksum > 0 {
            parts.push(format!("{} with a wrong checksum (imported and flagged ⚠)", r.bad_checksum));
        }
        if !r.loose.is_empty() {
            parts.push(format!("{} edit-buffer dump(s) not stored in a slot (see below)", r.loose.len()));
        }
        if r.skipped > 0 {
            parts.push(format!("{} dump(s) skipped (unsupported bank or program)", r.skipped));
        }
        if self.parsed_multi_dumps > 0 {
            parts.push(format!("{} multi dump(s) skipped", self.parsed_multi_dumps));
        }
        if self.parsed_other_messages > 0 {
            parts.push(format!("{} other Virus message(s) skipped", self.parsed_other_messages));
        }
        if self.parsed_invalid_frames > 0 {
            parts.push(format!("{} invalid SysEx frame(s) skipped", self.parsed_invalid_frames));
        }
        parts.join("; ") + "."
    }

    pub fn has_warnings(&self) -> bool {
        let r = &self.report;
        r.bad_checksum > 0 || r.skipped > 0 || self.parsed_invalid_frames > 0
    }
}

/// Imports single dumps into the library (L1–L3): `.syx`, `.bin` (raw SysEx streams) and `.mid`.
pub fn import_bytes(db: &PatchDatabase, bytes: &[u8], target: ImportTarget) -> Result<ImportOutcome, String> {
    let frames = extract_sysex_frames(bytes);
    if frames.is_empty() {
        return Err("No SysEx messages found in the file.".to_owned());
    }
    let parsed = parse_frames(&frames);
    if parsed.dumps.is_empty() {
        return Err(format!(
            "No single dumps found ({} SysEx message(s): {} multi, {} other, {} invalid).",
            frames.len(),
            parsed.multi_dumps,
            parsed.other_messages,
            parsed.invalid_frames
        ));
    }
    import_dumps(db, &parsed.dumps, target).map(|report| ImportOutcome {
        parsed_multi_dumps: parsed.multi_dumps,
        parsed_other_messages: parsed.other_messages,
        parsed_invalid_frames: parsed.invalid_frames,
        report,
    })
}

pub fn import_dumps(db: &PatchDatabase, dumps: &[IncomingDump], target: ImportTarget) -> Result<ImportReport, String> {
    db.import_single_dumps(dumps, target).map_err(|e| format!("Import failed: {e}"))
}

/// Imports dumps received from the synth into a new user bank (e.g. a backup before sending a bank).
pub fn import_as_new_bank(db: &PatchDatabase, name: &str, dumps: &[IncomingDump]) -> Result<ImportReport, String> {
    let bank_id = db.create_bank(name).map_err(|e| format!("Cannot create bank: {e}"))?;
    match import_dumps(db, dumps, ImportTarget::Bank(bank_id)) {
        Ok(report) => Ok(report),
        Err(e) => {
            // Don't leave an empty bank behind.
            let _ = db.delete_bank(bank_id);
            Err(e)
        }
    }
}

/// Writes a bank as a `.syx` file (one single dump per stored slot, device ID Omni so any
/// Virus accepts it). `target_bank` is the SysEx `bb` (1..=4); `None` = the bank's own letter.
#[allow(dead_code)] // the app writes the bytes on a worker thread (dialogs.rs); this is the synchronous form
pub fn export_bank_file(
    db: &PatchDatabase,
    bank_id: i64,
    target_bank: Option<u8>,
    path: &Path,
) -> Result<usize, String> {
    let (bytes, count) = export_bank_bytes(db, bank_id, target_bank)?;
    std::fs::write(path, bytes).map_err(|e| format!("Cannot write {}: {e}", path.display()))?;
    Ok(count)
}

/// The `.syx` content of [`export_bank_file`] and the number of patches, so the file can be
/// written off the UI thread while the database is read on it.
pub fn export_bank_bytes(db: &PatchDatabase, bank_id: i64, target_bank: Option<u8>) -> Result<(Vec<u8>, usize), String> {
    let frames = db
        .export_bank_dumps(bank_id, virus_syx::DEVICE_ID_OMNI, target_bank)
        .map_err(|e| format!("Export failed: {e}"))?;
    if frames.is_empty() {
        return Err("Export failed: the bank is empty.".to_owned());
    }
    Ok((frames.concat(), frames.len()))
}

/// Stores `patch` byte-exact in a slot. The slot's tags and favourite flag are kept when the
/// slot already holds a patch of the same name (a re-store of an edited sound), and reset
/// otherwise (a different sound replaces it).
pub fn store_patch(db: &PatchDatabase, bank_id: i64, program: u8, patch: &Patch) -> Result<i64, DbError> {
    let meta = match db.get_patch_by_slot(bank_id, program)? {
        Some(existing) if existing.name == patch.name() => {
            PatchMeta { tags: existing.tags, favorite: existing.favorite }
        }
        _ => PatchMeta::default(),
    };
    db.save_patch(bank_id, program, patch.as_bytes(), &meta)
}

pub fn first_free_slot(slots: &[Option<PatchSummary>]) -> Option<u8> {
    slots.iter().position(Option::is_none).and_then(|i| u8::try_from(i).ok())
}

/// `A005`-style slot label (or the bank name for user banks).
pub fn slot_label(bank: Option<&BankRecord>, program: u8) -> String {
    match bank {
        Some(BankRecord { bank_letter: Some(letter), .. }) => format!("{letter}{program:03}"),
        Some(bank) => format!("{} {program:03}", bank.name),
        None => format!("{program:03}"),
    }
}

/// The librarian's current view of the database.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LibraryQuery {
    pub bank_id: Option<i64>,
    pub search: String,
    pub category: Option<SoundCategory>,
    pub favorites_only: bool,
    /// Restrict a filtered search to the selected bank.
    pub this_bank_only: bool,
}

impl LibraryQuery {
    /// With no filter the librarian shows the bank's 128 slots; otherwise a result list.
    pub fn is_filtered(&self) -> bool {
        !self.search.trim().is_empty() || self.category.is_some() || self.favorites_only
    }

    fn filter(&self) -> PatchFilter {
        PatchFilter {
            search_text: Some(self.search.trim().to_owned()).filter(|s| !s.is_empty()),
            bank_id: self.bank_id.filter(|_| self.this_bank_only),
            category: self.category,
            favorite_only: self.favorites_only,
        }
    }
}

/// Query results cached until the database's `data_version()` or the query changes.
#[derive(Debug, Default)]
pub struct LibraryCache {
    key: Option<(u64, LibraryQuery)>,
    pub banks: Vec<BankRecord>,
    /// 128 entries for the selected bank (empty if none is selected).
    pub slots: Vec<Option<PatchSummary>>,
    /// Filtered search results (empty when the query isn't filtered).
    pub results: Vec<PatchSummary>,
    pub error: Option<String>,
    /// Number of database round-trips (for tests and diagnostics).
    pub refreshes: usize,
}

impl LibraryCache {
    /// Re-queries only if the data or the query changed. Returns whether it re-queried.
    pub fn refresh(&mut self, db: &PatchDatabase, query: &LibraryQuery) -> bool {
        let version = db.data_version();
        if self.key.as_ref().is_some_and(|(v, q)| *v == version && q == query) {
            return false;
        }
        self.refreshes += 1;
        self.key = Some((version, query.clone()));
        self.error = None;
        if let Err(e) = self.load(db, query) {
            self.error = Some(e.to_string());
        }
        true
    }

    fn load(&mut self, db: &PatchDatabase, query: &LibraryQuery) -> Result<(), DbError> {
        self.banks = db.get_banks()?;
        self.slots = match query.bank_id {
            Some(id) if self.banks.iter().any(|b| b.id == id) => db.get_bank_slots(id)?,
            _ => Vec::new(),
        };
        self.results = if query.is_filtered() { db.query_patches(&query.filter())? } else { Vec::new() };
        Ok(())
    }

    pub fn bank(&self, id: i64) -> Option<&BankRecord> {
        self.banks.iter().find(|b| b.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use virus_syx::{encode_multi_dump, encode_param_change, encode_program_dump, Page, DEVICE_ID_OMNI};

    fn named(name: &str) -> Patch {
        let mut patch = Patch::init();
        patch.set_name(name);
        patch
    }

    fn bank_id(db: &PatchDatabase, letter: &str) -> i64 {
        db.get_banks().unwrap().into_iter().find(|b| b.bank_letter.as_deref() == Some(letter)).unwrap().id
    }

    /// A total-dump-like file: edit buffer, bank A and B singles, one with a bad checksum,
    /// a multi dump, an undocumented message and a foreign frame.
    fn mixed_file() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend(encode_program_dump(DEVICE_ID_OMNI, 0, 0x40, named("EditBuf").as_bytes()));
        bytes.extend(encode_program_dump(DEVICE_ID_OMNI, 1, 0, named("A zero").as_bytes()));
        let mut bad = encode_program_dump(DEVICE_ID_OMNI, 2, 64, named("B 64").as_bytes());
        bad[265] ^= 1;
        bytes.extend(bad);
        bytes.extend(encode_multi_dump(DEVICE_ID_OMNI, 1, 0, &[0; 256]));
        bytes.extend([0xF0, 0x00, 0x20, 0x33, 0x01, 0x10, 0x12, 0x01, 0xF7]);
        bytes.extend([0xF0, 0x43, 0x10, 0x00, 0xF7]);
        bytes
    }

    #[test]
    fn parse_frames_classifies_messages() {
        let parsed = parse_frames(&extract_sysex_frames(&mixed_file()));
        assert_eq!(parsed.dumps.len(), 3);
        assert_eq!((parsed.multi_dumps, parsed.other_messages, parsed.invalid_frames), (1, 1, 1));
        assert_eq!(parsed.dumps[0].bank, 0);
        assert!(!parsed.dumps[2].checksum_ok);
    }

    #[test]
    fn import_honours_bank_byte_and_reports_everything() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let outcome = import_bytes(&db, &mixed_file(), ImportTarget::OriginalBanks).unwrap();
        assert_eq!(outcome.report.imported, 2);
        assert_eq!(outcome.report.bad_checksum, 1);
        assert_eq!(outcome.report.loose.len(), 1);
        assert!(outcome.has_warnings());

        let a = db.get_bank_slots(bank_id(&db, "A")).unwrap();
        assert_eq!(a[0].as_ref().unwrap().name, "A zero");
        assert!(a[64].is_none(), "edit buffer part 0x40 must not land in slot 64");
        let b = db.get_bank_slots(bank_id(&db, "B")).unwrap();
        assert!(!b[64].as_ref().unwrap().checksum_ok);

        let summary = outcome.summary();
        for expected in ["Imported 2", "Bank A: 1", "1 with a wrong checksum", "1 edit-buffer", "1 multi", "1 other", "1 invalid"] {
            assert!(summary.contains(expected), "{summary:?} lacks {expected:?}");
        }
    }

    #[test]
    fn import_rejects_files_without_singles() {
        let db = PatchDatabase::open_in_memory().unwrap();
        assert!(import_bytes(&db, b"not sysex", ImportTarget::OriginalBanks).unwrap_err().contains("No SysEx"));
        let param = encode_param_change(0, Page::PageA, 0x40, 40, 1);
        assert!(import_bytes(&db, &param, ImportTarget::OriginalBanks).unwrap_err().contains("No single dumps"));
    }

    #[test]
    fn import_into_one_bank_reports_mixed_sources() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let err = import_bytes(&db, &mixed_file(), ImportTarget::Bank(bank_id(&db, "A"))).unwrap_err();
        assert!(err.contains("several source banks"), "{err}");
    }

    #[test]
    fn import_as_new_bank_creates_backup_bank() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let dumps = vec![IncomingDump { bank: 1, program: 3, data: *named("Backup").as_bytes(), checksum_ok: true }];
        let report = import_as_new_bank(&db, "Backup A", &dumps).unwrap();
        assert_eq!(report.imported, 1);
        let bank = db.get_banks().unwrap().into_iter().find(|b| b.name == "Backup A").unwrap();
        assert!(db.get_bank_slots(bank.id).unwrap()[3].is_some());
    }

    #[test]
    fn export_round_trips_byte_exact_and_reports_errors() {
        let db = PatchDatabase::open_in_memory().unwrap();
        import_bytes(&db, &mixed_file(), ImportTarget::OriginalBanks).unwrap();
        let dir = std::env::temp_dir().join(format!("virus-app-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bank_a.syx");

        assert_eq!(export_bank_file(&db, bank_id(&db, "A"), None, &path), Ok(1));
        let written = std::fs::read(&path).unwrap();
        assert_eq!(written, encode_program_dump(DEVICE_ID_OMNI, 1, 0, named("A zero").as_bytes()));

        assert!(export_bank_file(&db, bank_id(&db, "D"), None, &path).unwrap_err().contains("empty"));
        let bad_path = dir.join("missing-dir").join("x.syx");
        assert!(export_bank_file(&db, bank_id(&db, "A"), None, &bad_path).unwrap_err().contains("Cannot write"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn store_patch_keeps_meta_only_for_same_name() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let a = bank_id(&db, "A");
        let id = db.save_patch(a, 7, named("Lead").as_bytes(), &PatchMeta::default()).unwrap();
        db.set_tags(id, "bright").unwrap();
        db.set_favorite(id, true).unwrap();

        let mut edited = named("Lead");
        edited.set(Page::PageA, 40, 3);
        store_patch(&db, a, 7, &edited).unwrap();
        let record = db.get_patch_by_slot(a, 7).unwrap().unwrap();
        assert_eq!((record.tags.as_str(), record.favorite), ("bright", true));
        assert_eq!(&record.data, edited.as_bytes());

        store_patch(&db, a, 7, &named("Other")).unwrap();
        let record = db.get_patch_by_slot(a, 7).unwrap().unwrap();
        assert_eq!((record.tags.as_str(), record.favorite), ("", false));
    }

    #[test]
    fn store_patch_reports_errors() {
        let db = PatchDatabase::open_in_memory().unwrap();
        assert!(matches!(store_patch(&db, 9999, 0, &Patch::init()), Err(DbError::BankNotFound(9999))));
        assert!(matches!(store_patch(&db, bank_id(&db, "A"), 128, &Patch::init()), Err(DbError::InvalidProgram(128))));
    }

    #[test]
    fn first_free_slot_finds_gaps() {
        let full = PatchSummary {
            id: 1,
            bank_id: 1,
            program_number: 0,
            name: String::new(),
            category1: SoundCategory::Off,
            category2: SoundCategory::Off,
            tags: String::new(),
            favorite: false,
            checksum_ok: true,
        };
        assert_eq!(first_free_slot(&[Some(full.clone()), None]), Some(1));
        assert_eq!(first_free_slot(&[Some(full)]), None);
    }

    #[test]
    fn cache_refreshes_only_when_data_or_query_changes() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let a = bank_id(&db, "A");
        let mut cache = LibraryCache::default();
        let mut query = LibraryQuery { bank_id: Some(a), ..Default::default() };

        assert!(cache.refresh(&db, &query));
        assert_eq!(cache.slots.len(), 128);
        assert!(cache.slots.iter().all(Option::is_none));
        assert!(!cache.refresh(&db, &query));
        assert!(!cache.refresh(&db, &query));

        db.save_patch(a, 2, named("New").as_bytes(), &PatchMeta::default()).unwrap();
        assert!(cache.refresh(&db, &query));
        assert_eq!(cache.slots[2].as_ref().unwrap().name, "New");

        query.search = "ne".into();
        assert!(cache.refresh(&db, &query));
        assert_eq!(cache.results.len(), 1);
        assert_eq!(cache.refreshes, 3);
    }

    #[test]
    fn slot_labels() {
        let bank = BankRecord { id: 1, name: "Bank A".into(), bank_letter: Some("A".into()), is_rom: false };
        assert_eq!(slot_label(Some(&bank), 5), "A005");
        let user = BankRecord { id: 2, name: "Backup".into(), bank_letter: None, is_rom: false };
        assert_eq!(slot_label(Some(&user), 127), "Backup 127");
    }
}

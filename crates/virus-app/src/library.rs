//! Librarian glue between files, the synth and the database: SysEx frames → `IncomingDump`s,
//! import/export with readable reports, storing the edited patch, and a query cache that only
//! refreshes when the database changes (U1).

use std::path::Path;
use std::time::{Duration, Instant};

use virus_db::{
    BadChecksumPolicy, BankRecord, DbError, ImportPlan, ImportReport, ImportTarget, IncomingDump, IncomingMultiDump,
    MultiImportReport, PatchDatabase, PatchFilter, PatchMeta, PatchSummary, SoundCategory,
};
use virus_syx::{extract_sysex_frames, parse_sysex, Patch, SingleBank, SysExMessage};

/// What a set of SysEx frames contained.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedFrames {
    /// Every single dump (`10`), in file order, whatever its bank byte.
    pub dumps: Vec<IncomingDump>,
    /// Every multi dump (`11`), in file order; imported into the Multi bank (F9).
    pub multis: Vec<IncomingMultiDump>,
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
            Ok(SysExMessage::MultiDump { bank, program, data, checksum_ok, .. }) => {
                parsed.multis.push(IncomingMultiDump { bank, program, data: *data, checksum_ok });
            }
            Ok(_) => parsed.other_messages += 1,
            Err(_) => parsed.invalid_frames += 1,
        }
    }
    parsed
}

/// Result of importing a file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportOutcome {
    pub parsed_other_messages: usize,
    pub parsed_invalid_frames: usize,
    pub report: ImportReport,
    /// The file's multi dumps, if it had any (F9).
    pub multis: Option<MultiImportReport>,
    /// Multi bank slots left alone because "import as new bank(s)" never overwrites.
    pub multis_left_out: Vec<u8>,
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
        if !r.duplicates.is_empty() {
            let slots: Vec<String> =
                r.duplicates.iter().take(6).map(|s| format!("{} {:03}", s.bank_name, s.program)).collect();
            let more = if r.duplicates.len() > 6 { ", …" } else { "" };
            parts.push(format!(
                "⚠ {} slot(s) were addressed by more than one dump, the later one was kept ({}{more})",
                r.duplicates.len(),
                slots.join(", ")
            ));
        }
        if let Some(m) = &self.multis {
            parts.push(format!("{} multi(s) into the Multi bank", m.imported));
            if m.bad_checksum > 0 {
                parts.push(format!("{} multi(s) with a wrong checksum (flagged ⚠)", m.bad_checksum));
            }
            if !m.loose.is_empty() {
                parts.push(format!("{} Multi edit-buffer dump(s) listed on the Multi tab", m.loose.len()));
            }
            if m.skipped > 0 {
                parts.push(format!("{} multi dump(s) skipped (unsupported bank or program)", m.skipped));
            }
        }
        if !self.multis_left_out.is_empty() {
            parts.push(format!(
                "⚠ {} multi(s) not imported because their Multi bank slots are occupied (import again and choose Overwrite)",
                self.multis_left_out.len()
            ));
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
        let multi_warnings = self.multis.as_ref().is_some_and(|m| m.bad_checksum > 0 || m.skipped > 0);
        r.bad_checksum > 0
            || r.skipped > 0
            || !r.duplicates.is_empty()
            || self.parsed_invalid_frames > 0
            || multi_warnings
            || !self.multis_left_out.is_empty()
    }
}

/// A Multi bank slot an import would overwrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiOverwrite {
    pub slot: u8,
    pub old_name: String,
    pub new_name: String,
}

/// A parsed import waiting for the user's decision (H1): what it would overwrite is known
/// before anything is written.
#[derive(Debug, Clone)]
pub struct PendingImport {
    /// File name or "received from synth", for messages.
    pub label: String,
    pub dumps: Vec<IncomingDump>,
    pub target: ImportTarget,
    pub plan: ImportPlan,
    /// Multi dumps from the same file (F9); they always go to the Multi bank.
    pub multis: Vec<IncomingMultiDump>,
    /// Occupied Multi bank slots the multis would replace, in slot order.
    pub multi_overwrites: Vec<MultiOverwrite>,
    pub parsed_other_messages: usize,
    pub parsed_invalid_frames: usize,
}

impl PendingImport {
    /// Dumps received from the synth (no file-level counters).
    pub fn from_dumps(db: &PatchDatabase, label: &str, dumps: &[IncomingDump], target: ImportTarget) -> Result<Self, String> {
        let plan = db.plan_import(dumps, target).map_err(|e| format!("Import failed: {e}"))?;
        Ok(Self {
            label: label.to_owned(),
            dumps: dumps.to_vec(),
            target,
            plan,
            multis: Vec::new(),
            multi_overwrites: Vec::new(),
            parsed_other_messages: 0,
            parsed_invalid_frames: 0,
        })
    }

    /// Adds multi dumps and plans which occupied Multi bank slots they would replace.
    pub fn with_multis(mut self, db: &PatchDatabase, multis: Vec<IncomingMultiDump>) -> Result<Self, String> {
        let slots = db.get_multi_slots().map_err(|e| format!("Import failed: {e}"))?;
        let mut overwrites: Vec<MultiOverwrite> = Vec::new();
        for dump in multis.iter().filter(|d| d.bank == virus_syx::MULTI_BANK) {
            let Some(Some(existing)) = slots.get(dump.program as usize) else { continue };
            let new_name = virus_db::multi_name(&dump.data);
            match overwrites.iter_mut().find(|o| o.slot == dump.program) {
                Some(o) => o.new_name = new_name,
                None => overwrites.push(MultiOverwrite { slot: dump.program, old_name: existing.name.clone(), new_name }),
            }
        }
        overwrites.sort_by_key(|o| o.slot);
        self.multis = multis;
        self.multi_overwrites = overwrites;
        Ok(self)
    }

    /// Whether importing as planned would replace stored patches or multis.
    pub fn needs_confirmation(&self) -> bool {
        !self.plan.overwrites.is_empty() || !self.multi_overwrites.is_empty()
    }

    /// Writes the multis (all of them, or only those into empty slots) after the singles.
    fn finish(&self, db: &PatchDatabase, report: ImportReport, overwrite_multis: bool) -> Result<ImportOutcome, String> {
        let mut outcome = ImportOutcome {
            parsed_other_messages: self.parsed_other_messages,
            parsed_invalid_frames: self.parsed_invalid_frames,
            report,
            multis: None,
            multis_left_out: Vec::new(),
        };
        if self.multis.is_empty() {
            return Ok(outcome);
        }
        let occupied = |d: &IncomingMultiDump| {
            d.bank == virus_syx::MULTI_BANK && self.multi_overwrites.iter().any(|o| o.slot == d.program)
        };
        let multis: Vec<IncomingMultiDump> =
            self.multis.iter().filter(|d| overwrite_multis || !occupied(d)).cloned().collect();
        if !overwrite_multis {
            outcome.multis_left_out = self.multi_overwrites.iter().map(|o| o.slot).collect();
        }
        let report = db.import_multi_dumps(&multis).map_err(|e| {
            format!("{} patch(es) were imported, but importing the multis failed: {e}", outcome.report.imported)
        })?;
        outcome.multis = Some(report);
        Ok(outcome)
    }

    /// Writes into the planned target, replacing occupied slots (and Multi bank slots).
    pub fn commit_overwrite(&self, db: &PatchDatabase) -> Result<ImportOutcome, String> {
        let report = import_dumps(db, &self.dumps, self.target)?;
        self.finish(db, report, true)
    }

    /// Writes into fresh library banks named after `name`, one per source bank. Nothing is
    /// overwritten: multis whose Multi bank slot is occupied are left out and reported.
    pub fn commit_as_new_banks(&self, db: &PatchDatabase, name: &str) -> Result<ImportOutcome, String> {
        let report = import_as_new_banks(db, name, &self.dumps)?;
        self.finish(db, report, false)
    }
}

/// Parses `bytes` and plans the import without writing (H1).
pub fn prepare_import_bytes(
    db: &PatchDatabase,
    label: &str,
    bytes: &[u8],
    target: ImportTarget,
) -> Result<PendingImport, String> {
    let frames = extract_sysex_frames(bytes);
    if frames.is_empty() {
        return Err("No SysEx messages found in the file.".to_owned());
    }
    let parsed = parse_frames(&frames);
    if parsed.dumps.is_empty() && parsed.multis.is_empty() {
        return Err(format!(
            "No single or multi dumps found ({} SysEx message(s): {} other, {} invalid).",
            frames.len(),
            parsed.other_messages,
            parsed.invalid_frames
        ));
    }
    let mut pending = PendingImport::from_dumps(db, label, &parsed.dumps, target)?.with_multis(db, parsed.multis)?;
    pending.parsed_other_messages = parsed.other_messages;
    pending.parsed_invalid_frames = parsed.invalid_frames;
    Ok(pending)
}

/// Imports single dumps, overwriting occupied slots (tests; the UI goes through
/// [`prepare_import_bytes`] and asks first).
#[cfg(test)]
pub fn import_bytes(db: &PatchDatabase, bytes: &[u8], target: ImportTarget) -> Result<ImportOutcome, String> {
    prepare_import_bytes(db, "", bytes, target)?.commit_overwrite(db)
}

pub fn import_dumps(db: &PatchDatabase, dumps: &[IncomingDump], target: ImportTarget) -> Result<ImportReport, String> {
    db.import_single_dumps(dumps, target).map_err(|e| format!("Import failed: {e}"))
}

/// Imports dumps (e.g. received from the synth) into new user banks, one per source bank, so
/// nothing existing is overwritten. A single source bank gets `name`; several get
/// `name (A)`, `name (B)`, …
pub fn import_as_new_banks(db: &PatchDatabase, name: &str, dumps: &[IncomingDump]) -> Result<ImportReport, String> {
    let mut sources: Vec<u8> = dumps.iter().map(|d| d.bank).filter(|&b| b != 0).collect();
    sources.sort_unstable();
    sources.dedup();
    if sources.is_empty() {
        // Only edit-buffer dumps: nothing is written, they are returned as `loose`.
        return import_dumps(db, dumps, ImportTarget::OriginalBanks);
    }

    let mut total = ImportReport::default();
    let mut created = Vec::new();
    for (index, &bb) in sources.iter().enumerate() {
        let bank_name = match (sources.len(), SingleBank::from_raw(bb)) {
            (1, _) => name.to_owned(),
            (_, Some(bank)) => format!("{name} ({})", bank.letter()),
            (_, None) => format!("{name} (bb {bb})"),
        };
        let bank_id = match db.create_bank(&bank_name) {
            Ok(id) => id,
            Err(e) => {
                delete_banks(db, &created);
                return Err(format!("Cannot create bank: {e}"));
            }
        };
        created.push(bank_id);
        // Edit-buffer dumps ride along with the first group so they end up in `loose` once.
        let group: Vec<IncomingDump> =
            dumps.iter().filter(|d| d.bank == bb || (index == 0 && d.bank == 0)).cloned().collect();
        match import_dumps(db, &group, ImportTarget::Bank(bank_id)) {
            Ok(report) => {
                if report.imported == 0 {
                    delete_banks(db, &[bank_id]);
                }
                total.imported += report.imported;
                total.per_bank.extend(report.per_bank);
                total.loose.extend(report.loose);
                total.bad_checksum += report.bad_checksum;
                total.skipped += report.skipped;
                total.duplicates.extend(report.duplicates);
            }
            Err(e) => {
                delete_banks(db, &created);
                return Err(e);
            }
        }
    }
    Ok(total)
}

fn delete_banks(db: &PatchDatabase, ids: &[i64]) {
    for &id in ids {
        let _ = db.delete_bank(id);
    }
}

/// What a bank export wrote and what it left out or altered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExportResult {
    pub written: usize,
    pub skipped_flagged: usize,
    pub included_flagged: usize,
    pub skipped_invalid: usize,
}

impl ExportResult {
    /// Suffix for the status line, e.g. "; 3 flagged ⚠ patch(es) left out".
    pub fn notes(&self) -> String {
        let mut s = String::new();
        if self.skipped_flagged > 0 {
            s.push_str(&format!("; {} flagged ⚠ patch(es) left out", self.skipped_flagged));
        }
        if self.included_flagged > 0 {
            s.push_str(&format!("; {} flagged ⚠ patch(es) included", self.included_flagged));
        }
        if self.skipped_invalid > 0 {
            s.push_str(&format!("; {} patch(es) with invalid data left out", self.skipped_invalid));
        }
        s
    }
}

/// Writes a bank as a `.syx` file (one single dump per stored slot, device ID Omni so any
/// Virus accepts it). `target_bank` is the SysEx `bb` (1..=4); `None` = the bank's own letter.
/// Flagged (⚠) patches are handled according to `policy` (H2).
#[allow(dead_code)] // the app writes the bytes on a worker thread (dialogs.rs); this is the synchronous form
pub fn export_bank_file(
    db: &PatchDatabase,
    bank_id: i64,
    target_bank: Option<u8>,
    policy: BadChecksumPolicy,
    path: &Path,
) -> Result<ExportResult, String> {
    let (bytes, result) = export_bank_bytes(db, bank_id, target_bank, policy)?;
    std::fs::write(path, bytes).map_err(|e| format!("Cannot write {}: {e}", path.display()))?;
    Ok(result)
}

/// The `.syx` content of [`export_bank_file`] and what was written or left out, so the file can
/// be written off the UI thread while the database is read on it.
pub fn export_bank_bytes(
    db: &PatchDatabase,
    bank_id: i64,
    target_bank: Option<u8>,
    policy: BadChecksumPolicy,
) -> Result<(Vec<u8>, ExportResult), String> {
    let export = db
        .export_bank(bank_id, virus_syx::DEVICE_ID_OMNI, target_bank, policy)
        .map_err(|e| format!("Export failed: {e}"))?;
    let result = ExportResult {
        written: export.frames.len(),
        skipped_flagged: export.skipped_flagged,
        included_flagged: export.included_flagged,
        skipped_invalid: export.skipped_invalid,
    };
    if export.frames.is_empty() {
        let why = if result.skipped_flagged > 0 { " (all patches are flagged ⚠)" } else { "" };
        return Err(format!("Export failed: the bank is empty{why}."));
    }
    Ok((export.frames.concat(), result))
}

/// Number of stored patches flagged ⚠ (wrong checksum, or migrated from v1).
pub fn flagged_count(slots: &[Option<PatchSummary>]) -> usize {
    slots.iter().flatten().filter(|s| !s.checksum_ok).count()
}

/// How many of `count` stored patches a bank send would transmit.
pub fn sendable_count(count: usize, flagged: usize, include_flagged: bool) -> usize {
    if include_flagged {
        count
    } else {
        count.saturating_sub(flagged)
    }
}

/// What "mark as checked" means, shown before the user confirms it (R9).
pub const MARK_CHECKED_EXPLANATION: &str = "A ⚠ flag means the patch came from a dump with a wrong checksum, \
or was migrated from an old library whose bytes may have been altered. Marking it as checked only clears the flag: \
the stored bytes stay as they are. Bank send and export then include it with a correct checksum, so neither you \
nor the synth will be warned about it again. Only do this after you have checked the sound, ideally by loading \
it, sending it to the synth and listening to it.";

/// A "mark as checked" waiting for the user's confirmation (R9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkChecked {
    Patch { id: i64, label: String },
    Bank { id: i64, name: String, flagged: usize },
}

impl MarkChecked {
    pub fn question(&self) -> String {
        match self {
            Self::Patch { label, .. } => format!("Mark {label} as checked?"),
            Self::Bank { name, flagged, .. } => format!("Mark all {flagged} flagged patch(es) in {name} as checked?"),
        }
    }

    /// Clears the flag(s); returns the status-bar text.
    pub fn apply(&self, db: &PatchDatabase) -> Result<String, String> {
        match self {
            Self::Patch { id, label } => match db.mark_checked(*id) {
                Ok(true) => Ok(format!("{label} marked as checked.")),
                Ok(false) => Err("That patch no longer exists.".to_owned()),
                Err(e) => Err(format!("Cannot mark {label} as checked: {e}")),
            },
            Self::Bank { id, name, .. } => db
                .mark_bank_checked(*id)
                .map(|n| format!("{n} patch(es) in {name} marked as checked."))
                .map_err(|e| format!("Cannot mark {name} as checked: {e}")),
        }
    }
}

/// Bank bytes for which `dumps` hold all 128 programs, i.e. a full backup of that bank.
pub fn complete_banks(dumps: &[IncomingDump]) -> Vec<u8> {
    let mut banks: Vec<u8> = dumps.iter().map(|d| d.bank).filter(|&b| b != 0).collect();
    banks.sort_unstable();
    banks.dedup();
    banks.retain(|&bb| {
        let mut programs: Vec<u8> = dumps.iter().filter(|d| d.bank == bb).map(|d| d.program).collect();
        programs.sort_unstable();
        programs.dedup();
        programs.len() >= 128
    });
    banks
}

/// Most dumps kept in the "received from synth" list.
pub const MAX_RECEIVED: usize = 1024;

/// Queues a received dump: a newer dump for the same (bank, program) replaces the older one
/// (edit-buffer dumps are keyed by their part too), and the list is capped (L3).
pub fn queue_received(list: &mut Vec<IncomingDump>, dump: IncomingDump) {
    if let Some(existing) = list.iter_mut().find(|d| d.bank == dump.bank && d.program == dump.program) {
        *existing = dump;
    } else if list.len() < MAX_RECEIVED {
        list.push(dump);
    }
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

/// What the Store button does with the patch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorePlan {
    /// Store the editor's copy as it is.
    StoreNow,
    /// Request the synth's edit buffer, adopt it, then store it.
    FetchThenStore,
}

/// "Fetch from synth first" applies only when the choice is on, an output is connected and the
/// editor's own patch is being stored (a received dump is already the synth's).
pub fn plan_store(fetch_first: bool, editor_patch: bool, output_connected: bool) -> StorePlan {
    if fetch_first && editor_patch && output_connected {
        StorePlan::FetchThenStore
    } else {
        StorePlan::StoreNow
    }
}

/// A store waiting for the synth's edit-buffer dump.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingStore {
    pub bank_id: i64,
    pub program: u8,
    deadline: Instant,
}

impl PendingStore {
    /// Quiet-line wait (up to 0.5 s) plus the synth's answer.
    pub const TIMEOUT: Duration = Duration::from_secs(5);

    pub fn new(bank_id: i64, program: u8, now: Instant) -> Self {
        Self { bank_id, program, deadline: now + Self::TIMEOUT }
    }

    pub fn expired(&self, now: Instant) -> bool {
        now >= self.deadline
    }

    pub fn deadline(&self) -> Instant {
        self.deadline
    }
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
        assert_eq!((parsed.multis.len(), parsed.other_messages, parsed.invalid_frames), (1, 1, 1));
        assert_eq!(parsed.dumps[0].bank, 0);
        assert!(!parsed.dumps[2].checksum_ok);
    }

    #[test]
    fn fetch_first_needs_the_choice_an_output_and_the_editor_patch() {
        assert_eq!(plan_store(true, true, true), StorePlan::FetchThenStore);
        assert_eq!(plan_store(false, true, true), StorePlan::StoreNow, "choice is off");
        assert_eq!(plan_store(true, true, false), StorePlan::StoreNow, "no output connected");
        assert_eq!(plan_store(true, false, true), StorePlan::StoreNow, "a received dump needs no fetch");
    }

    #[test]
    fn pending_store_expires_after_the_timeout() {
        let t0 = Instant::now();
        let pending = PendingStore::new(3, 17, t0);
        assert!(!pending.expired(t0 + PendingStore::TIMEOUT - Duration::from_millis(1)));
        assert!(pending.expired(t0 + PendingStore::TIMEOUT));
        assert_eq!((pending.bank_id, pending.program), (3, 17));
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
        for expected in ["Imported 2", "Bank A: 1", "1 with a wrong checksum", "1 edit-buffer", "1 multi(s) into the Multi bank", "1 other", "1 invalid"] {
            assert!(summary.contains(expected), "{summary:?} lacks {expected:?}");
        }
    }

    fn multi(name: &str) -> virus_syx::Multi {
        let mut multi = virus_syx::Multi::from_bytes(&[0; 256]);
        multi.set_name(name);
        multi
    }

    fn multi_dump(bank: u8, program: u8, name: &str) -> Vec<u8> {
        encode_multi_dump(DEVICE_ID_OMNI, bank, program, multi(name).as_bytes())
    }

    #[test]
    fn a_file_of_only_multis_goes_into_the_multi_bank() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let file = [multi_dump(1, 3, "Three"), multi_dump(0, 0, "EditBuf")].concat();
        let outcome = import_bytes(&db, &file, ImportTarget::OriginalBanks).unwrap();
        assert_eq!(db.get_multi_by_slot(3).unwrap().unwrap().name, "Three");
        let multis = outcome.multis.as_ref().unwrap();
        assert_eq!((multis.imported, multis.loose.len()), (1, 1));
        let summary = outcome.summary();
        assert!(summary.contains("1 multi(s) into the Multi bank"), "{summary}");
        assert!(summary.contains("1 Multi edit-buffer dump(s) listed on the Multi tab"), "{summary}");
    }

    #[test]
    fn occupied_multi_slots_need_confirmation_and_new_banks_leave_them_alone() {
        let db = PatchDatabase::open_in_memory().unwrap();
        crate::multi_session::store_multi(&db, Some(3), &multi("Mine")).unwrap();
        let file = [
            encode_program_dump(DEVICE_ID_OMNI, 1, 0, named("A zero").as_bytes()),
            multi_dump(1, 3, "Theirs"),
            multi_dump(1, 4, "Free"),
        ]
        .concat();

        let pending = prepare_import_bytes(&db, "f", &file, ImportTarget::OriginalBanks).unwrap();
        assert!(pending.plan.overwrites.is_empty());
        assert!(pending.needs_confirmation(), "the multi in slot 3 would be replaced");
        assert_eq!(
            pending.multi_overwrites,
            [MultiOverwrite { slot: 3, old_name: "Mine".into(), new_name: "Theirs".into() }]
        );
        assert_eq!(db.get_multi_by_slot(4).unwrap(), None, "planning must not write");

        let outcome = pending.commit_as_new_banks(&db, "Imported").unwrap();
        assert_eq!(db.get_multi_by_slot(3).unwrap().unwrap().name, "Mine");
        assert_eq!(db.get_multi_by_slot(4).unwrap().unwrap().name, "Free");
        assert_eq!(outcome.multis_left_out, [3]);
        assert!(outcome.has_warnings());
        assert!(outcome.summary().contains("1 multi(s) not imported because their Multi bank slots are occupied"));

        pending.commit_overwrite(&db).unwrap();
        assert_eq!(db.get_multi_by_slot(3).unwrap().unwrap().name, "Theirs");
    }

    #[test]
    fn mark_checked_for_a_patch_and_a_bank() {
        let db = PatchDatabase::open_in_memory().unwrap();
        import_bytes(&db, &mixed_file(), ImportTarget::OriginalBanks).unwrap();
        let b = bank_id(&db, "B");
        let id = db.get_patch_by_slot(b, 64).unwrap().unwrap().id;

        let one = MarkChecked::Patch { id, label: "B064".into() };
        assert_eq!(one.question(), "Mark B064 as checked?");
        assert_eq!(one.apply(&db).unwrap(), "B064 marked as checked.");
        assert_eq!(flagged_count(&db.get_bank_slots(b).unwrap()), 0);
        let gone = MarkChecked::Patch { id: 9999, label: "X".into() };
        assert!(gone.apply(&db).unwrap_err().contains("no longer exists"));

        db.import_single_dumps(
            &[IncomingDump { checksum_ok: false, ..dump(2, 1, "Again") }],
            ImportTarget::OriginalBanks,
        )
        .unwrap();
        let bank = MarkChecked::Bank { id: b, name: "Bank B".into(), flagged: 1 };
        assert_eq!(bank.apply(&db).unwrap(), "1 patch(es) in Bank B marked as checked.");
        assert_eq!(flagged_count(&db.get_bank_slots(b).unwrap()), 0);
    }

    #[test]
    fn import_rejects_files_without_singles() {
        let db = PatchDatabase::open_in_memory().unwrap();
        assert!(import_bytes(&db, b"not sysex", ImportTarget::OriginalBanks).unwrap_err().contains("No SysEx"));
        let param = encode_param_change(0, Page::PageA, 0x40, 40, 1);
        assert!(import_bytes(&db, &param, ImportTarget::OriginalBanks).unwrap_err().contains("No single or multi dumps"));
    }

    #[test]
    fn import_into_one_bank_reports_mixed_sources() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let err = import_bytes(&db, &mixed_file(), ImportTarget::Bank(bank_id(&db, "A"))).unwrap_err();
        assert!(err.contains("several source banks"), "{err}");
    }

    #[test]
    fn import_as_new_banks_creates_backup_bank() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let dumps = vec![IncomingDump { bank: 1, program: 3, data: *named("Backup").as_bytes(), checksum_ok: true }];
        let report = import_as_new_banks(&db, "Backup A", &dumps).unwrap();
        assert_eq!(report.imported, 1);
        let bank = db.get_banks().unwrap().into_iter().find(|b| b.name == "Backup A").unwrap();
        assert!(db.get_bank_slots(bank.id).unwrap()[3].is_some());
    }

    fn dump(bank: u8, program: u8, name: &str) -> IncomingDump {
        IncomingDump { bank, program, data: *named(name).as_bytes(), checksum_ok: true }
    }

    #[test]
    fn mixed_banks_become_one_new_bank_each_without_touching_existing() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let a = bank_id(&db, "A");
        db.save_patch(a, 0, named("Mine").as_bytes(), &PatchMeta::default()).unwrap();
        let dumps = vec![dump(1, 0, "Rx A0"), dump(2, 5, "Rx B5"), dump(1, 1, "Rx A1")];

        let pending = PendingImport::from_dumps(&db, "received", &dumps, ImportTarget::OriginalBanks).unwrap();
        assert!(pending.needs_confirmation());
        assert_eq!(pending.plan.overwrites.len(), 1);

        let out = pending.commit_as_new_banks(&db, "Backup").unwrap();
        assert_eq!(out.report.imported, 3);
        let names: Vec<String> = db.get_banks().unwrap().into_iter().map(|b| b.name).collect();
        assert!(names.contains(&"Backup (A)".to_owned()) && names.contains(&"Backup (B)".to_owned()), "{names:?}");
        assert_eq!(db.get_patch_by_slot(a, 0).unwrap().unwrap().name, "Mine");
    }

    #[test]
    fn overwrite_is_planned_before_it_is_written() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let a = bank_id(&db, "A");
        db.save_patch(a, 0, named("Mine").as_bytes(), &PatchMeta::default()).unwrap();
        let pending = prepare_import_bytes(&db, "f", &mixed_file(), ImportTarget::OriginalBanks).unwrap();
        assert_eq!(pending.plan.overwrites[0].old_name, "Mine");
        assert_eq!(db.get_patch_by_slot(a, 0).unwrap().unwrap().name, "Mine", "planning must not write");
        pending.commit_overwrite(&db).unwrap();
        assert_eq!(db.get_patch_by_slot(a, 0).unwrap().unwrap().name, "A zero");
    }

    /// The damaged `multis.mid` total dump end to end (REVIEW T4, F9): singles go to A and B,
    /// the 33 edit-buffer dumps stay loose, the multis go to the Multi bank (the edit-buffer
    /// multi stays loose) and the undocumented message is only counted.
    #[test]
    fn multis_mid_total_dump_imports_end_to_end() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../VirusOSR200_INT/OSR200_INT/Goodies/Factory soundset/multis.mid"
        );
        let bytes = std::fs::read(path).unwrap();
        let db = PatchDatabase::open_in_memory().unwrap();
        let pending = prepare_import_bytes(&db, "multis.mid", &bytes, ImportTarget::OriginalBanks).unwrap();
        assert!(!pending.needs_confirmation(), "factory banks are empty on a fresh library");
        assert_eq!(pending.multis.len(), 129);
        let outcome = pending.commit_overwrite(&db).unwrap();
        let r = &outcome.report;
        assert_eq!(r.loose.len(), 33);
        let multis = outcome.multis.as_ref().unwrap();
        // Multi 10 reads as mm = 13 in the damaged file, so slot 13 is written twice.
        assert_eq!((multis.imported, multis.loose.len()), (128, 1));
        assert_eq!(db.get_multi_slots().unwrap().iter().flatten().count(), 127);
        assert_eq!(outcome.parsed_other_messages, 1);
        // The damaged file has one slot addressed twice per bank.
        assert_eq!((r.imported, r.duplicates.len()), (254, 2));
        assert!(outcome.has_warnings());
    }

    #[test]
    fn received_list_dedups_and_caps() {
        let mut list = Vec::new();
        queue_received(&mut list, dump(1, 0, "first"));
        queue_received(&mut list, dump(1, 0, "second"));
        queue_received(&mut list, dump(2, 0, "other bank"));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].data, *named("second").as_bytes());
        for i in 0..2000u32 {
            queue_received(&mut list, dump(1 + (i / 128) as u8, (i % 128) as u8, "x"));
        }
        assert_eq!(list.len(), MAX_RECEIVED);
    }

    #[test]
    fn complete_banks_needs_all_128_and_send_count_follows_policy() {
        let mut dumps: Vec<IncomingDump> = (0..128).map(|p| dump(1, p, "x")).collect();
        dumps.extend((0..127).map(|p| dump(2, p, "y")));
        assert_eq!(complete_banks(&dumps), vec![1]);
        assert_eq!(sendable_count(10, 3, false), 7);
        assert_eq!(sendable_count(10, 3, true), 10);
    }

    #[test]
    fn export_policy_skips_flagged_by_default() {
        let db = PatchDatabase::open_in_memory().unwrap();
        import_bytes(&db, &mixed_file(), ImportTarget::OriginalBanks).unwrap();
        let dir = std::env::temp_dir().join(format!("virus-app-flag-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("b.syx");
        let b = bank_id(&db, "B");
        let err = export_bank_file(&db, b, None, BadChecksumPolicy::Skip, &path).unwrap_err();
        assert!(err.contains("flagged"), "{err}");
        let res = export_bank_file(&db, b, None, BadChecksumPolicy::Rechecksum, &path).unwrap();
        assert_eq!((res.written, res.included_flagged), (1, 1));
        assert_eq!(flagged_count(&db.get_bank_slots(b).unwrap()), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn export_round_trips_byte_exact_and_reports_errors() {
        let db = PatchDatabase::open_in_memory().unwrap();
        import_bytes(&db, &mixed_file(), ImportTarget::OriginalBanks).unwrap();
        let dir = std::env::temp_dir().join(format!("virus-app-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bank_a.syx");
        let skip = BadChecksumPolicy::Skip;

        assert_eq!(export_bank_file(&db, bank_id(&db, "A"), None, skip, &path).unwrap().written, 1);
        let written = std::fs::read(&path).unwrap();
        assert_eq!(written, encode_program_dump(DEVICE_ID_OMNI, 1, 0, named("A zero").as_bytes()));

        assert!(export_bank_file(&db, bank_id(&db, "D"), None, skip, &path).unwrap_err().contains("empty"));
        let bad_path = dir.join("missing-dir").join("x.syx");
        assert!(export_bank_file(&db, bank_id(&db, "A"), None, skip, &bad_path).unwrap_err().contains("Cannot write"));
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

//! Multi mode logic (TODO F4/F5) without egui: the Multi edit buffer, incoming multi dumps and
//! parameter changes, live edits via Page C, librarian glue for multis, and the lookup of the
//! single names that the parts play.
//!
//! Nothing here touches egui or a real MIDI port, so it is unit-tested directly.

use std::path::Path;

use virus_db::{
    BadChecksumPolicy, DbError, ImportTarget, IncomingMultiDump, MultiImportReport, MultiSummary,
    PatchDatabase, PatchMeta,
};
use virus_midi::MidiEvent;
use virus_syx::{
    encode_dump_request, encode_multi_bank_request, encode_multi_dump, encode_multi_request,
    extract_sysex_frames, multi_param_for_live, parse_sysex, Multi, MultiDumpTarget, MultiParamDef,
    SysExMessage, ValueKind, DEVICE_ID_OMNI, MULTI_BANK, MULTI_BANK_SLOTS, MULTI_PARAMS,
    MULTI_PARTS, PART_BANK_NAMES,
};

use crate::library::{
    multi_duplicates_text, ExportResult, ImportOutcome, PendingImport, MAX_RECEIVED,
};
use crate::session::{EditPart, MidiSettings};
use crate::views::Notice;

/// What an incoming MIDI event meant for the Multi edit buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultiIncoming {
    /// A Multi edit-buffer dump (`11 00`) replaced the edited multi.
    Loaded {
        name: String,
        checksum_ok: bool,
    },
    /// A Multi bank dump (`11 01 mm`), queued in [`MultiSession::received`].
    Queued {
        program: u8,
        checksum_ok: bool,
    },
    /// A parameter change updated one value of the edited multi.
    ParamChanged {
        name: &'static str,
        part: u8,
        value: u8,
    },
    Ignored,
}

impl MultiIncoming {
    /// Status-bar text for events worth reporting.
    pub fn notice(&self, received: usize) -> Option<Notice> {
        match self {
            Self::Loaded {
                name,
                checksum_ok: true,
            } => Some(Notice::info(format!(
                "Received multi '{name}' from the synth."
            ))),
            Self::Loaded {
                name,
                checksum_ok: false,
            } => Some(Notice::warning(format!(
                "Received multi '{name}' from the synth with a wrong checksum."
            ))),
            Self::Queued {
                program,
                checksum_ok,
            } => {
                let flag = if *checksum_ok {
                    ""
                } else {
                    " (wrong checksum)"
                };
                Some(Notice::info(format!(
                    "Received multi dump M{program:03}{flag}; {received} waiting under Multi → Received from synth."
                )))
            }
            Self::ParamChanged { .. } | Self::Ignored => None,
        }
    }
}

/// The edited multi (a copy of the synth's Multi edit buffer) and the rules for keeping it in sync.
#[derive(Debug, Default)]
pub struct MultiSession {
    multi: Option<Multi>,
    /// Where the edited multi came from (for the header).
    pub origin: String,
    /// Some edited values have no parameter change (tempo, multi delay): send the whole multi.
    pub unsent_changes: bool,
    /// Multi bank dumps received from the synth (e.g. the answer to `33 01`), offered for import.
    /// One per slot (a newer dump replaces an older one) and capped; see [`queue_received_multi`].
    pub received: Vec<IncomingMultiDump>,
    /// The edited multi came from a flagged source (CR-H1); see [`Self::flagged`].
    flagged: bool,
}

impl MultiSession {
    pub fn multi(&self) -> Option<&Multi> {
        self.multi.as_ref()
    }

    /// `flagged`: the multi comes from a flagged library record, a flagged edit-buffer dump from
    /// a file, or a Multi edit-buffer dump that arrived with a wrong checksum (CR-H1).
    pub fn load(&mut self, multi: Multi, origin: impl Into<String>, flagged: bool) {
        self.multi = Some(multi);
        self.origin = origin.into();
        self.unsent_changes = false;
        self.flagged = flagged;
    }

    /// Whether the edited multi came from a flagged source: storing it keeps the ⚠ flag.
    /// Editing doesn't clear it; loading another multi replaces it.
    pub fn flagged(&self) -> bool {
        self.flagged
    }

    /// Interprets one event from the MIDI worker. Only SysEx is relevant: multi dumps and
    /// parameter changes that address a Multi value (SPEC §4.3). Never consumes the event; the
    /// single-sound session sees it too.
    pub fn handle_midi(&mut self, event: &MidiEvent, settings: &MidiSettings) -> MultiIncoming {
        let MidiEvent::SysEx(frame) = event else {
            return MultiIncoming::Ignored;
        };
        let Ok(message) = parse_sysex(frame) else {
            return MultiIncoming::Ignored;
        };
        let dd = message.device_id();
        if !(settings.device_id == DEVICE_ID_OMNI
            || dd == DEVICE_ID_OMNI
            || dd == settings.device_id)
        {
            return MultiIncoming::Ignored;
        }
        match message {
            SysExMessage::MultiDump {
                bank,
                program,
                data,
                checksum_ok,
                ..
            } => match MultiDumpTarget::from_raw(bank, program) {
                Some(MultiDumpTarget::EditBuffer) => {
                    self.load(Multi::from_bytes(&data), "synth", !checksum_ok);
                    MultiIncoming::Loaded {
                        name: self.multi.as_ref().map(Multi::name).unwrap_or_default(),
                        checksum_ok,
                    }
                }
                Some(MultiDumpTarget::Bank { program }) => {
                    queue_received_multi(
                        &mut self.received,
                        IncomingMultiDump {
                            bank,
                            program,
                            data: *data,
                            checksum_ok,
                        },
                    );
                    MultiIncoming::Queued {
                        program,
                        checksum_ok,
                    }
                }
                None => MultiIncoming::Ignored,
            },
            SysExMessage::ParamChange {
                page,
                part,
                param,
                value,
                ..
            } => {
                let (Some(multi), Some(def)) =
                    (self.multi.as_mut(), multi_param_for_live(page, param))
                else {
                    return MultiIncoming::Ignored;
                };
                // Part parameters are addressed to parts 0..15; part 0x40 is the Single buffer.
                if def.is_part_param() && part as usize >= MULTI_PARTS {
                    return MultiIncoming::Ignored;
                }
                let Some(stored) = def.stored_from_live(value) else {
                    return MultiIncoming::Ignored;
                };
                multi.set(def, part, stored);
                MultiIncoming::ParamChanged {
                    name: def.name,
                    part,
                    value: multi.get(def, part),
                }
            }
            _ => MultiIncoming::Ignored,
        }
    }

    /// Changes one value of the edited multi and returns the parameter change to send, if the
    /// parameter has one. Values without one set [`Self::unsent_changes`].
    pub fn edit(
        &mut self,
        settings: &MidiSettings,
        def: &MultiParamDef,
        part: u8,
        value: u8,
    ) -> Option<Vec<u8>> {
        let multi = self.multi.as_mut()?;
        if !multi.set(def, part, value) {
            return None;
        }
        let message = def.param_change(settings.device_id, part, multi.get(def, part));
        if message.is_none() {
            self.unsent_changes = true;
        }
        message
    }

    /// Writes the name (ASCII 32..126, at most 10 characters) and returns one C5–C14 change per
    /// name byte that changed.
    pub fn set_name(&mut self, settings: &MidiSettings, name: &str) -> Vec<Vec<u8>> {
        let Some(multi) = self.multi.as_mut() else {
            return Vec::new();
        };
        let before = multi.clone();
        multi.set_name(&crate::session::sanitize_name(name));
        MULTI_PARAMS
            .iter()
            .filter(|def| {
                def.kind == ValueKind::NameChar && multi.get(def, 0) != before.get(def, 0)
            })
            .filter_map(|def| def.param_change(settings.device_id, 0, multi.get(def, 0)))
            .collect()
    }

    /// Multi request for the edit buffer (`31 00 00`).
    pub fn fetch_request(settings: &MidiSettings) -> Vec<u8> {
        encode_multi_request(settings.device_id, 0, 0)
    }

    /// Multi bank request (`33 01`).
    pub fn bank_request(settings: &MidiSettings) -> Vec<u8> {
        encode_multi_bank_request(settings.device_id)
    }

    /// Single request for one part's edit buffer (`30 00 pp`), F4.
    pub fn part_fetch_request(settings: &MidiSettings, part: u8) -> Vec<u8> {
        encode_dump_request(settings.device_id, 0, part & 0x0F)
    }

    /// Multi dump of the edited multi into the Multi edit buffer (`11 00 00`). Clears
    /// [`Self::unsent_changes`].
    pub fn edit_buffer_dump(&mut self, settings: &MidiSettings) -> Option<Vec<u8>> {
        let multi = self.multi.as_ref()?;
        self.unsent_changes = false;
        Some(multi.to_sysex(settings.device_id, MultiDumpTarget::EditBuffer))
    }

    /// MIDI channel (0..15) of `part` from the fetched/loaded multi (bytes 64..79, C34).
    pub fn part_channel(&self, part: u8) -> Option<u8> {
        self.multi
            .as_ref()
            .map(|m| m.part(part).midi_channel & 0x0F)
    }

    /// The edited Multi part's channel from the multi, for note audition (TODO N9); `None` in
    /// Single mode or before a multi has been fetched.
    pub fn edited_part_channel(&self, part: EditPart) -> Option<u8> {
        match part {
            EditPart::Single => None,
            EditPart::Multi(part) => self.part_channel(part),
        }
    }

    /// Channels Panic must silence: the synth channel, plus (in Multi mode, once a multi is
    /// known) the channel of every enabled part. No duplicates, synth channel first.
    pub fn panic_channels(&self, settings: &MidiSettings) -> Vec<u8> {
        let mut channels = vec![settings.channel & 0x0F];
        if let (EditPart::Multi(_), Some(multi)) = (settings.part, self.multi()) {
            for part in (0..16u8).filter(|&p| multi.part_enabled(p)) {
                let channel = multi.part(part).midi_channel & 0x0F;
                if !channels.contains(&channel) {
                    channels.push(channel);
                }
            }
        }
        channels
    }
}

/// Display text for a Multi value (`MIDI Volume Init` 0 is "Off").
pub fn display_value(def: &MultiParamDef, raw: u8) -> String {
    if def.name == "Part MIDI Volume Init" && raw == 0 {
        return "Off".to_owned();
    }
    def.format(raw)
}

// ---------------------------------------------------------------------------------------------
// Librarian glue
// ---------------------------------------------------------------------------------------------

/// Every multi dump (`11`) in a set of SysEx frames, in order, whatever its bank byte.
pub fn parse_multi_frames(frames: &[Vec<u8>]) -> Vec<IncomingMultiDump> {
    frames
        .iter()
        .filter_map(|frame| match parse_sysex(frame) {
            Ok(SysExMessage::MultiDump {
                bank,
                program,
                data,
                checksum_ok,
                ..
            }) => Some(IncomingMultiDump {
                bank,
                program,
                data: *data,
                checksum_ok,
            }),
            _ => None,
        })
        .collect()
}

/// Queues a received Multi bank dump: a newer dump for the same (bank, program) replaces the
/// older one, and the list is capped like the singles' (CR-M9).
pub fn queue_received_multi(list: &mut Vec<IncomingMultiDump>, dump: IncomingMultiDump) {
    if let Some(existing) = list
        .iter_mut()
        .find(|d| d.bank == dump.bank && d.program == dump.program)
    {
        *existing = dump;
    } else if list.len() < MAX_RECEIVED {
        list.push(dump);
    }
}

/// Plans importing multi dumps (received from the synth, or read from a file) into the Multi
/// bank without writing anything (CR-H2). Ask before committing if it
/// [`PendingImport::needs_confirmation`]; then use [`commit_multi_import`].
pub fn prepare_multi_import(
    db: &PatchDatabase,
    label: &str,
    dumps: Vec<IncomingMultiDump>,
) -> Result<PendingImport, String> {
    if dumps.is_empty() {
        return Err("No multi dumps to import.".to_owned());
    }
    PendingImport::from_dumps(db, label, &[], ImportTarget::OriginalBanks)?.with_multis(db, dumps)
}

/// [`prepare_multi_import`] for the multi dumps of a `.syx` / `.mid` file. Singles and other
/// messages are ignored here (the Librarian imports them).
pub fn prepare_multi_import_bytes(
    db: &PatchDatabase,
    label: &str,
    bytes: &[u8],
) -> Result<PendingImport, String> {
    let dumps = parse_multi_frames(&extract_sysex_frames(bytes));
    if dumps.is_empty() {
        return Err("No multi dumps found in the file.".to_owned());
    }
    prepare_multi_import(db, label, dumps)
}

/// Writes a planned multi import: `overwrite` replaces occupied Multi bank slots, otherwise
/// only empty slots are written and the occupied ones are reported as left out.
pub fn commit_multi_import(
    db: &PatchDatabase,
    pending: &PendingImport,
    overwrite: bool,
) -> Result<ImportOutcome, String> {
    debug_assert!(
        pending.dumps.is_empty(),
        "a multi import carries no singles"
    );
    if overwrite {
        pending.commit_overwrite(db)
    } else {
        // Without singles no bank is created: this only skips the occupied Multi bank slots.
        pending.commit_as_new_banks(db, &pending.label)
    }
}

/// Status text for a committed multi import.
pub fn multi_import_summary(outcome: &ImportOutcome) -> String {
    let mut text = match &outcome.multis {
        Some(report) => import_summary(report),
        None => "No multis were imported.".to_owned(),
    };
    if !outcome.multis_left_out.is_empty() {
        text.pop();
        let slots: Vec<String> = outcome
            .multis_left_out
            .iter()
            .take(6)
            .map(|s| format!("M{s:03}"))
            .collect();
        let more = if outcome.multis_left_out.len() > 6 {
            ", …"
        } else {
            ""
        };
        text.push_str(&format!(
            "; {} occupied slot(s) left alone ({}{more}).",
            outcome.multis_left_out.len(),
            slots.join(", ")
        ));
    }
    text
}

/// Whether a committed multi import deserves a warning rather than an info notice.
pub fn multi_import_has_warnings(outcome: &ImportOutcome) -> bool {
    outcome
        .multis
        .as_ref()
        .is_some_and(|m| m.bad_checksum > 0 || m.skipped > 0 || !m.duplicates.is_empty())
}

pub fn import_summary(report: &MultiImportReport) -> String {
    let mut parts = vec![format!(
        "Imported {} multi(s) into the Multi bank",
        report.imported
    )];
    if report.bad_checksum > 0 {
        parts.push(format!(
            "{} with a wrong checksum (imported and flagged ⚠)",
            report.bad_checksum
        ));
    }
    if !report.loose.is_empty() {
        parts.push(format!(
            "{} Multi edit-buffer dump(s) not stored in a slot",
            report.loose.len()
        ));
    }
    if report.skipped > 0 {
        parts.push(format!(
            "{} dump(s) skipped (unsupported bank or program, or invalid data)",
            report.skipped
        ));
    }
    if !report.duplicates.is_empty() {
        parts.push(multi_duplicates_text(&report.duplicates));
    }
    parts.join("; ") + "."
}

/// Whether `dumps` hold all 128 Multi bank programs, i.e. a full backup of the synth's bank.
pub fn is_complete_multi_bank(dumps: &[IncomingMultiDump]) -> bool {
    let mut programs: Vec<u8> = dumps
        .iter()
        .filter(|d| d.bank == MULTI_BANK)
        .map(|d| d.program)
        .collect();
    programs.sort_unstable();
    programs.dedup();
    programs.len() >= MULTI_BANK_SLOTS
}

/// `.syx` bytes of received multi dumps, for a backup file (device ID Omni). A dump that
/// arrived with a wrong checksum keeps a wrong one, so the damage stays detectable.
pub fn multi_backup_bytes(dumps: &[IncomingMultiDump]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for dump in dumps {
        let mut frame = encode_multi_dump(DEVICE_ID_OMNI, dump.bank, dump.program, &dump.data);
        if !dump.checksum_ok {
            let checksum = frame.len() - 2;
            frame[checksum] = (frame[checksum] + 1) & 0x7F;
        }
        bytes.extend(frame);
    }
    bytes
}

/// The `.syx` content of the Multi bank (device ID Omni) and what was written or left out, so
/// the file can be written off the UI thread. Flagged (⚠) multis follow `policy`; multis with
/// bytes ≥ 0x80 are always left out (CR-H3).
pub fn export_multi_bank_bytes(
    db: &PatchDatabase,
    policy: BadChecksumPolicy,
) -> Result<(Vec<u8>, ExportResult), String> {
    let export = db
        .export_multi_bank(DEVICE_ID_OMNI, policy)
        .map_err(|e| format!("Export failed: {e}"))?;
    let result = ExportResult::from(&export);
    if export.frames.is_empty() {
        let why = if result.skipped_flagged > 0 {
            " (all multis are flagged ⚠)"
        } else {
            ""
        };
        return Err(format!("Export failed: the Multi bank is empty{why}."));
    }
    Ok((export.frames.concat(), result))
}

/// Writes the Multi bank as a `.syx` file (device ID Omni); see [`export_multi_bank_bytes`].
#[allow(dead_code)] // the app writes the bytes on a worker thread (dialogs.rs); this is the synchronous form
pub fn export_multi_bank_file(
    db: &PatchDatabase,
    policy: BadChecksumPolicy,
    path: &Path,
) -> Result<ExportResult, String> {
    let (bytes, result) = export_multi_bank_bytes(db, policy)?;
    std::fs::write(path, bytes).map_err(|e| format!("Cannot write {}: {e}", path.display()))?;
    Ok(result)
}

/// Status text for a written Multi bank export.
pub fn export_summary(result: &ExportResult, path: &Path) -> String {
    format!(
        "Exported {} multi(s) to {}{}.",
        result.written,
        path.display(),
        result.notes_for("multi(s)")
    )
}

/// Stores `multi` byte-exact in a Multi bank slot (`None` = unslotted). Tags and favourite are
/// kept when the slot already holds a multi of the same name. `checksum_ok = false` stores it
/// flagged ⚠ (CR-H1).
pub fn store_multi(
    db: &PatchDatabase,
    slot: Option<u8>,
    multi: &Multi,
    checksum_ok: bool,
) -> Result<i64, DbError> {
    let meta = match slot.map(|s| db.get_multi_by_slot(s)).transpose()?.flatten() {
        Some(existing) if existing.name == multi.name() => PatchMeta {
            tags: existing.tags,
            favorite: existing.favorite,
        },
        _ => PatchMeta::default(),
    };
    db.save_multi(slot, multi.as_bytes(), &meta, checksum_ok)
}

/// Stores the edited multi in a Multi bank slot with its flag. Returns the status text.
pub fn store_edited_multi(
    db: &PatchDatabase,
    session: &MultiSession,
    slot: u8,
) -> Result<String, String> {
    let Some(multi) = session.multi() else {
        return Err("No multi is loaded.".to_owned());
    };
    match store_multi(db, Some(slot), multi, !session.flagged()) {
        Ok(_) if session.flagged() => Ok(format!(
            "Stored '{}' in M{slot:03}, flagged ⚠.",
            multi.name()
        )),
        Ok(_) => Ok(format!("Stored '{}' in M{slot:03}.", multi.name())),
        Err(e) => Err(format!("Library error: {e}")),
    }
}

/// A Multi-tab library change waiting for the user's confirmation (CR-L2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultiConfirm {
    /// Delete a stored multi (slotted or unslotted).
    Delete {
        id: i64,
        label: String,
        name: String,
    },
    /// Store the edited multi over an occupied Multi bank slot.
    Overwrite { slot: u8, old_name: String },
}

impl MultiConfirm {
    pub fn title(&self) -> &'static str {
        match self {
            Self::Delete { .. } => "Delete multi",
            Self::Overwrite { .. } => "Overwrite multi",
        }
    }

    /// What is lost, shown before the user confirms.
    pub fn question(&self, session: &MultiSession) -> String {
        match self {
            Self::Delete { label, name, .. } => format!(
                "Delete '{name}' ({label}) from the library, with its tags and favourite? This can't be undone."
            ),
            Self::Overwrite { slot, old_name } => {
                let new = session.multi().map(Multi::name).unwrap_or_default();
                format!("M{slot:03} holds '{old_name}'. Store '{new}' over it? The old multi, its tags and favourite are lost.")
            }
        }
    }

    pub fn confirm_label(&self) -> String {
        match self {
            Self::Delete { .. } => "Delete".to_owned(),
            Self::Overwrite { slot, .. } => format!("Overwrite M{slot:03}"),
        }
    }

    /// Carries out the confirmed change; returns the status-bar text.
    pub fn apply(&self, db: &PatchDatabase, session: &MultiSession) -> Result<String, String> {
        match self {
            Self::Delete { id, name, .. } => match db.delete_multi(*id) {
                Ok(true) => Ok(format!("Deleted '{name}'.")),
                Ok(false) => Err("That multi no longer exists.".to_owned()),
                Err(e) => Err(format!("Library error: {e}")),
            },
            Self::Overwrite { slot, .. } => store_edited_multi(db, session, *slot),
        }
    }
}

/// The Multi bank and unslotted multis, cached until the database changes.
#[derive(Debug, Default)]
pub struct MultiLibraryCache {
    version: Option<u64>,
    /// 128 entries, index = Multi bank program.
    pub slots: Vec<Option<MultiSummary>>,
    pub loose: Vec<MultiSummary>,
    pub error: Option<String>,
}

impl MultiLibraryCache {
    /// Re-queries only if the data changed. Returns whether it re-queried.
    pub fn refresh(&mut self, db: &PatchDatabase) -> bool {
        let version = db.data_version();
        if self.version == Some(version) {
            return false;
        }
        self.version = Some(version);
        match db
            .get_multi_slots()
            .and_then(|slots| Ok((slots, db.get_loose_multis()?)))
        {
            Ok((slots, loose)) => {
                self.slots = slots;
                self.loose = loose;
                self.error = None;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        true
    }
}

/// Names of the library's singles in banks A–D, to label the sound each part plays.
#[derive(Debug, Default)]
pub struct SingleNames {
    version: Option<u64>,
    /// Index = part bank byte (0 = A … 3 = D); 128 names each (`None` = empty slot).
    banks: [Vec<Option<String>>; 4],
}

impl SingleNames {
    pub fn refresh(&mut self, db: &PatchDatabase) -> bool {
        let version = db.data_version();
        if self.version == Some(version) {
            return false;
        }
        self.version = Some(version);
        self.banks = Default::default();
        let Ok(banks) = db.get_banks() else {
            return true;
        };
        for (index, letter) in PART_BANK_NAMES.iter().enumerate() {
            let Some(bank) = banks
                .iter()
                .find(|b| b.bank_letter.as_deref() == Some(*letter))
            else {
                continue;
            };
            if let Ok(slots) = db.get_bank_slots(bank.id) {
                self.banks[index] = slots.into_iter().map(|s| s.map(|s| s.name)).collect();
            }
        }
        true
    }

    /// The library's name for part bank byte `bank` (0 = A, 1 = B, …) and `program`, if known.
    pub fn name(&self, bank: u8, program: u8) -> Option<&str> {
        self.banks
            .get(bank as usize)?
            .get(program as usize)?
            .as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::EditPart;
    use virus_db::MULTI_LEN;
    use virus_syx::domain::sysex::PAGE_C_PART_BANK_CHANGE;
    use virus_syx::{
        encode_multi_dump, encode_param_change, encode_program_dump, part_bank_to_c32, Page,
        PartFlag, Patch,
    };

    fn settings() -> MidiSettings {
        MidiSettings {
            channel: 0,
            device_id: 0x00,
            part: EditPart::Single,
        }
    }

    fn param(name: &str) -> &'static MultiParamDef {
        MULTI_PARAMS.iter().find(|d| d.name == name).unwrap()
    }

    /// A multi with internal bytes set, all parts enabled with bit 4 (internal) set.
    fn sample(name: &str) -> Multi {
        let mut bytes = [0u8; MULTI_LEN];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = ((i * 13 + 3) % 128) as u8;
        }
        for state in &mut bytes[240..256] {
            *state = 0x51;
        }
        let mut multi = Multi::from_bytes(&bytes);
        multi.set_name(name);
        multi
    }

    fn sysex(frame: Vec<u8>) -> MidiEvent {
        MidiEvent::SysEx(frame)
    }

    #[test]
    fn edit_buffer_dump_loads_and_bank_dump_queues() {
        let mut s = MultiSession::default();
        let multi = sample("Sequencer");
        let edit = sysex(encode_multi_dump(0x00, 0, 0, multi.as_bytes()));
        assert_eq!(
            s.handle_midi(&edit, &settings()),
            MultiIncoming::Loaded {
                name: "Sequencer".into(),
                checksum_ok: true
            }
        );
        assert_eq!(s.multi(), Some(&multi));

        let bank = sysex(encode_multi_dump(0x00, 1, 5, sample("Other").as_bytes()));
        assert_eq!(
            s.handle_midi(&bank, &settings()),
            MultiIncoming::Queued {
                program: 5,
                checksum_ok: true
            }
        );
        assert_eq!(s.multi(), Some(&multi));
        assert_eq!(s.received.len(), 1);

        let other_device = sysex(encode_multi_dump(0x05, 0, 0, sample("X").as_bytes()));
        assert_eq!(
            s.handle_midi(&other_device, &settings()),
            MultiIncoming::Ignored
        );
        let single = sysex(encode_program_dump(0x00, 0, 0x40, Patch::init().as_bytes()));
        assert_eq!(s.handle_midi(&single, &settings()), MultiIncoming::Ignored);
        assert_eq!(s.multi(), Some(&multi));
    }

    #[test]
    fn incoming_page_c_change_updates_the_addressed_part_only() {
        let mut s = MultiSession::default();
        s.load(sample("M"), "test", false);
        let before = s.multi().unwrap().clone();
        let vol = sysex(encode_param_change(0, Page::PageC, 3, 39, 100));
        assert_eq!(
            s.handle_midi(&vol, &settings()),
            MultiIncoming::ParamChanged {
                name: "Part Volume",
                part: 3,
                value: 100
            }
        );
        let multi = s.multi().unwrap();
        assert_eq!(multi.part(3).volume, 100);
        let changed: Vec<usize> = (0..256)
            .filter(|&i| multi.as_bytes()[i] != before.as_bytes()[i])
            .collect();
        assert_eq!(changed, [144 + 3]);

        let disable = sysex(encode_param_change(0, Page::PageC, 2, 72, 0));
        s.handle_midi(&disable, &settings());
        assert_eq!(s.multi().unwrap().part_state(2), 0x50, "bit 4 kept");

        // Single-mode part 0x40, global params and sound params don't touch the multi.
        let before = s.multi().unwrap().clone();
        for (page, part, param) in [
            (Page::PageA, 0x40, 113),
            (Page::PageC, 0x40, 39),
            (Page::PageC, 0, 93),
            (Page::PageA, 0, 40),
        ] {
            let msg = sysex(encode_param_change(0, page, part, param, 1));
            assert_eq!(
                s.handle_midi(&msg, &settings()),
                MultiIncoming::Ignored,
                "{page:?} {param}"
            );
        }
        assert_eq!(s.multi(), Some(&before));
    }

    #[test]
    fn incoming_part_bank_change_is_stored_through_the_shared_c32_mapping() {
        let mut s = MultiSession::default();
        s.load(sample("M"), "test", false);
        let bank_b = 1;
        let change = sysex(encode_param_change(
            0,
            Page::PageC,
            4,
            PAGE_C_PART_BANK_CHANGE,
            part_bank_to_c32(bank_b),
        ));
        assert_eq!(
            s.handle_midi(&change, &settings()),
            MultiIncoming::ParamChanged {
                name: "Part Bank",
                part: 4,
                value: bank_b
            }
        );
        assert_eq!(s.multi().unwrap().as_bytes()[32 + 4], bank_b);
    }

    #[test]
    fn edits_send_page_c_to_the_part_and_flag_unsent_values() {
        let mut s = MultiSession::default();
        let mut st = settings();
        st.device_id = 0x03;
        assert_eq!(
            s.edit(&st, param("Part Volume"), 0, 1),
            None,
            "nothing loaded"
        );

        s.load(sample("M"), "test", false);
        let msg = s.edit(&st, param("Part Transpose"), 9, 76).unwrap();
        assert_eq!(msg, encode_param_change(0x03, Page::PageC, 9, 37, 76));
        let msg = s.edit(&st, param("Part Effect Send"), 1, 40).unwrap();
        assert_eq!(msg[6..10], [0x70, 0x01, 113, 40]);
        let msg = s.edit(&st, param("Note Steal Priority"), 4, 1).unwrap();
        assert_eq!(msg[6..10], [0x72, 0x04, 77, 1]);
        assert!(s.multi().unwrap().part(4).flag(PartFlag::PriorityHigh));
        assert_eq!(s.multi().unwrap().part_state(4), 0x71);
        assert!(!s.unsent_changes);

        let current = s.multi().unwrap().get(param("Part Transpose"), 9);
        assert_eq!(s.edit(&st, param("Part Transpose"), 9, current), None);

        assert_eq!(s.edit(&st, param("Multi Clock Tempo"), 0, 1), None);
        assert!(s.unsent_changes);
        let dump = s.edit_buffer_dump(&st).unwrap();
        assert_eq!(dump[6..9], [0x11, 0x00, 0x00]);
        assert_eq!(&dump[9..265], s.multi().unwrap().as_bytes());
        assert!(!s.unsent_changes);
    }

    #[test]
    fn set_name_sends_only_changed_chars() {
        let mut s = MultiSession::default();
        s.load(sample("Seq"), "test", false);
        let messages = s.set_name(&settings(), "Sex");
        assert_eq!(messages, [encode_param_change(0, Page::PageC, 0, 7, b'x')]);
        s.set_name(&settings(), "Ünïcode Multi!!");
        assert_eq!(s.multi().unwrap().name(), "ncode Mult");
    }

    #[test]
    fn requests() {
        let st = settings();
        assert_eq!(MultiSession::fetch_request(&st)[6..9], [0x31, 0x00, 0x00]);
        assert_eq!(MultiSession::bank_request(&st)[6..8], [0x33, 0x01]);
        assert_eq!(
            MultiSession::part_fetch_request(&st, 7)[6..9],
            [0x30, 0x00, 0x07]
        );
    }

    fn multi_file() -> Vec<u8> {
        let mut file = Vec::new();
        file.extend(encode_multi_dump(
            DEVICE_ID_OMNI,
            0,
            0,
            sample("EditBuf").as_bytes(),
        ));
        file.extend(encode_program_dump(
            DEVICE_ID_OMNI,
            1,
            0,
            Patch::init().as_bytes(),
        ));
        file.extend(encode_multi_dump(
            DEVICE_ID_OMNI,
            1,
            2,
            sample("Two").as_bytes(),
        ));
        file
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("virus-app-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn import_store_export_round_trip() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let pending = prepare_multi_import_bytes(&db, "f.syx", &multi_file()).unwrap();
        assert!(!pending.needs_confirmation());
        let outcome = commit_multi_import(&db, &pending, false).unwrap();
        let report = outcome.multis.as_ref().unwrap();
        assert_eq!((report.imported, report.loose.len()), (1, 1));
        assert!(multi_import_summary(&outcome).contains("1 Multi edit-buffer"));
        assert!(prepare_multi_import_bytes(
            &db,
            "f",
            &encode_program_dump(0, 1, 0, Patch::init().as_bytes())
        )
        .unwrap_err()
        .contains("No multi dumps"));
        assert!(
            db.get_banks().unwrap().len() == 4,
            "a multi import never creates library banks"
        );

        let id = db.get_multi_by_slot(2).unwrap().unwrap().id;
        db.set_multi_tags(id, "keep").unwrap();
        let mut edited = sample("Two");
        edited.set(param("Part Volume"), 0, 1);
        store_multi(&db, Some(2), &edited, true).unwrap();
        assert_eq!(db.get_multi_by_slot(2).unwrap().unwrap().tags, "keep");
        store_multi(&db, Some(2), &sample("Renamed"), true).unwrap();
        assert_eq!(db.get_multi_by_slot(2).unwrap().unwrap().tags, "");
        store_multi(&db, None, &sample("Loose"), true).unwrap();

        let mut cache = MultiLibraryCache::default();
        assert!(cache.refresh(&db));
        assert!(!cache.refresh(&db));
        assert_eq!(cache.slots[2].as_ref().unwrap().name, "Renamed");
        assert_eq!(cache.loose.len(), 1);

        let dir = temp_dir("multi-export");
        let path = dir.join("multis.syx");
        let result = export_multi_bank_file(&db, BadChecksumPolicy::Skip, &path).unwrap();
        assert_eq!(result.written, 1);
        let written = std::fs::read(&path).unwrap();
        assert_eq!(
            written,
            sample("Renamed").to_sysex(DEVICE_ID_OMNI, MultiDumpTarget::Bank { program: 2 })
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn occupied_db() -> PatchDatabase {
        let db = PatchDatabase::open_in_memory().unwrap();
        store_multi(&db, Some(3), &sample("Mine"), true).unwrap();
        db
    }

    #[test]
    fn importing_received_multis_over_occupied_slots_asks_first() {
        let db = occupied_db();
        let dumps = vec![incoming(1, 3, "Theirs", true), incoming(1, 4, "Free", true)];
        let pending = prepare_multi_import(&db, "received from synth", dumps).unwrap();
        assert!(pending.needs_confirmation());
        assert_eq!(pending.multi_overwrites.len(), 1);
        assert_eq!(
            (
                pending.multi_overwrites[0].slot,
                pending.multi_overwrites[0].old_name.as_str(),
                pending.multi_overwrites[0].new_name.as_str()
            ),
            (3, "Mine", "Theirs")
        );
        assert_eq!(
            db.get_multi_by_slot(4).unwrap(),
            None,
            "planning must not write"
        );
    }

    #[test]
    fn only_empty_slots_leaves_occupied_multis_alone() {
        let db = occupied_db();
        let pending = prepare_multi_import(
            &db,
            "f",
            vec![incoming(1, 3, "Theirs", true), incoming(1, 4, "Free", true)],
        )
        .unwrap();
        let outcome = commit_multi_import(&db, &pending, false).unwrap();
        assert_eq!(db.get_multi_by_slot(3).unwrap().unwrap().name, "Mine");
        assert_eq!(db.get_multi_by_slot(4).unwrap().unwrap().name, "Free");
        assert_eq!(outcome.multis_left_out, [3]);
        assert!(
            multi_import_summary(&outcome).contains("1 occupied slot(s) left alone (M003)"),
            "{}",
            multi_import_summary(&outcome)
        );
    }

    #[test]
    fn overwrite_replaces_occupied_multis() {
        let db = occupied_db();
        let pending = prepare_multi_import(&db, "f", vec![incoming(1, 3, "Theirs", true)]).unwrap();
        commit_multi_import(&db, &pending, true).unwrap();
        assert_eq!(db.get_multi_by_slot(3).unwrap().unwrap().name, "Theirs");
    }

    #[test]
    fn importing_a_file_over_occupied_slots_asks_first() {
        let db = PatchDatabase::open_in_memory().unwrap();
        store_multi(&db, Some(2), &sample("Mine"), true).unwrap();
        let pending = prepare_multi_import_bytes(&db, "f.syx", &multi_file()).unwrap();
        assert!(pending.needs_confirmation());
        assert_eq!(db.get_multi_by_slot(2).unwrap().unwrap().name, "Mine");
    }

    #[test]
    fn received_multis_are_one_per_slot_and_capped() {
        let mut list = Vec::new();
        queue_received_multi(&mut list, incoming(1, 0, "First", true));
        queue_received_multi(&mut list, incoming(1, 0, "Second", false));
        queue_received_multi(&mut list, incoming(1, 1, "Other", true));
        assert_eq!(list.len(), 2);
        assert_eq!(
            (list[0].data, list[0].checksum_ok),
            (*sample("Second").as_bytes(), false)
        );
        for i in 0..2000u32 {
            queue_received_multi(
                &mut list,
                incoming((i / 128) as u8, (i % 128) as u8, "x", true),
            );
        }
        assert_eq!(list.len(), MAX_RECEIVED);
    }

    #[test]
    fn a_second_bank_request_does_not_double_the_received_list() {
        let mut s = MultiSession::default();
        for _ in 0..2 {
            for program in 0..128u8 {
                let frame = sysex(encode_multi_dump(0x00, 1, program, sample("M").as_bytes()));
                s.handle_midi(&frame, &settings());
            }
        }
        assert_eq!(s.received.len(), 128);
    }

    #[test]
    fn a_multi_edit_buffer_with_a_wrong_checksum_is_flagged() {
        let mut s = MultiSession::default();
        let mut frame = encode_multi_dump(0x00, 0, 0, sample("Bad").as_bytes());
        frame[265] = (frame[265] + 1) & 0x7F;
        s.handle_midi(&sysex(frame), &settings());
        assert!(s.flagged());
        s.handle_midi(
            &sysex(encode_multi_dump(0x00, 0, 0, sample("Good").as_bytes())),
            &settings(),
        );
        assert!(!s.flagged());
    }

    #[test]
    fn editing_a_flagged_multi_does_not_clear_the_flag() {
        let mut s = MultiSession::default();
        s.load(sample("M"), "file (edit buffer)", true);
        s.edit(&settings(), param("Part Volume"), 0, 5);
        assert!(s.flagged());
    }

    #[test]
    fn storing_a_flagged_loose_multi_keeps_the_flag() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let loose = incoming(0, 0, "Loose", false);
        let id = store_multi(
            &db,
            None,
            &Multi::from_bytes(&loose.data),
            loose.checksum_ok,
        )
        .unwrap();
        assert!(!db.get_multi_by_id(id).unwrap().unwrap().checksum_ok);
    }

    #[test]
    fn a_flagged_multi_loaded_and_stored_elsewhere_stays_flagged() {
        let db = PatchDatabase::open_in_memory().unwrap();
        db.import_multi_dumps(&[incoming(1, 1, "Damaged", false)])
            .unwrap();
        let record = db.get_multi_by_slot(1).unwrap().unwrap();
        let mut s = MultiSession::default();
        s.load(
            Multi::from_bytes(&record.data),
            "library M001",
            !record.checksum_ok,
        );

        let text = store_edited_multi(&db, &s, 7).unwrap();
        assert!(text.contains("flagged ⚠"), "{text}");
        assert!(!db.get_multi_by_slot(7).unwrap().unwrap().checksum_ok);
    }

    #[test]
    fn delete_and_overwrite_wait_for_confirmation_and_name_what_is_lost() {
        let db = occupied_db();
        let mut s = MultiSession::default();
        s.load(sample("New"), "test", false);

        let overwrite = MultiConfirm::Overwrite {
            slot: 3,
            old_name: "Mine".into(),
        };
        assert!(
            overwrite.question(&s).contains("'Mine'"),
            "{}",
            overwrite.question(&s)
        );
        assert_eq!(
            db.get_multi_by_slot(3).unwrap().unwrap().name,
            "Mine",
            "nothing happens before apply"
        );
        overwrite.apply(&db, &s).unwrap();
        assert_eq!(db.get_multi_by_slot(3).unwrap().unwrap().name, "New");

        let id = db.get_multi_by_slot(3).unwrap().unwrap().id;
        let delete = MultiConfirm::Delete {
            id,
            label: "M003".into(),
            name: "New".into(),
        };
        assert!(delete.question(&s).contains("'New' (M003)"));
        assert_eq!(delete.apply(&db, &s).unwrap(), "Deleted 'New'.");
        assert_eq!(db.get_multi_by_slot(3).unwrap(), None);
        assert!(delete
            .apply(&db, &s)
            .unwrap_err()
            .contains("no longer exists"));
    }

    #[test]
    fn file_export_leaves_flagged_multis_out_by_default_and_says_so() {
        let db = PatchDatabase::open_in_memory().unwrap();
        db.import_multi_dumps(&[
            incoming(1, 0, "Fine", true),
            incoming(1, 1, "Damaged", false),
        ])
        .unwrap();
        let (bytes, result) = export_multi_bank_bytes(&db, BadChecksumPolicy::Skip).unwrap();
        assert_eq!(parse_multi_frames(&extract_sysex_frames(&bytes)).len(), 1);
        assert_eq!((result.written, result.skipped_flagged), (1, 1));
        let text = export_summary(&result, Path::new("m.syx"));
        assert!(text.contains("1 flagged ⚠ multi(s) left out"), "{text}");
    }

    #[test]
    fn file_export_as_stored_keeps_the_wrong_checksum() {
        let db = PatchDatabase::open_in_memory().unwrap();
        db.import_multi_dumps(&[incoming(1, 1, "Damaged", false)])
            .unwrap();
        let (bytes, result) = export_multi_bank_bytes(&db, BadChecksumPolicy::KeepInvalid).unwrap();
        assert_eq!(result.included_flagged, 1);
        let parsed = parse_multi_frames(&extract_sysex_frames(&bytes));
        assert!(
            !parsed[0].checksum_ok,
            "re-importing the file flags it again"
        );
    }

    #[test]
    fn exporting_only_flagged_multis_with_skip_is_an_error() {
        let db = PatchDatabase::open_in_memory().unwrap();
        db.import_multi_dumps(&[incoming(1, 1, "Damaged", false)])
            .unwrap();
        let err = export_multi_bank_bytes(&db, BadChecksumPolicy::Skip).unwrap_err();
        assert!(err.contains("flagged"), "{err}");
    }

    #[test]
    fn import_summary_reports_duplicate_slots() {
        let report = MultiImportReport {
            imported: 1,
            duplicates: vec![13],
            ..Default::default()
        };
        assert!(import_summary(&report)
            .contains("1 Multi bank slot(s) were addressed by more than one dump"));
    }

    #[test]
    fn single_names_follow_the_library() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let b = db
            .get_banks()
            .unwrap()
            .into_iter()
            .find(|b| b.bank_letter.as_deref() == Some("B"))
            .unwrap()
            .id;
        let mut names = SingleNames::default();
        names.refresh(&db);
        assert_eq!(names.name(1, 70), None);
        let mut patch = Patch::init();
        patch.set_name("Kick 1");
        db.save_patch(b, 70, patch.as_bytes(), &PatchMeta::default(), true)
            .unwrap();
        assert!(names.refresh(&db));
        assert_eq!(names.name(1, 70), Some("Kick 1"));
        assert_eq!(names.name(0, 70), None);
        assert_eq!(names.name(9, 0), None);
    }

    #[test]
    fn edited_part_channel_comes_from_the_multi() {
        let mut s = MultiSession::default();
        assert_eq!(
            s.edited_part_channel(EditPart::Multi(6)),
            None,
            "no multi fetched"
        );

        let mut multi = sample("M");
        multi.set(param("Part MIDI Channel"), 6, 0);
        s.load(multi, "test", false);
        assert_eq!(s.edited_part_channel(EditPart::Multi(6)), Some(0));
        assert_eq!(s.edited_part_channel(EditPart::Single), None);

        // A live C34 change from the synth moves it too.
        let c34 = sysex(encode_param_change(0, Page::PageC, 6, 34, 9));
        s.handle_midi(&c34, &settings());
        assert_eq!(s.edited_part_channel(EditPart::Multi(6)), Some(9));
    }

    #[test]
    fn panic_channels_cover_enabled_parts_in_multi_mode_only() {
        let mut s = MultiSession::default();
        let single = MidiSettings {
            channel: 10,
            device_id: 0,
            part: EditPart::Single,
        };
        let multi_mode = MidiSettings {
            part: EditPart::Multi(0),
            ..single
        };
        assert_eq!(
            s.panic_channels(&multi_mode),
            vec![10],
            "no multi fetched yet"
        );

        let mut multi = sample("M");
        for part in 0..16 {
            multi.set_part_enabled(part, false);
        }
        for (part, channel) in [(0u8, 0u8), (1, 10), (2, 2), (3, 2)] {
            multi.set_part_enabled(part, true);
            multi.set(param("Part MIDI Channel"), part, channel);
        }
        multi.set(param("Part MIDI Channel"), 9, 7); // disabled part: not touched
        s.load(multi, "test", false);
        assert_eq!(s.panic_channels(&multi_mode), vec![10, 0, 2]);
        assert_eq!(s.panic_channels(&single), vec![10]);
    }

    #[test]
    fn midi_volume_init_zero_is_off() {
        assert_eq!(display_value(param("Part MIDI Volume Init"), 0), "Off");
        assert_eq!(display_value(param("Part MIDI Volume Init"), 100), "100");
        assert_eq!(display_value(param("Part Volume"), 0), "-64");
    }

    fn incoming(bank: u8, program: u8, name: &str, checksum_ok: bool) -> IncomingMultiDump {
        IncomingMultiDump {
            bank,
            program,
            data: *sample(name).as_bytes(),
            checksum_ok,
        }
    }

    #[test]
    fn a_complete_multi_bank_needs_all_128_programs() {
        let mut dumps: Vec<IncomingMultiDump> =
            (0..127).map(|p| incoming(1, p, "M", true)).collect();
        dumps.push(incoming(0, 127, "EditBuf", true));
        dumps.push(incoming(1, 5, "Again", true));
        assert!(
            !is_complete_multi_bank(&dumps),
            "edit buffers and repeats don't count"
        );
        dumps.push(incoming(1, 127, "Last", true));
        assert!(is_complete_multi_bank(&dumps));
    }

    #[test]
    fn backup_bytes_reimport_byte_exact_and_keep_bad_checksums_detectable() {
        let dumps = [
            incoming(1, 0, "Good", true),
            incoming(1, 9, "Damaged", false),
        ];
        let bytes = multi_backup_bytes(&dumps);
        let parsed = parse_multi_frames(&extract_sysex_frames(&bytes));
        assert_eq!(parsed.len(), 2);
        assert_eq!(
            (parsed[0].data, parsed[0].checksum_ok),
            (dumps[0].data, true)
        );
        assert_eq!(
            (parsed[1].program, parsed[1].data, parsed[1].checksum_ok),
            (9, dumps[1].data, false)
        );
    }
}

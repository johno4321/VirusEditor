//! Multi mode logic (TODO F4/F5) without egui: the Multi edit buffer, incoming multi dumps and
//! parameter changes, live edits via Page C, librarian glue for multis, and the lookup of the
//! single names that the parts play.
//!
//! Nothing here touches egui or a real MIDI port, so it is unit-tested directly.

use std::path::Path;

use virus_db::{DbError, IncomingMultiDump, MultiImportReport, MultiSummary, PatchDatabase, PatchMeta};
use virus_midi::MidiEvent;
use virus_syx::{
    encode_dump_request, encode_multi_bank_request, encode_multi_dump, encode_multi_request, extract_sysex_frames,
    multi_param_for_live, parse_sysex, Multi, MultiDumpTarget, MultiParamDef, SysExMessage, ValueKind,
    DEVICE_ID_OMNI, MULTI_BANK, MULTI_BANK_SLOTS, MULTI_PARAMS, MULTI_PARTS, PART_BANK_NAMES,
};

use crate::session::{EditPart, MidiSettings};
use crate::views::Notice;

/// What an incoming MIDI event meant for the Multi edit buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultiIncoming {
    /// A Multi edit-buffer dump (`11 00`) replaced the edited multi.
    Loaded { name: String, checksum_ok: bool },
    /// A Multi bank dump (`11 01 mm`), queued in [`MultiSession::received`].
    Queued { program: u8, checksum_ok: bool },
    /// A parameter change updated one value of the edited multi.
    ParamChanged { name: &'static str, part: u8, value: u8 },
    Ignored,
}

impl MultiIncoming {
    /// Status-bar text for events worth reporting.
    pub fn notice(&self, received: usize) -> Option<Notice> {
        match self {
            Self::Loaded { name, checksum_ok: true } => Some(Notice::info(format!("Received multi '{name}' from the synth."))),
            Self::Loaded { name, checksum_ok: false } => {
                Some(Notice::warning(format!("Received multi '{name}' from the synth with a wrong checksum.")))
            }
            Self::Queued { program, checksum_ok } => {
                let flag = if *checksum_ok { "" } else { " (wrong checksum)" };
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
    pub received: Vec<IncomingMultiDump>,
}

impl MultiSession {
    pub fn multi(&self) -> Option<&Multi> {
        self.multi.as_ref()
    }

    pub fn load(&mut self, multi: Multi, origin: impl Into<String>) {
        self.multi = Some(multi);
        self.origin = origin.into();
        self.unsent_changes = false;
    }

    /// Interprets one event from the MIDI worker. Only SysEx is relevant: multi dumps and
    /// parameter changes that address a Multi value (SPEC §4.3). Never consumes the event; the
    /// single-sound session sees it too.
    pub fn handle_midi(&mut self, event: &MidiEvent, settings: &MidiSettings) -> MultiIncoming {
        let MidiEvent::SysEx(frame) = event else { return MultiIncoming::Ignored };
        let Ok(message) = parse_sysex(frame) else { return MultiIncoming::Ignored };
        let dd = message.device_id();
        if !(settings.device_id == DEVICE_ID_OMNI || dd == DEVICE_ID_OMNI || dd == settings.device_id) {
            return MultiIncoming::Ignored;
        }
        match message {
            SysExMessage::MultiDump { bank, program, data, checksum_ok, .. } => match MultiDumpTarget::from_raw(bank, program) {
                Some(MultiDumpTarget::EditBuffer) => {
                    self.load(Multi::from_bytes(&data), "synth");
                    MultiIncoming::Loaded { name: self.multi.as_ref().map(Multi::name).unwrap_or_default(), checksum_ok }
                }
                Some(MultiDumpTarget::Bank { program }) => {
                    self.received.push(IncomingMultiDump { bank, program, data: *data, checksum_ok });
                    MultiIncoming::Queued { program, checksum_ok }
                }
                None => MultiIncoming::Ignored,
            },
            SysExMessage::ParamChange { page, part, param, value, .. } => {
                let (Some(multi), Some(def)) = (self.multi.as_mut(), multi_param_for_live(page, param)) else {
                    return MultiIncoming::Ignored;
                };
                // Part parameters are addressed to parts 0..15; part 0x40 is the Single buffer.
                if def.is_part_param() && part as usize >= MULTI_PARTS {
                    return MultiIncoming::Ignored;
                }
                multi.set(def, part, value);
                MultiIncoming::ParamChanged { name: def.name, part, value: multi.get(def, part) }
            }
            _ => MultiIncoming::Ignored,
        }
    }

    /// Changes one value of the edited multi and returns the parameter change to send, if the
    /// parameter has one. Values without one set [`Self::unsent_changes`].
    pub fn edit(&mut self, settings: &MidiSettings, def: &MultiParamDef, part: u8, value: u8) -> Option<Vec<u8>> {
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
        let Some(multi) = self.multi.as_mut() else { return Vec::new() };
        let before = multi.clone();
        multi.set_name(&crate::session::sanitize_name(name));
        MULTI_PARAMS
            .iter()
            .filter(|def| def.kind == ValueKind::NameChar && multi.get(def, 0) != before.get(def, 0))
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
        self.multi.as_ref().map(|m| m.part(part).midi_channel & 0x0F)
    }

    /// The edited Multi part's channel from the multi, for note audition (TODO N9); `None` in
    /// Single mode or before a multi has been fetched.
    pub fn edited_part_channel(&self, part: EditPart) -> Option<u8> {
        match part {
            EditPart::Single => None,
            EditPart::Multi(part) => self.part_channel(part),
        }
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
            Ok(SysExMessage::MultiDump { bank, program, data, checksum_ok, .. }) => {
                Some(IncomingMultiDump { bank, program, data: *data, checksum_ok })
            }
            _ => None,
        })
        .collect()
}

/// Imports the multi dumps of a `.syx` / `.mid` file: `bb = 01` → Multi bank slot `mm`,
/// `bb = 00` → returned as loose. Singles and other messages are ignored here.
pub fn import_multi_bytes(db: &PatchDatabase, bytes: &[u8]) -> Result<MultiImportReport, String> {
    let dumps = parse_multi_frames(&extract_sysex_frames(bytes));
    if dumps.is_empty() {
        return Err("No multi dumps found in the file.".to_owned());
    }
    db.import_multi_dumps(&dumps).map_err(|e| format!("Multi import failed: {e}"))
}

pub fn import_multi_file(db: &PatchDatabase, path: &Path) -> Result<MultiImportReport, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    import_multi_bytes(db, &bytes)
}

pub fn import_summary(report: &MultiImportReport) -> String {
    let mut parts = vec![format!("Imported {} multi(s) into the Multi bank", report.imported)];
    if report.bad_checksum > 0 {
        parts.push(format!("{} with a wrong checksum (imported and flagged ⚠)", report.bad_checksum));
    }
    if !report.loose.is_empty() {
        parts.push(format!("{} Multi edit-buffer dump(s) not stored in a slot", report.loose.len()));
    }
    if report.skipped > 0 {
        parts.push(format!("{} dump(s) skipped (unsupported bank or program)", report.skipped));
    }
    parts.join("; ") + "."
}

/// Pause between dumps when sending the Multi bank (F9). The Virus sends its own Multi bank
/// dumps about 339 ms apart (live test, 24 Nov), so 300 ms keeps a similar pace.
pub const MULTI_BANK_SEND_GAP_MS: u64 = 300;

/// Whether `dumps` hold all 128 Multi bank programs, i.e. a full backup of the synth's bank.
pub fn is_complete_multi_bank(dumps: &[IncomingMultiDump]) -> bool {
    let mut programs: Vec<u8> = dumps.iter().filter(|d| d.bank == MULTI_BANK).map(|d| d.program).collect();
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

/// Writes the Multi bank as a `.syx` file (device ID Omni). Returns the number of multis.
pub fn export_multi_bank_file(db: &PatchDatabase, path: &Path) -> Result<usize, String> {
    let frames = db.export_multi_bank_dumps(DEVICE_ID_OMNI).map_err(|e| format!("Export failed: {e}"))?;
    if frames.is_empty() {
        return Err("Export failed: the Multi bank is empty.".to_owned());
    }
    std::fs::write(path, frames.concat()).map_err(|e| format!("Cannot write {}: {e}", path.display()))?;
    Ok(frames.len())
}

/// Stores `multi` byte-exact in a Multi bank slot (`None` = unslotted). Tags and favourite are
/// kept when the slot already holds a multi of the same name.
pub fn store_multi(db: &PatchDatabase, slot: Option<u8>, multi: &Multi) -> Result<i64, DbError> {
    let meta = match slot.map(|s| db.get_multi_by_slot(s)).transpose()?.flatten() {
        Some(existing) if existing.name == multi.name() => PatchMeta { tags: existing.tags, favorite: existing.favorite },
        _ => PatchMeta::default(),
    };
    db.save_multi(slot, multi.as_bytes(), &meta)
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
        match db.get_multi_slots().and_then(|slots| Ok((slots, db.get_loose_multis()?))) {
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
        let Ok(banks) = db.get_banks() else { return true };
        for (index, letter) in PART_BANK_NAMES.iter().enumerate() {
            let Some(bank) = banks.iter().find(|b| b.bank_letter.as_deref() == Some(*letter)) else { continue };
            if let Ok(slots) = db.get_bank_slots(bank.id) {
                self.banks[index] = slots.into_iter().map(|s| s.map(|s| s.name)).collect();
            }
        }
        true
    }

    /// The library's name for part bank byte `bank` (0 = A, 1 = B, …) and `program`, if known.
    pub fn name(&self, bank: u8, program: u8) -> Option<&str> {
        self.banks.get(bank as usize)?.get(program as usize)?.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::EditPart;
    use virus_db::MULTI_LEN;
    use virus_syx::{encode_multi_dump, encode_param_change, encode_program_dump, Page, Patch, PartFlag};

    fn settings() -> MidiSettings {
        MidiSettings { channel: 0, device_id: 0x00, part: EditPart::Single }
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
        assert_eq!(s.handle_midi(&edit, &settings()), MultiIncoming::Loaded { name: "Sequencer".into(), checksum_ok: true });
        assert_eq!(s.multi(), Some(&multi));

        let bank = sysex(encode_multi_dump(0x00, 1, 5, sample("Other").as_bytes()));
        assert_eq!(s.handle_midi(&bank, &settings()), MultiIncoming::Queued { program: 5, checksum_ok: true });
        assert_eq!(s.multi(), Some(&multi));
        assert_eq!(s.received.len(), 1);

        let other_device = sysex(encode_multi_dump(0x05, 0, 0, sample("X").as_bytes()));
        assert_eq!(s.handle_midi(&other_device, &settings()), MultiIncoming::Ignored);
        let single = sysex(encode_program_dump(0x00, 0, 0x40, Patch::init().as_bytes()));
        assert_eq!(s.handle_midi(&single, &settings()), MultiIncoming::Ignored);
        assert_eq!(s.multi(), Some(&multi));
    }

    #[test]
    fn incoming_page_c_change_updates_the_addressed_part_only() {
        let mut s = MultiSession::default();
        s.load(sample("M"), "test");
        let before = s.multi().unwrap().clone();
        let vol = sysex(encode_param_change(0, Page::PageC, 3, 39, 100));
        assert_eq!(
            s.handle_midi(&vol, &settings()),
            MultiIncoming::ParamChanged { name: "Part Volume", part: 3, value: 100 }
        );
        let multi = s.multi().unwrap();
        assert_eq!(multi.part(3).volume, 100);
        let changed: Vec<usize> =
            (0..256).filter(|&i| multi.as_bytes()[i] != before.as_bytes()[i]).collect();
        assert_eq!(changed, [144 + 3]);

        let disable = sysex(encode_param_change(0, Page::PageC, 2, 72, 0));
        s.handle_midi(&disable, &settings());
        assert_eq!(s.multi().unwrap().part_state(2), 0x50, "bit 4 kept");

        // Single-mode part 0x40, global params and sound params don't touch the multi.
        let before = s.multi().unwrap().clone();
        for (page, part, param) in [(Page::PageA, 0x40, 113), (Page::PageC, 0x40, 39), (Page::PageC, 0, 93), (Page::PageA, 0, 40)] {
            let msg = sysex(encode_param_change(0, page, part, param, 1));
            assert_eq!(s.handle_midi(&msg, &settings()), MultiIncoming::Ignored, "{page:?} {param}");
        }
        assert_eq!(s.multi(), Some(&before));
    }

    #[test]
    fn edits_send_page_c_to_the_part_and_flag_unsent_values() {
        let mut s = MultiSession::default();
        let mut st = settings();
        st.device_id = 0x03;
        assert_eq!(s.edit(&st, param("Part Volume"), 0, 1), None, "nothing loaded");

        s.load(sample("M"), "test");
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
        s.load(sample("Seq"), "test");
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
        assert_eq!(MultiSession::part_fetch_request(&st, 7)[6..9], [0x30, 0x00, 0x07]);
    }

    #[test]
    fn import_store_export_round_trip() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let mut file = Vec::new();
        file.extend(encode_multi_dump(DEVICE_ID_OMNI, 0, 0, sample("EditBuf").as_bytes()));
        file.extend(encode_program_dump(DEVICE_ID_OMNI, 1, 0, Patch::init().as_bytes()));
        file.extend(encode_multi_dump(DEVICE_ID_OMNI, 1, 2, sample("Two").as_bytes()));
        let report = import_multi_bytes(&db, &file).unwrap();
        assert_eq!((report.imported, report.loose.len()), (1, 1));
        assert!(import_summary(&report).contains("1 Multi edit-buffer"));
        assert!(import_multi_bytes(&db, &encode_program_dump(0, 1, 0, Patch::init().as_bytes()))
            .unwrap_err()
            .contains("No multi dumps"));

        let id = db.get_multi_by_slot(2).unwrap().unwrap().id;
        db.set_multi_tags(id, "keep").unwrap();
        let mut edited = sample("Two");
        edited.set(param("Part Volume"), 0, 1);
        store_multi(&db, Some(2), &edited).unwrap();
        assert_eq!(db.get_multi_by_slot(2).unwrap().unwrap().tags, "keep");
        store_multi(&db, Some(2), &sample("Renamed")).unwrap();
        assert_eq!(db.get_multi_by_slot(2).unwrap().unwrap().tags, "");
        store_multi(&db, None, &sample("Loose")).unwrap();

        let mut cache = MultiLibraryCache::default();
        assert!(cache.refresh(&db));
        assert!(!cache.refresh(&db));
        assert_eq!(cache.slots[2].as_ref().unwrap().name, "Renamed");
        assert_eq!(cache.loose.len(), 1);

        let dir = std::env::temp_dir().join(format!("virus-app-multi-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("multis.syx");
        assert_eq!(export_multi_bank_file(&db, &path), Ok(1));
        let written = std::fs::read(&path).unwrap();
        assert_eq!(written, sample("Renamed").to_sysex(DEVICE_ID_OMNI, MultiDumpTarget::Bank { program: 2 }));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn single_names_follow_the_library() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let b = db.get_banks().unwrap().into_iter().find(|b| b.bank_letter.as_deref() == Some("B")).unwrap().id;
        let mut names = SingleNames::default();
        names.refresh(&db);
        assert_eq!(names.name(1, 70), None);
        let mut patch = Patch::init();
        patch.set_name("Kick 1");
        db.save_patch(b, 70, patch.as_bytes(), &PatchMeta::default()).unwrap();
        assert!(names.refresh(&db));
        assert_eq!(names.name(1, 70), Some("Kick 1"));
        assert_eq!(names.name(0, 70), None);
        assert_eq!(names.name(9, 0), None);
    }

    #[test]
    fn edited_part_channel_comes_from_the_multi() {
        let mut s = MultiSession::default();
        assert_eq!(s.edited_part_channel(EditPart::Multi(6)), None, "no multi fetched");

        let mut multi = sample("M");
        multi.set(param("Part MIDI Channel"), 6, 0);
        s.load(multi, "test");
        assert_eq!(s.edited_part_channel(EditPart::Multi(6)), Some(0));
        assert_eq!(s.edited_part_channel(EditPart::Single), None);

        // A live C34 change from the synth moves it too.
        let c34 = sysex(encode_param_change(0, Page::PageC, 6, 34, 9));
        s.handle_midi(&c34, &settings());
        assert_eq!(s.edited_part_channel(EditPart::Multi(6)), Some(9));
    }

    #[test]
    fn midi_volume_init_zero_is_off() {
        assert_eq!(display_value(param("Part MIDI Volume Init"), 0), "Off");
        assert_eq!(display_value(param("Part MIDI Volume Init"), 100), "100");
        assert_eq!(display_value(param("Part Volume"), 0), "-64");
    }

    fn incoming(bank: u8, program: u8, name: &str, checksum_ok: bool) -> IncomingMultiDump {
        IncomingMultiDump { bank, program, data: *sample(name).as_bytes(), checksum_ok }
    }

    #[test]
    fn a_complete_multi_bank_needs_all_128_programs() {
        let mut dumps: Vec<IncomingMultiDump> = (0..127).map(|p| incoming(1, p, "M", true)).collect();
        dumps.push(incoming(0, 127, "EditBuf", true));
        dumps.push(incoming(1, 5, "Again", true));
        assert!(!is_complete_multi_bank(&dumps), "edit buffers and repeats don't count");
        dumps.push(incoming(1, 127, "Last", true));
        assert!(is_complete_multi_bank(&dumps));
    }

    #[test]
    fn backup_bytes_reimport_byte_exact_and_keep_bad_checksums_detectable() {
        let dumps = [incoming(1, 0, "Good", true), incoming(1, 9, "Damaged", false)];
        let bytes = multi_backup_bytes(&dumps);
        let parsed = parse_multi_frames(&extract_sysex_frames(&bytes));
        assert_eq!(parsed.len(), 2);
        assert_eq!((parsed[0].data, parsed[0].checksum_ok), (dumps[0].data, true));
        assert_eq!((parsed[1].program, parsed[1].data, parsed[1].checksum_ok), (9, dumps[1].data, false));
    }
}

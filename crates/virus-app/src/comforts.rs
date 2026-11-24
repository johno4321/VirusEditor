//! Editor comforts state (E1–E5): original patch, undo history, compare and section clipboard.
//!
//! Pure logic over [`Session`]; the app only forwards edits and sends the returned SysEx messages.

use virus_syx::Patch;

use crate::compare::Compare;
use crate::history::{apply_values, CoalesceKey, History};
use crate::sections::{self, SectionClip};
use crate::session::Session;
use crate::views::Edit;

/// A library slot a loaded patch came from, for "Select on synth" (F7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LibrarySlot {
    pub bank_id: i64,
    pub program: u8,
}

/// A replacement of the edited patch, held until the user confirms (E5).
#[derive(Debug, Clone, PartialEq)]
pub enum Replace {
    /// `select`: also switch the synth to this library slot, once the load has happened (E6).
    Load { patch: Patch, origin: String, select: Option<LibrarySlot> },
    SendToEditBuffer { patch: Patch, origin: String },
    Init,
    Fetch,
}

impl Replace {
    /// What the confirmation says happens to the unstored edits of `name`. A fetched dump over
    /// an edited patch is kept as one undo step (`Comforts::adopt_dump`); the others start a new
    /// history.
    pub fn warning(&self, name: &str) -> String {
        let intro = format!("'{name}' has edits that are not stored in the library.");
        match self {
            Replace::Fetch => format!(
                "{intro} Fetching replaces them with the synth's edit buffer; they stay in the undo history, \
                 so Undo brings them back."
            ),
            _ => format!("{intro} Replacing it discards them and the undo history."),
        }
    }
}

/// Holds a [`Replace`] while the "replace the edited patch?" dialog is open.
#[derive(Debug, Default)]
pub struct ReplaceRequest {
    pending: Option<Replace>,
}

impl ReplaceRequest {
    /// The action to perform now, or `None` if it waits for the user because the patch is edited.
    pub fn request(&mut self, action: Replace, edited: bool) -> Option<Replace> {
        if edited {
            self.pending = Some(action);
            None
        } else {
            Some(action)
        }
    }

    pub fn pending(&self) -> Option<&Replace> {
        self.pending.as_ref()
    }

    /// The user's answer: `true` returns the action to perform; `false` drops it with everything
    /// it would have done, including the synth selection.
    pub fn resolve(&mut self, replace: bool) -> Option<Replace> {
        self.pending.take().filter(|_| replace)
    }
}

#[derive(Debug, Default)]
pub struct Comforts {
    /// The patch as loaded (library slot, file, synth fetch, init) or last stored.
    original: Patch,
    history: History,
    compare: Compare,
    /// See [`Comforts::on_synth`].
    on_synth: bool,
}

/// Label and coalescing key for the edits of one frame.
pub fn describe(edits: &[Edit]) -> (String, Option<CoalesceKey>) {
    let key_of = |e: &Edit| match e {
        Edit::Param { page, number, .. } => {
            Some((if *page == virus_syx::Page::PageA { 0 } else { 128 }) + u32::from(*number))
        }
        Edit::Name(_) => Some(1000),
    };
    let label_of = |e: &Edit| match e {
        Edit::Param { page, number, .. } => {
            virus_syx::single_param(*page, *number).map_or_else(|| format!("param {number}"), |d| d.name.to_owned())
        }
        Edit::Name(_) => "name".to_owned(),
    };
    match edits {
        [] => (String::new(), None),
        [first, rest @ ..] => {
            let key = key_of(first);
            let same = rest.iter().all(|e| key_of(e) == key);
            (label_of(first), if same { key } else { None })
        }
    }
}

impl Comforts {
    pub fn new(original: Patch) -> Self {
        Self { original, ..Self::default() }
    }

    /// A new patch replaced the edited one (load, init, fetch): new original, empty history,
    /// compare ended (nothing is sent). `on_synth` says whether the synth's edit buffer holds it.
    pub fn reset(&mut self, original: Patch, on_synth: bool) {
        *self = Self { on_synth, ..Self::new(original) };
    }

    /// The editor bytes equal the synth's edit buffer, as far as we know (RV-M5). False after
    /// Load/Init, a changed edited part/device, or when nothing could be sent, until Send or Fetch.
    pub fn on_synth(&self) -> bool {
        self.on_synth
    }

    pub fn mark_sent(&mut self) {
        self.on_synth = true;
    }

    pub fn mark_unsynced(&mut self) {
        self.on_synth = false;
    }

    /// An edit-buffer dump from the synth replaced a dirty patch (`previous`, the edited bytes).
    /// The replacement becomes the original and the edits stay recoverable as one undo step.
    pub fn adopt_dump(&mut self, previous: &Patch, session: &Session, now: f64) {
        self.compare.discard();
        self.history.seal();
        self.history.record(previous, session.patch(), "synth dump", None, now);
        self.original = session.patch().clone();
        self.on_synth = true;
    }

    /// The edited patch was stored: it becomes the original; undo history stays.
    pub fn mark_stored(&mut self, stored: Patch) {
        self.original = stored;
    }

    pub fn original(&self) -> &Patch {
        &self.original
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    pub fn comparing(&self) -> bool {
        self.compare.is_active()
    }

    /// The edited patch, also while the session temporarily holds the original (Compare).
    pub fn edited<'a>(&'a self, session: &'a Session) -> &'a Patch {
        self.compare.edited().unwrap_or_else(|| session.patch())
    }

    pub fn is_edited(&self, session: &Session) -> bool {
        self.edited(session) != &self.original
    }

    /// Records the effect of this frame's edits (`before` → the session's patch).
    pub fn record(&mut self, before: &Patch, session: &Session, edits: &[Edit], now: f64) {
        let (label, key) = describe(edits);
        self.history.record(before, session.patch(), &label, key, now);
    }

    /// Ends drag coalescing (pointer released).
    pub fn seal(&mut self) {
        self.history.seal();
    }

    pub fn undo(&mut self, session: &mut Session) -> Vec<Vec<u8>> {
        if self.comparing() {
            return Vec::new();
        }
        self.history.undo(session)
    }

    pub fn redo(&mut self, session: &mut Session) -> Vec<Vec<u8>> {
        if self.comparing() {
            return Vec::new();
        }
        self.history.redo(session)
    }

    pub fn toggle_compare(&mut self, session: &mut Session) -> Vec<Vec<u8>> {
        if self.comparing() {
            self.compare.leave(session)
        } else {
            self.compare.enter(session, &self.original)
        }
    }

    /// Pastes (or inits) a section into the edited patch as one undo step.
    pub fn paste_section(&mut self, session: &mut Session, clip: &SectionClip, verb: &str, now: f64) -> Vec<Vec<u8>> {
        if self.comparing() {
            return Vec::new();
        }
        let before = session.patch().clone();
        let messages = apply_values(session, sections::changes(&before, clip));
        self.history.seal();
        self.history.record(&before, session.patch(), &format!("{verb} {}", clip.section), None, now);
        messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use virus_syx::{parse_sysex, read_sysex_file, Page, SysExMessage};

    fn bank_a() -> Vec<Patch> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/factory/Bank_A.syx");
        read_sysex_file(path)
            .expect("factory bank")
            .iter()
            .map(|f| match parse_sysex(f).expect("single dump") {
                SysExMessage::SingleDump { data, .. } => Patch::from_bytes(&data),
                other => panic!("{other:?}"),
            })
            .collect()
    }

    fn setup() -> (Comforts, Session, Vec<Patch>) {
        let banks = bank_a();
        let mut session = Session::default();
        session.load_patch(banks[4].clone());
        (Comforts::new(banks[4].clone()), session, banks)
    }

    fn edit(c: &mut Comforts, s: &mut Session, number: u8, value: u8, now: f64) {
        let before = s.patch().clone();
        let edits = [Edit::Param { page: Page::PageA, number, value }];
        s.edit(Page::PageA, number, value);
        c.record(&before, s, &edits, now);
    }

    #[test]
    fn edited_flag_follows_the_bytes() {
        let (mut c, mut s, _) = setup();
        assert!(!c.is_edited(&s));
        let old = s.patch().get(Page::PageA, 40);
        edit(&mut c, &mut s, 40, old ^ 0x20, 0.0);
        assert!(c.is_edited(&s));
        c.undo(&mut s);
        assert!(!c.is_edited(&s), "undoing back to the original clears the flag");
    }

    #[test]
    fn compare_toggles_and_blocks_undo_and_paste() {
        let (mut c, mut s, _) = setup();
        let old = s.patch().get(Page::PageA, 40);
        edit(&mut c, &mut s, 40, old ^ 0x20, 0.0);
        let edited = s.patch().clone();

        let to_original = c.toggle_compare(&mut s);
        assert_eq!(to_original.len(), 1);
        assert_eq!(s.patch().get(Page::PageA, 40), old);
        assert!(c.is_edited(&s), "still edited while comparing");
        assert_eq!(c.edited(&s), &edited);
        assert!(c.undo(&mut s).is_empty());
        let clip = sections::init_clip("Filters").unwrap();
        assert!(c.paste_section(&mut s, &clip, "Init", 1.0).is_empty());

        c.toggle_compare(&mut s);
        assert_eq!(s.patch(), &edited);
    }

    #[test]
    fn reset_starts_a_new_history() {
        let (mut c, mut s, banks) = setup();
        edit(&mut c, &mut s, 40, 3, 0.0);
        s.load_patch(banks[9].clone());
        c.reset(banks[9].clone(), false);
        assert!(!c.on_synth());
        assert!(!c.history().can_undo());
        assert!(!c.is_edited(&s));
    }

    #[test]
    fn paste_section_is_one_undo_step_that_sends_changes() {
        let (mut c, mut s, banks) = setup();
        let before = s.patch().clone();
        let clip = sections::copy(&banks[50], "Filters").unwrap();
        let expected = sections::differing(&before, &clip);
        assert!(expected > 0);
        let messages = c.paste_section(&mut s, &clip, "Paste", 0.0);
        assert_eq!(messages.len(), expected);
        assert_eq!(c.history().undo_label(), Some("Paste Filters"));
        c.undo(&mut s);
        assert_eq!(s.patch(), &before);
    }

    #[test]
    fn mark_stored_clears_edited_but_keeps_undo() {
        let (mut c, mut s, _) = setup();
        let old = s.patch().get(Page::PageA, 40);
        edit(&mut c, &mut s, 40, old ^ 0x20, 0.0);
        c.mark_stored(s.patch().clone());
        assert!(!c.is_edited(&s));
        assert!(c.history().can_undo());
    }

    #[test]
    fn sync_state_transitions() {
        let (mut c, mut s, banks) = setup();
        assert!(!c.on_synth(), "a freshly loaded/init patch is not on the synth");
        c.mark_sent();
        assert!(c.on_synth());
        let old = s.patch().get(Page::PageA, 40);
        edit(&mut c, &mut s, 40, old ^ 0x20, 0.0);
        assert!(c.on_synth() && c.is_edited(&s), "edits are sent, so still in sync, and dirty");
        c.mark_unsynced(); // part change or no output
        assert!(!c.on_synth());
        c.reset(banks[1].clone(), true); // fetch
        assert!(c.on_synth() && !c.history().can_undo());
    }

    #[test]
    fn incoming_dump_over_dirty_patch_stays_undoable() {
        let (mut c, mut s, banks) = setup();
        let old = s.patch().get(Page::PageA, 40);
        edit(&mut c, &mut s, 40, old ^ 0x20, 0.0);
        let dirty = s.patch().clone();
        let history_before = c.history().can_undo();
        assert!(history_before);

        s.load_patch(banks[8].clone()); // what handle_midi does for a dump
        c.adopt_dump(&dirty, &s, 1.0);
        assert!(c.on_synth() && !c.is_edited(&s));
        assert_eq!(c.history().undo_label(), Some("synth dump"));

        let messages = c.undo(&mut s);
        assert!(!messages.is_empty());
        assert_eq!(s.patch(), &dirty, "undo restores the discarded edits exactly");
        assert!(c.is_edited(&s));
    }

    #[test]
    fn describe_names_and_keys() {
        let one = [Edit::Param { page: Page::PageA, number: 40, value: 1 }];
        let (label, key) = describe(&one);
        assert_eq!(label, "Cutoff");
        assert!(key.is_some());
        let two = [one[0].clone(), Edit::Name("x".into())];
        assert_eq!(describe(&two).1, None);
        assert_eq!(describe(&[Edit::Name("x".into())]).0, "name");
    }

    fn library_load(select: Option<LibrarySlot>) -> Replace {
        Replace::Load { patch: Patch::init(), origin: "library (Init)".into(), select }
    }

    #[test]
    fn unedited_patch_is_replaced_at_once_with_its_selection() {
        let mut request = ReplaceRequest::default();
        let slot = LibrarySlot { bank_id: 1, program: 5 };
        assert_eq!(request.request(library_load(Some(slot)), false), Some(library_load(Some(slot))));
        assert!(request.pending().is_none());
    }

    #[test]
    fn cancelling_the_replace_drops_the_synth_selection() {
        let mut request = ReplaceRequest::default();
        let slot = LibrarySlot { bank_id: 1, program: 5 };
        assert_eq!(request.request(library_load(Some(slot)), true), None, "waits for the user");
        assert!(request.pending().is_some());
        assert_eq!(request.resolve(false), None, "nothing to perform, so nothing is selected on the synth");
        assert!(request.pending().is_none());
    }

    #[test]
    fn confirming_the_replace_performs_the_load_and_its_selection() {
        let mut request = ReplaceRequest::default();
        let slot = LibrarySlot { bank_id: 2, program: 127 };
        request.request(library_load(Some(slot)), true);
        match request.resolve(true) {
            Some(Replace::Load { select, .. }) => assert_eq!(select, Some(slot)),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(request.resolve(true), None, "performed once");
    }

    #[test]
    fn replace_warning_tells_the_truth_about_undo() {
        let fetch = Replace::Fetch.warning("Lead");
        assert!(fetch.contains("Undo brings them back"), "{fetch}");
        assert!(!fetch.contains("discards"), "{fetch}");
        for action in [library_load(None), Replace::Init, Replace::SendToEditBuffer { patch: Patch::init(), origin: String::new() }] {
            let text = action.warning("Lead");
            assert!(text.contains("discards them and the undo history"), "{text}");
        }
    }
}

//! Undo/redo history of byte-level changes to the edited patch (comfort E1).
//!
//! A step stores `(offset, before, after)` for every changed byte of the 256-byte dump, so undo
//! and redo restore exact bytes (SPEC §5.1) and never touch anything else. Applying a step goes
//! through [`Session::edit`], i.e. the same path as a normal edit, so the synth gets the same
//! SysEx parameter changes.
//!
//! Policy: loading or fetching a patch starts a new, empty history (the caller creates a fresh
//! [`History`]); the replaced edits are not undoable.

use virus_syx::{Page, Patch, PATCH_TOTAL_LEN};

use crate::session::Session;

/// Steps kept; older ones are dropped.
pub const MAX_STEPS: usize = 200;
/// Edits of the same control closer together than this (seconds) merge into one step.
pub const COALESCE_GAP: f64 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteChange {
    pub offset: usize,
    pub before: u8,
    pub after: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub label: String,
    pub changes: Vec<ByteChange>,
}

/// Identifies "the same control" for coalescing continuous drags.
pub type CoalesceKey = u32;

#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// Key and time of the newest undo step, while it can still absorb edits.
    open: Option<(CoalesceKey, f64)>,
}

/// Byte changes between two patches, ascending by offset.
pub fn changes_between(before: &Patch, after: &Patch) -> Vec<ByteChange> {
    (0..PATCH_TOTAL_LEN)
        .filter(|&i| before.as_bytes()[i] != after.as_bytes()[i])
        .map(|i| ByteChange { offset: i, before: before.as_bytes()[i], after: after.as_bytes()[i] })
        .collect()
}

/// Page and parameter number of a dump offset (A = 0..128, B = 128..256).
pub fn page_number(offset: usize) -> (Page, u8) {
    if offset < 128 {
        (Page::PageA, offset as u8)
    } else {
        (Page::PageB, (offset - 128) as u8)
    }
}

/// Writes `(offset, value)` pairs through [`Session::edit`] and returns the SysEx messages to send.
pub fn apply_values(session: &mut Session, values: impl IntoIterator<Item = (usize, u8)>) -> Vec<Vec<u8>> {
    values
        .into_iter()
        .filter_map(|(offset, value)| {
            let (page, number) = page_number(offset);
            session.edit(page, number, value)
        })
        .collect()
}

impl History {
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|s| s.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|s| s.label.as_str())
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.undo.len()
    }

    /// Records the change `before → after`. Does nothing if the bytes are equal. With a `key`,
    /// consecutive records of the same key within [`COALESCE_GAP`] (and not [`seal`](Self::seal)ed)
    /// merge into the newest step; a merge that ends back at the start removes the step.
    pub fn record(&mut self, before: &Patch, after: &Patch, label: &str, key: Option<CoalesceKey>, now: f64) {
        let changes = changes_between(before, after);
        if changes.is_empty() {
            return;
        }
        self.redo.clear();
        let mergeable = match (key, self.open) {
            (Some(k), Some((open_key, at))) => k == open_key && now - at <= COALESCE_GAP && !self.undo.is_empty(),
            _ => false,
        };
        if mergeable {
            let step = self.undo.last_mut().expect("checked non-empty");
            for change in changes {
                match step.changes.iter_mut().find(|c| c.offset == change.offset) {
                    Some(existing) => existing.after = change.after,
                    None => step.changes.push(change),
                }
            }
            step.changes.retain(|c| c.before != c.after);
            step.changes.sort_by_key(|c| c.offset);
            if step.changes.is_empty() {
                self.undo.pop();
                self.open = None;
                return;
            }
        } else {
            self.undo.push(Step { label: label.to_owned(), changes });
            if self.undo.len() > MAX_STEPS {
                self.undo.remove(0);
            }
        }
        self.open = key.map(|k| (k, now));
    }

    /// Ends coalescing: the next edit starts a new step (call on pointer release).
    pub fn seal(&mut self) {
        self.open = None;
    }

    /// Reverts the newest step through `session`; returns the messages to send.
    pub fn undo(&mut self, session: &mut Session) -> Vec<Vec<u8>> {
        self.open = None;
        let Some(step) = self.undo.pop() else { return Vec::new() };
        let messages = apply_values(session, step.changes.iter().map(|c| (c.offset, c.before)));
        self.redo.push(step);
        messages
    }

    pub fn redo(&mut self, session: &mut Session) -> Vec<Vec<u8>> {
        self.open = None;
        let Some(step) = self.redo.pop() else { return Vec::new() };
        let messages = apply_values(session, step.changes.iter().map(|c| (c.offset, c.after)));
        self.undo.push(step);
        messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use virus_syx::{read_sysex_file, parse_sysex, SysExMessage};

    fn factory(program: usize) -> Patch {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/factory/Bank_A.syx");
        let frames = read_sysex_file(path).expect("factory bank");
        match parse_sysex(&frames[program]).expect("single dump") {
            SysExMessage::SingleDump { data, .. } => Patch::from_bytes(&data),
            other => panic!("{other:?}"),
        }
    }

    fn session_with(patch: Patch) -> Session {
        let mut s = Session::default();
        s.load_patch(patch);
        s
    }

    /// Edits one parameter like the app does: before/after snapshot around `Session::edit`.
    fn edit(h: &mut History, s: &mut Session, offset: usize, value: u8, key: Option<u32>, now: f64) {
        let before = s.patch().clone();
        let (page, number) = page_number(offset);
        s.edit(page, number, value);
        h.record(&before, s.patch(), "edit", key, now);
    }

    #[test]
    fn undo_redo_restore_exact_bytes_and_send_param_changes() {
        let start = factory(3);
        let mut s = session_with(start.clone());
        let mut h = History::default();
        edit(&mut h, &mut s, 40, 7, None, 0.0);
        edit(&mut h, &mut s, 128 + 22, 99, None, 5.0); // an undocumented byte
        let edited = s.patch().clone();
        assert_ne!(edited, start);

        let messages = h.undo(&mut s);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0], s.param_message(Page::PageB, 22, start.get(Page::PageB, 22)));
        h.undo(&mut s);
        assert_eq!(s.patch(), &start);
        assert!(!h.can_undo() && h.can_redo());

        h.redo(&mut s);
        h.redo(&mut s);
        assert_eq!(s.patch(), &edited);
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut s = session_with(factory(0));
        let mut h = History::default();
        edit(&mut h, &mut s, 40, 1, None, 0.0);
        h.undo(&mut s);
        assert!(h.can_redo());
        edit(&mut h, &mut s, 41, 2, None, 1.0);
        assert!(!h.can_redo());
    }

    #[test]
    fn drag_coalesces_into_one_step_until_sealed_or_paused() {
        let start = factory(1);
        let mut s = session_with(start.clone());
        let mut h = History::default();
        for (i, v) in [10u8, 20, 30, 40].into_iter().enumerate() {
            edit(&mut h, &mut s, 40, v, Some(40), i as f64 * 0.1);
        }
        assert_eq!(h.len(), 1);
        h.seal();
        edit(&mut h, &mut s, 40, 50, Some(40), 0.5);
        assert_eq!(h.len(), 2);
        edit(&mut h, &mut s, 41, 50, Some(41), 0.6); // other control
        assert_eq!(h.len(), 3);
        edit(&mut h, &mut s, 41, 60, Some(41), 5.0); // too late
        assert_eq!(h.len(), 4);

        h.undo(&mut s);
        h.undo(&mut s);
        assert_eq!(s.patch().get(Page::PageA, 40), 50);
        h.undo(&mut s);
        assert_eq!(s.patch().get(Page::PageA, 40), 40, "the drag was one step: 4 edits → first value kept as before");
        h.undo(&mut s);
        assert_eq!(s.patch(), &start);
    }

    #[test]
    fn drag_back_to_start_leaves_no_step() {
        let start = factory(2);
        let original = start.get(Page::PageA, 40);
        let mut s = session_with(start);
        let mut h = History::default();
        edit(&mut h, &mut s, 40, original.wrapping_add(5) & 0x7F, Some(40), 0.0);
        edit(&mut h, &mut s, 40, original, Some(40), 0.1);
        assert!(!h.can_undo());
    }

    #[test]
    fn a0_is_never_recorded_or_changed() {
        let start = factory(5);
        let mut s = session_with(start.clone());
        let mut h = History::default();
        edit(&mut h, &mut s, 0, 99, None, 0.0);
        assert!(!h.can_undo());
        assert_eq!(s.patch().as_bytes()[0], start.as_bytes()[0]);
    }

    #[test]
    fn history_is_bounded() {
        let mut s = session_with(factory(0));
        let mut h = History::default();
        for i in 0..(MAX_STEPS + 20) {
            edit(&mut h, &mut s, 40, (i % 100) as u8 + 1, None, i as f64);
        }
        assert!(h.len() <= MAX_STEPS);
    }
}

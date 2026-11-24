//! Compare (A/B with the original) and the patch diff list (comforts E2, E3).

use virus_syx::{format_value, single_param, Patch, PATCH_TOTAL_LEN};

use crate::history::{apply_step, changes_between, page_number};
use crate::session::Session;

/// "A40" / "B112" style label of a dump offset.
pub fn offset_label(offset: usize) -> String {
    let (page, number) = page_number(offset);
    let letter = if page == virus_syx::Page::PageA { 'A' } else { 'B' };
    format!("{letter}{number}")
}

/// Compare state: while active the session holds the original and the edited patch is parked here.
#[derive(Debug, Default)]
pub struct Compare {
    edited: Option<Patch>,
}

impl Compare {
    pub fn is_active(&self) -> bool {
        self.edited.is_some()
    }

    /// The parked edited patch while comparing.
    pub fn edited(&self) -> Option<&Patch> {
        self.edited.as_ref()
    }

    /// Parks the edited patch and makes the session (and synth) play `original`.
    /// Returns the SysEx messages to send (see [`apply_step`]). No-op if already active.
    pub fn enter(&mut self, session: &mut Session, original: &Patch) -> Vec<Vec<u8>> {
        if self.is_active() {
            return Vec::new();
        }
        self.edited = Some(session.patch().clone());
        let target = *original.as_bytes();
        let values: Vec<_> = changes_between(session.patch(), original).iter().map(|c| (c.offset, target[c.offset])).collect();
        apply_step(session, values)
    }

    /// Restores the parked edited patch. Returns the SysEx messages to send (see [`apply_step`]).
    pub fn leave(&mut self, session: &mut Session) -> Vec<Vec<u8>> {
        let Some(edited) = self.edited.take() else { return Vec::new() };
        let target = *edited.as_bytes();
        let values: Vec<_> = changes_between(session.patch(), &edited).iter().map(|c| (c.offset, target[c.offset])).collect();
        apply_step(session, values)
    }
    /// Forgets the parked patch without touching the session (the patch is being replaced).
    pub fn discard(&mut self) {
        self.edited = None;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffEntry {
    pub offset: usize,
    /// "Cutoff (A40)" for documented parameters, "A92 (undocumented)" otherwise.
    pub label: String,
    pub section: Option<&'static str>,
    pub before: String,
    pub after: String,
}

/// Every byte that differs between `a` and `b`, in dump order. Documented parameters use the
/// parameter table's name and value text; other bytes show raw values.
pub fn diff(a: &Patch, b: &Patch) -> Vec<DiffEntry> {
    (0..PATCH_TOTAL_LEN)
        .filter(|&i| a.as_bytes()[i] != b.as_bytes()[i])
        .map(|offset| {
            let (before, after) = (a.as_bytes()[offset], b.as_bytes()[offset]);
            let (page, number) = page_number(offset);
            match single_param(page, number) {
                Some(def) => DiffEntry {
                    offset,
                    label: format!("{} ({})", def.name, offset_label(offset)),
                    section: Some(def.section),
                    before: format_value(def, before),
                    after: format_value(def, after),
                },
                None => DiffEntry {
                    offset,
                    label: format!("{} (undocumented)", offset_label(offset)),
                    section: None,
                    before: before.to_string(),
                    after: after.to_string(),
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use virus_syx::{is_sound_param, parse_sysex, read_sysex_file, Page, SysExMessage};

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

    #[test]
    fn compare_round_trip_restores_exact_bytes() {
        let banks = bank_a();
        for (original, edited) in banks.iter().zip(banks.iter().skip(1)) {
            let mut session = Session::default();
            session.load_patch(edited.clone());
            let mut compare = Compare::default();

            let to_original = compare.enter(&mut session, original);
            assert!(compare.is_active());
            // Everything except the synth-owned A0 now equals the original.
            let mut expected = *original.as_bytes();
            expected[0] = edited.as_bytes()[0];
            assert_eq!(session.patch().as_bytes(), &expected);
            assert_sent_safely(&to_original);

            let to_edited = compare.leave(&mut session);
            assert!(!compare.is_active());
            assert_eq!(session.patch(), edited);
            assert_sent_safely(&to_edited);
        }
    }

    /// Only sound parameters go out as parameter changes; anything else is one edit-buffer dump.
    fn assert_sent_safely(messages: &[Vec<u8>]) {
        for message in messages {
            match parse_sysex(message).expect("valid SysEx") {
                SysExMessage::ParamChange { page, param, .. } => {
                    assert!(is_sound_param(page, param), "parameter change for {page:?} {param}");
                }
                SysExMessage::SingleDump { bank: 0, .. } => assert_eq!(messages.len(), 1, "a dump is sent alone"),
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn enter_twice_keeps_the_first_edited_patch() {
        let banks = bank_a();
        let mut session = Session::default();
        session.load_patch(banks[2].clone());
        let mut compare = Compare::default();
        compare.enter(&mut session, &banks[0]);
        assert!(compare.enter(&mut session, &banks[0]).is_empty());
        compare.leave(&mut session);
        assert_eq!(session.patch(), &banks[2]);
    }

    #[test]
    fn diff_lists_named_parameters_and_raw_offsets() {
        let a = bank_a()[10].clone();
        let mut b = a.clone();
        b.set(Page::PageA, 40, a.get(Page::PageA, 40) ^ 0x10);
        // B22 is undocumented but holds data in factory patches.
        assert!(single_param(Page::PageB, 22).is_none());
        b.set(Page::PageB, 22, a.get(Page::PageB, 22) ^ 0x01);

        let d = diff(&a, &b);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].offset, 40);
        assert!(d[0].label.starts_with("Cutoff") && d[0].label.ends_with("(A40)"), "{}", d[0].label);
        assert_eq!(d[0].before, format_value(single_param(Page::PageA, 40).unwrap(), a.get(Page::PageA, 40)));
        assert_eq!(d[1].offset, 128 + 22);
        assert_eq!(d[1].label, "B22 (undocumented)");
        assert_eq!(d[1].section, None);
        assert_eq!(d[1].after, b.get(Page::PageB, 22).to_string());
        assert!(diff(&a, &a).is_empty());
    }
}

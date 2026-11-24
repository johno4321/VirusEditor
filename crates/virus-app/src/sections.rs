//! Copy / paste / init of whole parameter sections (comfort E4).
//!
//! A section is the set of documented sound parameters sharing `ParamDef::section`. Only those
//! bytes are ever read or written: never A0 (Sound Version), the name, the categories,
//! performance controllers or undocumented bytes (SPEC §5.1–5.4).

use virus_syx::{Patch, SINGLE_PARAMS};

/// Sections that hold identity or synth-owned data rather than sound settings.
const EXCLUDED: &[&str] = &["Internal", "Performance", "Name", "Category"];

const A0_OFFSET: usize = 0;

/// Copyable section names in parameter-table order.
pub fn section_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = Vec::new();
    for def in SINGLE_PARAMS {
        if is_copyable(def.section, def.is_sound(), def.dump_offset()) && !names.contains(&def.section) {
            names.push(def.section);
        }
    }
    names
}

fn is_copyable(section: &str, sound: bool, offset: Option<usize>) -> bool {
    sound && !EXCLUDED.contains(&section) && offset.is_some_and(|o| o != A0_OFFSET)
}

/// Dump offsets of a section's documented parameters.
pub fn section_offsets(section: &str) -> Vec<usize> {
    SINGLE_PARAMS
        .iter()
        .filter(|d| d.section == section && is_copyable(d.section, d.is_sound(), d.dump_offset()))
        .filter_map(|d| d.dump_offset())
        .collect()
}

/// The values of one section, taken from a patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionClip {
    pub section: String,
    pub values: Vec<(usize, u8)>,
}

/// Copies `section` from `patch`; `None` for an unknown or non-copyable section.
pub fn copy(patch: &Patch, section: &str) -> Option<SectionClip> {
    let values: Vec<(usize, u8)> = section_offsets(section).into_iter().map(|o| (o, patch.as_bytes()[o])).collect();
    (!values.is_empty()).then(|| SectionClip { section: section.to_owned(), values })
}

/// The section as in [`Patch::init`].
pub fn init_clip(section: &str) -> Option<SectionClip> {
    copy(&Patch::init(), section)
}

/// The `(offset, value)` pairs that would change `current` when `clip` is pasted.
pub fn changes(current: &Patch, clip: &SectionClip) -> Vec<(usize, u8)> {
    clip.values.iter().copied().filter(|&(o, v)| current.as_bytes()[o] != v).collect()
}

/// Number of bytes a paste would change.
pub fn differing(current: &Patch, clip: &SectionClip) -> usize {
    changes(current, clip).len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::changes_between;
    use virus_syx::{parse_sysex, read_sysex_file, SysExMessage};

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

    fn paste(target: &Patch, clip: &SectionClip) -> Patch {
        let mut out = target.clone();
        for (offset, value) in changes(target, clip) {
            let (page, number) = crate::history::page_number(offset);
            out.set(page, number, value);
        }
        out
    }

    #[test]
    fn expected_sections_exist_and_identity_sections_do_not() {
        let names = section_names();
        for wanted in ["Filters", "LFO 1", "Assign"] {
            assert!(names.contains(&wanted), "{wanted} missing from {names:?}");
        }
        for banned in EXCLUDED {
            assert!(!names.contains(banned));
        }
    }

    #[test]
    fn paste_touches_only_the_sections_bytes() {
        let banks = bank_a();
        for section in section_names() {
            let allowed = section_offsets(section);
            for (src, dst) in banks.iter().zip(banks.iter().skip(7)).take(40) {
                let clip = copy(src, section).unwrap();
                let pasted = paste(dst, &clip);
                for i in 0..256 {
                    if !allowed.contains(&i) {
                        assert_eq!(pasted.as_bytes()[i], dst.as_bytes()[i], "{section} touched offset {i}");
                    } else {
                        assert_eq!(pasted.as_bytes()[i], src.as_bytes()[i]);
                    }
                }
                assert_eq!(pasted.as_bytes()[0], dst.as_bytes()[0], "A0");
                assert_eq!(pasted.name(), dst.name());
                assert_eq!(&pasted.as_bytes()[240..250], &dst.as_bytes()[240..250]);
            }
        }
    }

    #[test]
    fn pasting_a_section_onto_itself_changes_nothing() {
        let p = &bank_a()[20];
        for section in section_names() {
            assert!(changes(p, &copy(p, section).unwrap()).is_empty());
        }
    }

    #[test]
    fn init_section_resets_to_init_values_only() {
        let target = &bank_a()[33];
        let clip = init_clip("Filters").unwrap();
        let reset = paste(target, &clip);
        let init = Patch::init();
        let allowed = section_offsets("Filters");
        for i in 0..256 {
            let expected = if allowed.contains(&i) { init.as_bytes()[i] } else { target.as_bytes()[i] };
            assert_eq!(reset.as_bytes()[i], expected, "offset {i}");
        }
        assert_eq!(changes_between(target, &reset).iter().filter(|c| !allowed.contains(&c.offset)).count(), 0);
    }

    #[test]
    fn unknown_or_excluded_sections_cannot_be_copied() {
        let p = Patch::init();
        assert!(copy(&p, "Name").is_none());
        assert!(copy(&p, "Internal").is_none());
        assert!(copy(&p, "Nope").is_none());
    }
}

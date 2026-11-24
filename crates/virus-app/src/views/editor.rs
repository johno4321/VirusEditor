//! The sound editor, generated from `SINGLE_PARAMS` grouped by section (P7).

use egui::{RichText, ScrollArea, TextEdit, Ui};
use virus_syx::{single_param, Page, ParamClass, ParamDef, Patch, SINGLE_PARAMS};

use super::controls::param_control;
use super::{Edit, DIM};
use crate::session::sanitize_name;
use crate::widgets::{EnvelopeCurve, EnvelopeParams, FilterCurve, FilterParams};

/// Sections drawn elsewhere: the name/category header, the modulation tab, or not editable.
const SECTIONS_ELSEWHERE: &[&str] = &["Internal", "Performance", "Name", "Category", "Assign", "Definable"];

const NAME_OFFSET: usize = 128 + 112;
const NAME_LEN: usize = 10;

/// Editable sections in table order; Page B rows join the Page A section of the same name.
pub fn editor_sections() -> Vec<(&'static str, Vec<&'static ParamDef>)> {
    let mut sections: Vec<(&'static str, Vec<&'static ParamDef>)> = Vec::new();
    for def in SINGLE_PARAMS {
        if def.class != ParamClass::Sound || SECTIONS_ELSEWHERE.contains(&def.section) {
            continue;
        }
        match sections.iter_mut().find(|(name, _)| *name == def.section) {
            Some((_, defs)) => defs.push(def),
            None => sections.push((def.section, vec![def])),
        }
    }
    sections
}

/// Parameters shown in the editor header (name characters and categories).
#[cfg(test)]
pub fn header_params() -> Vec<(Page, u8)> {
    (112..=121).chain(123..=124).map(|n| (Page::PageB, n)).collect()
}

#[derive(Default)]
pub struct EditorView {
    /// The name being typed, kept separately so trailing spaces survive while typing.
    name_buffer: String,
    name_bytes: [u8; NAME_LEN],
}

impl EditorView {
    pub fn show(&mut self, ui: &mut Ui, patch: &Patch, edits: &mut Vec<Edit>) {
        self.header(ui, patch, edits);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add(FilterCurve::new(FilterParams::from_patch(patch)));
            ui.add(EnvelopeCurve::new("Filter Envelope", EnvelopeParams::filter(patch)));
            ui.add(EnvelopeCurve::new("Amp Envelope", EnvelopeParams::amp(patch)));
        });
        ui.add_space(4.0);

        let sections = editor_sections();
        let total: usize = sections.iter().map(|(_, defs)| defs.len()).sum();
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.columns(2, |columns| {
                let mut drawn = 0;
                for (name, defs) in &sections {
                    // Fill the left column with roughly half of the controls.
                    let column = usize::from(drawn * 2 >= total);
                    drawn += defs.len();
                    section(&mut columns[column], name, defs, patch, edits);
                }
            });
        });
    }

    fn header(&mut self, ui: &mut Ui, patch: &Patch, edits: &mut Vec<Edit>) {
        let mut bytes = [0u8; NAME_LEN];
        bytes.copy_from_slice(&patch.as_bytes()[NAME_OFFSET..NAME_OFFSET + NAME_LEN]);
        if bytes != self.name_bytes {
            self.name_bytes = bytes;
            self.name_buffer = patch.name();
        }

        ui.horizontal(|ui| {
            ui.label(RichText::new("Name").strong());
            let response = ui.add(
                TextEdit::singleline(&mut self.name_buffer)
                    .char_limit(NAME_LEN)
                    .desired_width(110.0)
                    .font(egui::TextStyle::Monospace),
            );
            if response.changed() {
                self.name_buffer = sanitize_name(&self.name_buffer);
                // Remember the bytes this buffer writes, so the next frame doesn't reset it
                // (e.g. a trailing space typed before the next letter).
                let mut preview = patch.clone();
                preview.set_name(&self.name_buffer);
                self.name_bytes.copy_from_slice(&preview.as_bytes()[NAME_OFFSET..NAME_OFFSET + NAME_LEN]);
                if self.name_bytes != bytes {
                    edits.push(Edit::Name(self.name_buffer.clone()));
                }
            }
            response.on_hover_text("10 characters, printable ASCII only (B112–B121)");

            ui.separator();
            for number in [123, 124] {
                if let Some(def) = single_param(Page::PageB, number) {
                    param_control(ui, patch, def, edits);
                }
            }
            ui.separator();
            ui.label(RichText::new(format!("Sound Version {}", patch.sound_version())).color(DIM))
                .on_hover_text("A0 is owned by the synth and never changed by the editor (SPEC §5.2).");
        });
    }
}

fn section(ui: &mut Ui, name: &str, defs: &[&'static ParamDef], patch: &Patch, edits: &mut Vec<Edit>) {
    crate::theme::section(ui, name, None, |ui| {
        ui.horizontal_wrapped(|ui| {
            for def in defs {
                param_control(ui, patch, def, edits);
            }
        });
    });
    ui.add_space(4.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::modulation;

    #[test]
    fn every_sound_parameter_has_exactly_one_control() {
        let mut shown: Vec<(Page, u8)> = editor_sections()
            .iter()
            .flat_map(|(_, defs)| defs.iter().map(|d| (d.page, d.number)))
            .chain(modulation::shown_params())
            .chain(header_params())
            .collect();
        shown.sort();
        let before_dedup = shown.len();
        shown.dedup();
        assert_eq!(shown.len(), before_dedup, "a parameter is shown twice");

        let mut sound: Vec<(Page, u8)> = SINGLE_PARAMS
            .iter()
            .filter(|d| d.class == ParamClass::Sound)
            .map(|d| (d.page, d.number))
            .collect();
        sound.sort();
        assert_eq!(shown, sound);
    }

    #[test]
    fn performance_and_internal_params_are_not_in_the_editor() {
        for (_, defs) in editor_sections() {
            assert!(defs.iter().all(|d| d.class == ParamClass::Sound), "{:?}", defs);
        }
    }
}

//! UI for the editor comforts: undo/redo, Compare, "edited" badge, patch diff and section
//! copy/paste. Reports what the user asked for; the app applies it through [`Comforts`].

use egui::{ComboBox, Context, RichText, ScrollArea, Ui, Window};
use virus_db::PatchDatabase;
use virus_syx::Patch;

use super::{accent, DIM, WARN};
use crate::comforts::Comforts;
use crate::compare::diff;
use crate::sections::{self, SectionClip};
use crate::session::Session;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComfortAction {
    Undo,
    Redo,
    ToggleCompare,
    /// Send the edited patch to the synth's edit buffer (RV-M5).
    SendNow,
    /// Paste or init a section (`verb` names the undo step).
    Section {
        clip: SectionClip,
        verb: &'static str,
    },
}

/// A library slot picked by bank and slot number.
#[derive(Default)]
struct LibraryPick {
    bank_id: Option<i64>,
    program: u8,
}

impl LibraryPick {
    /// Draws the picker; returns the patch in the chosen slot and a label, if the slot is filled.
    fn show(&mut self, ui: &mut Ui, db: &PatchDatabase, salt: &str) -> Option<(Patch, String)> {
        let banks = db.get_banks().unwrap_or_default();
        if self.bank_id.is_none() {
            self.bank_id = banks.first().map(|b| b.id);
        }
        let current = banks
            .iter()
            .find(|b| Some(b.id) == self.bank_id)
            .map_or("—".to_owned(), |b| b.name.clone());
        ui.horizontal(|ui| {
            ComboBox::from_id_source((salt, "bank"))
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for bank in &banks {
                        ui.selectable_value(&mut self.bank_id, Some(bank.id), &bank.name);
                    }
                });
            ui.label("slot");
            ui.add(egui::DragValue::new(&mut self.program).clamp_range(0..=127));
        });
        let record = db
            .get_patch_by_slot(self.bank_id?, self.program)
            .ok()
            .flatten();
        match record {
            Some(r) => {
                ui.label(RichText::new(&r.name).monospace());
                Some((
                    Patch::from_bytes(&r.data),
                    format!("library slot {}", self.program),
                ))
            }
            None => {
                ui.label(RichText::new("empty slot").color(DIM));
                None
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Source {
    #[default]
    Edited,
    Library,
}

#[derive(Default)]
pub struct ComfortsView {
    diff_open: bool,
    diff_library: bool,
    diff_pick: LibraryPick,
    sections_open: bool,
    section: Option<&'static str>,
    source: Source,
    source_pick: LibraryPick,
    clip: Option<(SectionClip, String)>,
}

impl ComfortsView {
    /// The toolbar row: undo/redo, Compare, edited/compare badges, and the window toggles.
    pub fn bar(
        &mut self,
        ui: &mut Ui,
        comforts: &Comforts,
        session: &Session,
    ) -> Vec<ComfortAction> {
        let mut actions = Vec::new();
        let history = comforts.history();
        let comparing = comforts.comparing();
        ui.horizontal(|ui| {
            let undo = ui.add_enabled(history.can_undo() && !comparing, egui::Button::new("↶ Undo"));
            if undo.on_hover_text(match history.undo_label() {
                Some(label) => format!("Undo {label} (Ctrl+Z)"),
                None => "Nothing to undo".to_owned(),
            })
            .clicked()
            {
                actions.push(ComfortAction::Undo);
            }
            let redo = ui.add_enabled(history.can_redo() && !comparing, egui::Button::new("↷ Redo"));
            if redo.on_hover_text(match history.redo_label() {
                Some(label) => format!("Redo {label} (Ctrl+Y / Ctrl+Shift+Z)"),
                None => "Nothing to redo".to_owned(),
            })
            .clicked()
            {
                actions.push(ComfortAction::Redo);
            }
            ui.separator();
            let compare = egui::SelectableLabel::new(comparing, "⇄ Compare");
            if ui
                .add(compare)
                .on_hover_text("Toggle between the edited patch and the original it was loaded from; the synth follows")
                .clicked()
            {
                actions.push(ComfortAction::ToggleCompare);
            }
            if comparing {
                ui.label(RichText::new("COMPARE — playing the ORIGINAL; editing is disabled").strong().color(WARN));
            } else if comforts.is_edited(session) {
                ui.label(RichText::new("● edited").strong().color(accent()))
                    .on_hover_text("Differs from the patch as loaded; not stored");
            } else {
                ui.label(RichText::new("unchanged").color(DIM));
            }
            if comforts.on_synth() {
                ui.label(RichText::new("✔ on synth").color(DIM)).on_hover_text("The synth's edit buffer holds this sound");
            } else {
                ui.label(RichText::new("⚠ not on synth").strong().color(WARN))
                    .on_hover_text("Edits go to the synth's edit buffer, which holds a different sound. Store saves what you see here, not what you hear.");
                if ui.add_enabled(!comparing, egui::Button::new("Send now")).clicked() {
                    actions.push(ComfortAction::SendNow);
                }
            }
            ui.separator();
            ui.toggle_value(&mut self.diff_open, "Diff…");
            ui.toggle_value(&mut self.sections_open, "Sections…");
        });
        actions
    }

    pub fn windows(
        &mut self,
        ctx: &Context,
        comforts: &Comforts,
        session: &Session,
        db: &PatchDatabase,
    ) -> Vec<ComfortAction> {
        let mut actions = Vec::new();
        self.diff_window(ctx, comforts, session, db);
        self.sections_window(ctx, comforts, session, db, &mut actions);
        actions
    }

    fn diff_window(
        &mut self,
        ctx: &Context,
        comforts: &Comforts,
        session: &Session,
        db: &PatchDatabase,
    ) {
        let mut open = self.diff_open;
        Window::new("Patch diff")
            .open(&mut open)
            .default_width(420.0)
            .show(ctx, |ui| {
                let edited = comforts.edited(session);
                ui.horizontal(|ui| {
                    ui.label("Compare the edited patch with");
                    ui.selectable_value(&mut self.diff_library, false, "its original");
                    ui.selectable_value(&mut self.diff_library, true, "a library patch");
                });
                let other = if self.diff_library {
                    self.diff_pick.show(ui, db, "diff-pick").map(|(p, _)| p)
                } else {
                    Some(comforts.original().clone())
                };
                let Some(other) = other else { return };
                let entries = diff(&other, edited);
                ui.separator();
                if entries.is_empty() {
                    ui.label("Identical (all 256 bytes).");
                    return;
                }
                ui.label(format!(
                    "{} byte(s) differ — 'other' → edited",
                    entries.len()
                ));
                ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    egui::Grid::new("diff-grid").striped(true).show(ui, |ui| {
                        for entry in &entries {
                            let label = RichText::new(&entry.label);
                            ui.label(if entry.section.is_some() {
                                label
                            } else {
                                label.color(DIM)
                            });
                            ui.label(entry.section.unwrap_or(""));
                            ui.monospace(&entry.before);
                            ui.label("→");
                            ui.monospace(&entry.after);
                            ui.end_row();
                        }
                    });
                });
            });
        self.diff_open = open;
    }

    fn sections_window(
        &mut self,
        ctx: &Context,
        comforts: &Comforts,
        session: &Session,
        db: &PatchDatabase,
        actions: &mut Vec<ComfortAction>,
    ) {
        let mut open = self.sections_open;
        Window::new("Copy / paste sections").open(&mut open).default_width(360.0).show(ctx, |ui| {
            let names = sections::section_names();
            if self.section.is_none() {
                self.section = names.first().copied();
            }
            ui.horizontal(|ui| {
                ui.label("Section");
                ComboBox::from_id_source("section-pick").selected_text(self.section.unwrap_or("—")).show_ui(ui, |ui| {
                    for name in &names {
                        ui.selectable_value(&mut self.section, Some(*name), *name);
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label("Copy from");
                ui.selectable_value(&mut self.source, Source::Edited, "the edited patch");
                ui.selectable_value(&mut self.source, Source::Library, "a library patch");
            });
            let source = match self.source {
                Source::Edited => Some((comforts.edited(session).clone(), "the edited patch".to_owned())),
                Source::Library => self.source_pick.show(ui, db, "section-source"),
            };
            let section = self.section;
            ui.horizontal(|ui| {
                let can_copy = source.is_some() && section.is_some();
                if ui.add_enabled(can_copy, egui::Button::new("Copy")).clicked() {
                    if let (Some((patch, from)), Some(name)) = (&source, section) {
                        self.clip = sections::copy(patch, name).map(|c| (c, format!("{name} from {from} ('{}')", patch.name())));
                    }
                }
            });
            ui.separator();
            let comparing = comforts.comparing();
            match &self.clip {
                Some((clip, text)) => {
                    ui.label(format!("Clipboard: {text}"));
                    let differing = sections::differing(comforts.edited(session), clip);
                    let paste = egui::Button::new(format!("Paste into the edited patch ({differing} byte(s) change)"));
                    if ui.add_enabled(!comparing && differing > 0, paste).clicked() {
                        actions.push(ComfortAction::Section { clip: clip.clone(), verb: "Paste" });
                    }
                }
                None => {
                    ui.label(RichText::new("Clipboard empty").color(DIM));
                }
            }
            if let Some(init) = section.and_then(sections::init_clip) {
                let differing = sections::differing(comforts.edited(session), &init);
                if ui
                    .add_enabled(!comparing && differing > 0, egui::Button::new(format!("Init section ({differing} byte(s) change)")))
                    .clicked()
                {
                    actions.push(ComfortAction::Section { clip: init, verb: "Init" });
                }
            }
            ui.label(RichText::new("Never changes the name, categories, Sound Version (A0) or undocumented bytes.").size(10.0).color(DIM));
        });
        self.sections_open = open;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs frames headlessly to prove the UI draws without panicking in every state.
    #[test]
    fn draws_without_panicking() {
        let ctx = Context::default();
        let db = PatchDatabase::open_in_memory().expect("db");
        db.create_bank("Test").expect("bank");
        let mut session = Session::default();
        let mut comforts = Comforts::new(Patch::init());
        let mut view = ComfortsView {
            diff_open: true,
            sections_open: true,
            ..Default::default()
        };
        for frame in 0..4 {
            match frame {
                1 => view.diff_library = true,
                2 => view.source = Source::Library,
                3 => {
                    session.load_patch(Patch::init());
                    session.edit(virus_syx::Page::PageA, 40, 3);
                    comforts.toggle_compare(&mut session);
                }
                _ => {}
            }
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    view.bar(ui, &comforts, &session);
                });
                view.windows(ctx, &comforts, &session, &db);
            });
        }
    }
}

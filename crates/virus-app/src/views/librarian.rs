//! Patch librarian: banks, the 128 slots of a bank, search, favourites and tags, file
//! import/export, bank transfer to/from the synth, and storing the edited patch.

use egui::{Align, Button, ComboBox, Context, Layout, RichText, ScrollArea, TextEdit, Ui, Window};
use virus_db::{ImportTarget, IncomingDump, PatchDatabase, PatchSummary, SoundCategory};
use virus_syx::{Patch, SingleBank};

use super::{Notice, ACCENT, DANGER, DIM, WARN};
use crate::library::{
    export_bank_file, first_free_slot, import_as_new_bank, import_dumps, import_file, slot_label, store_patch,
    ImportOutcome, LibraryCache, LibraryQuery,
};

/// Pause between dumps when sending a bank, so the synth can write each program.
pub const BANK_SEND_GAP_MS: u64 = 150;

/// What the librarian asks the app to do.
#[derive(Debug, Clone)]
pub enum LibAction {
    Load { patch: Patch, origin: String },
    SendToEditBuffer { patch: Patch, origin: String },
    RequestBank(SingleBank),
    SendBank { bank_id: i64 },
    CancelBatch,
    Notice(Notice),
}

struct StoreDialog {
    /// `None` = the editor's patch.
    patch: Option<Patch>,
    program: u8,
    error: Option<String>,
}

enum RowClick {
    Select { program: Option<u8>, id: Option<i64> },
    Load(i64),
    Send(i64),
    Favourite(i64, bool),
}

#[derive(Default)]
pub struct LibrarianView {
    query: LibraryQuery,
    cache: LibraryCache,
    selected_program: Option<u8>,
    selected_id: Option<i64>,
    tags_buffer: String,
    tags_for: Option<i64>,
    import_into_selected: bool,
    export_as: Option<u8>,
    last_import: Option<ImportOutcome>,
    /// Edit-buffer dumps from the last import or synth transfer; never put in a slot automatically.
    loose: Vec<IncomingDump>,
    store: Option<StoreDialog>,
    confirm_send_bank: Option<i64>,
}

impl LibrarianView {
    fn refresh(&mut self, db: &PatchDatabase) {
        self.cache.refresh(db, &self.query);
        if self.query.bank_id.is_none() {
            if let Some(first) = self.cache.banks.first() {
                self.query.bank_id = Some(first.id);
                self.cache.refresh(db, &self.query);
            }
        }
    }

    fn select_bank(&mut self, bank_id: i64) {
        if self.query.bank_id != Some(bank_id) {
            self.query.bank_id = Some(bank_id);
            self.selected_program = None;
        }
    }

    fn selected_summary(&self) -> Option<&PatchSummary> {
        let id = self.selected_id?;
        self.cache.slots.iter().flatten().chain(&self.cache.results).find(|s| s.id == id)
    }

    /// Opens the "store" dialog for the editor's patch (`None`) or another patch, preset to the
    /// selected slot or the first free slot of the selected bank.
    pub fn open_store_dialog(&mut self, db: &PatchDatabase, patch: Option<Patch>) {
        self.refresh(db);
        let program = self.selected_program.or_else(|| first_free_slot(&self.cache.slots)).unwrap_or(0);
        self.store = Some(StoreDialog { patch, program, error: None });
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        db: &PatchDatabase,
        received: &mut Vec<IncomingDump>,
        actions: &mut Vec<LibAction>,
    ) {
        self.refresh(db);
        egui::SidePanel::left("librarian-banks").resizable(false).exact_width(250.0).show_inside(ui, |ui| {
            ScrollArea::vertical().show(ui, |ui| self.left_panel(ui, db, received, actions));
        });
        egui::SidePanel::right("librarian-details").resizable(false).exact_width(250.0).show_inside(ui, |ui| {
            self.details(ui, db, actions);
        });
        egui::CentralPanel::default().show_inside(ui, |ui| self.patch_list(ui, db, actions));
    }

    fn left_panel(
        &mut self,
        ui: &mut Ui,
        db: &PatchDatabase,
        received: &mut Vec<IncomingDump>,
        actions: &mut Vec<LibAction>,
    ) {
        ui.label(RichText::new("Banks").strong().color(ACCENT));
        let mut picked = None;
        for bank in &self.cache.banks {
            let mut text = bank.name.clone();
            if bank.is_rom {
                text.push_str("  (ROM)");
            }
            let response = ui.selectable_label(self.query.bank_id == Some(bank.id), text);
            let response = if bank.is_rom {
                response.on_hover_text("Flash ROM on the synth: it can be requested but not overwritten by a bank dump.")
            } else {
                response
            };
            if response.clicked() {
                picked = Some(bank.id);
            }
        }
        if let Some(id) = picked {
            self.select_bank(id);
        }
        if let Some(error) = &self.cache.error {
            ui.colored_label(DANGER, format!("Database error: {error}"));
        }

        ui.separator();
        ui.label(RichText::new("Files").strong().color(ACCENT));
        if ui.button("📂 Import .syx / .mid / .bin…").clicked() {
            self.import_from_file(db, actions);
        }
        ui.checkbox(&mut self.import_into_selected, "Import into the selected bank")
            .on_hover_text("Off: dumps go to the bank they were saved from (bb = A–D). On: all go into the selected bank.");
        ui.horizontal(|ui| {
            if ui.button("💾 Export bank .syx…").clicked() {
                self.export_to_file(db, actions);
            }
            let as_text = |bb: Option<u8>| bb.and_then(SingleBank::from_raw).map_or("own".to_owned(), |b| b.letter().to_string());
            ComboBox::from_id_source("export-as").width(48.0).selected_text(as_text(self.export_as)).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.export_as, None, "own");
                for bank in SingleBank::ALL {
                    ui.selectable_value(&mut self.export_as, Some(bank.to_raw()), bank.letter().to_string());
                }
            })
            .response
            .on_hover_text("Bank byte written into the file: the bank's own letter, or A–D (needed for user banks).");
        });

        ui.separator();
        ui.label(RichText::new("Synth").strong().color(ACCENT));
        ui.horizontal(|ui| {
            ui.label("Request bank");
            for bank in SingleBank::ALL {
                if ui.small_button(bank.letter().to_string()).on_hover_text("Single bank request (32 bb)").clicked() {
                    actions.push(LibAction::RequestBank(bank));
                }
            }
        });
        let selected_bank = self.query.bank_id.and_then(|id| self.cache.bank(id));
        let ram_letter = selected_bank
            .filter(|b| !b.is_rom)
            .and_then(|b| b.bank_letter.as_deref())
            .filter(|l| matches!(*l, "A" | "B"));
        let send = ui.add_enabled(ram_letter.is_some(), Button::new("📤 Send bank to synth…"));
        let send = if ram_letter.is_none() {
            send.on_disabled_hover_text("Only library banks A and B can be sent: C and D are ROM on the synth.")
        } else {
            send
        };
        if send.clicked() {
            self.confirm_send_bank = self.query.bank_id;
        }
        if ui.button("⏹ Cancel sending").clicked() {
            actions.push(LibAction::CancelBatch);
        }

        if !received.is_empty() {
            ui.separator();
            self.received_panel(ui, db, received, actions);
        }
        if let Some(outcome) = &self.last_import {
            ui.separator();
            ui.label(RichText::new("Last import").strong().color(ACCENT));
            let colour = if outcome.has_warnings() { WARN } else { DIM };
            ui.label(RichText::new(outcome.summary()).color(colour).size(11.0));
        }
        if !self.loose.is_empty() {
            ui.separator();
            self.loose_panel(ui, db, actions);
        }
    }

    fn received_panel(
        &mut self,
        ui: &mut Ui,
        db: &PatchDatabase,
        received: &mut Vec<IncomingDump>,
        actions: &mut Vec<LibAction>,
    ) {
        ui.label(RichText::new("Received from synth").strong().color(ACCENT));
        let mut banks: Vec<(u8, usize)> = Vec::new();
        for dump in received.iter() {
            match banks.iter_mut().find(|(b, _)| *b == dump.bank) {
                Some((_, n)) => *n += 1,
                None => banks.push((dump.bank, 1)),
            }
        }
        let describe = |bank: u8| match SingleBank::from_raw(bank) {
            Some(b) => format!("Bank {}", b.letter()),
            None if bank == 0 => "edit buffers".to_owned(),
            None => format!("bank byte {bank}"),
        };
        let parts: Vec<String> = banks.iter().map(|&(b, n)| format!("{}: {n}", describe(b))).collect();
        ui.label(format!("{} single dump(s) — {}", received.len(), parts.join(", ")));
        let bad = received.iter().filter(|d| !d.checksum_ok).count();
        if bad > 0 {
            ui.colored_label(WARN, format!("⚠ {bad} with a wrong checksum"));
        }

        let result = if ui.button("Import as a new library bank").on_hover_text("Safe backup: nothing is overwritten.").clicked() {
            let name = match banks.as_slice() {
                [(bank, _)] => format!("From synth: {}", describe(*bank)),
                _ => "From synth".to_owned(),
            };
            Some(import_as_new_bank(db, &name, received).map(|r| (r, name)))
        } else if ui.button("Import into library banks A–D").on_hover_text("Overwrites the library slots they came from.").clicked() {
            Some(import_dumps(db, received, ImportTarget::OriginalBanks).map(|r| (r, "banks A–D".to_owned())))
        } else {
            if ui.button("Discard").clicked() {
                received.clear();
            }
            None
        };
        match result {
            Some(Ok((report, target))) => {
                actions.push(LibAction::Notice(Notice::info(format!(
                    "Imported {} received patch(es) into {target}.",
                    report.imported
                ))));
                self.loose.extend(report.loose);
                received.clear();
            }
            Some(Err(e)) => actions.push(LibAction::Notice(Notice::error(e))),
            None => {}
        }
    }

    fn loose_panel(&mut self, ui: &mut Ui, db: &PatchDatabase, actions: &mut Vec<LibAction>) {
        ui.label(RichText::new("Edit-buffer dumps").strong().color(ACCENT))
            .on_hover_text("Dumps with bank byte 00 are never put in a slot by their part number (SPEC §5.6).");
        let mut store = None;
        let mut remove = None;
        for (index, dump) in self.loose.iter().enumerate() {
            let patch = Patch::from_bytes(&dump.data);
            let part = if dump.program == 0x40 { "Single".to_owned() } else { format!("part {}", dump.program + 1) };
            ui.horizontal(|ui| {
                let mut label = format!("{} ({part})", patch.name());
                if !dump.checksum_ok {
                    label.push_str(" ⚠");
                }
                ui.label(RichText::new(label).monospace().size(11.0));
            });
            ui.horizontal(|ui| {
                if ui.small_button("Load").clicked() {
                    actions.push(LibAction::Load { patch: patch.clone(), origin: format!("edit-buffer dump ({part})") });
                }
                if ui.small_button("Store…").clicked() {
                    store = Some(patch.clone());
                }
                if ui.small_button("✖").on_hover_text("Remove from this list").clicked() {
                    remove = Some(index);
                }
            });
        }
        if let Some(patch) = store {
            self.open_store_dialog(db, Some(patch));
        }
        if let Some(index) = remove {
            self.loose.remove(index);
        }
    }

    fn import_from_file(&mut self, db: &PatchDatabase, actions: &mut Vec<LibAction>) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("SysEx / MIDI", &["syx", "mid", "midi", "bin"])
            .add_filter("All files", &["*"])
            .pick_file()
        else {
            return;
        };
        let target = match (self.import_into_selected, self.query.bank_id) {
            (true, Some(id)) => ImportTarget::Bank(id),
            _ => ImportTarget::OriginalBanks,
        };
        match import_file(db, &path, target) {
            Ok(outcome) => {
                let file = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                let text = format!("{file}: {}", outcome.summary());
                actions.push(LibAction::Notice(if outcome.has_warnings() { Notice::warning(text) } else { Notice::info(text) }));
                self.loose = outcome.report.loose.clone();
                self.last_import = Some(outcome);
            }
            Err(e) => actions.push(LibAction::Notice(Notice::error(e))),
        }
    }

    fn export_to_file(&mut self, db: &PatchDatabase, actions: &mut Vec<LibAction>) {
        let Some(bank) = self.query.bank_id.and_then(|id| self.cache.bank(id)).cloned() else {
            actions.push(LibAction::Notice(Notice::warning("Select a bank to export.")));
            return;
        };
        let file_name: String = bank.name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
        let Some(path) = rfd::FileDialog::new()
            .add_filter("SysEx", &["syx"])
            .set_file_name(format!("{file_name}.syx"))
            .save_file()
        else {
            return;
        };
        match export_bank_file(db, bank.id, self.export_as, &path) {
            Ok(count) => actions.push(LibAction::Notice(Notice::info(format!(
                "Exported {count} patch(es) of {} to {}.",
                bank.name,
                path.display()
            )))),
            Err(e) => actions.push(LibAction::Notice(Notice::error(e))),
        }
    }

    fn patch_list(&mut self, ui: &mut Ui, db: &PatchDatabase, actions: &mut Vec<LibAction>) {
        ui.horizontal(|ui| {
            ui.label("🔍");
            ui.add(TextEdit::singleline(&mut self.query.search).hint_text("Search name or tags").desired_width(160.0));
            let category_text = self.query.category.map_or("Any category".to_owned(), |c| c.to_string());
            ComboBox::from_id_source("category-filter").selected_text(category_text).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.query.category, None, "Any category");
                for category in SoundCategory::ALL {
                    ui.selectable_value(&mut self.query.category, Some(category), category.name());
                }
            });
            ui.checkbox(&mut self.query.favorites_only, "★ only");
            if self.query.is_filtered() {
                ui.checkbox(&mut self.query.this_bank_only, "This bank only");
                if ui.button("Clear").clicked() {
                    self.query.search.clear();
                    self.query.category = None;
                    self.query.favorites_only = false;
                }
            }
        });
        self.refresh(db);
        ui.separator();

        let mut click = None;
        let row_height = 22.0;
        if self.query.is_filtered() {
            let rows = &self.cache.results;
            ui.label(RichText::new(format!("{} result(s)", rows.len())).color(DIM));
            ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row_height, rows.len(), |ui, range| {
                for summary in &rows[range] {
                    let label = slot_label(self.cache.bank(summary.bank_id), summary.program_number);
                    let selected = self.selected_id == Some(summary.id);
                    if let Some(c) = patch_row(ui, &label, Some(summary), selected) {
                        click = Some(c);
                    }
                }
            });
        } else if self.cache.slots.is_empty() {
            ui.label(RichText::new("Select a bank.").color(DIM));
        } else {
            let bank = self.query.bank_id.and_then(|id| self.cache.bank(id));
            let used = self.cache.slots.iter().flatten().count();
            ui.label(RichText::new(format!("{used} of 128 slots used")).color(DIM));
            ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row_height, self.cache.slots.len(), |ui, range| {
                for program in range {
                    let slot = self.cache.slots[program].as_ref();
                    let program = program as u8;
                    let selected = self.selected_program == Some(program);
                    if let Some(c) = patch_row(ui, &slot_label(bank, program), slot, selected) {
                        click = Some(match c {
                            RowClick::Select { id, .. } => RowClick::Select { program: Some(program), id },
                            other => other,
                        });
                    }
                }
            });
        }

        match click {
            Some(RowClick::Select { program, id }) => {
                self.selected_program = program;
                self.selected_id = id;
            }
            Some(RowClick::Load(id)) => load(db, id, false, actions),
            Some(RowClick::Send(id)) => load(db, id, true, actions),
            Some(RowClick::Favourite(id, favourite)) => {
                if let Err(e) = db.set_favorite(id, favourite) {
                    actions.push(LibAction::Notice(Notice::error(format!("Cannot change favourite: {e}"))));
                }
            }
            None => {}
        }
    }

    fn details(&mut self, ui: &mut Ui, db: &PatchDatabase, actions: &mut Vec<LibAction>) {
        ui.label(RichText::new("Selected").strong().color(ACCENT));
        let bank = self.query.bank_id.and_then(|id| self.cache.bank(id)).cloned();
        let Some(summary) = self.selected_summary().cloned() else {
            match (self.selected_program, &bank) {
                (Some(program), Some(bank)) if !self.query.is_filtered() => {
                    ui.label(format!("{} — empty slot", slot_label(Some(bank), program)));
                    if ui.button("💾 Store editor patch here…").clicked() {
                        self.open_store_dialog(db, None);
                    }
                }
                _ => {
                    ui.label(RichText::new("Click a slot or a search result.").color(DIM));
                }
            }
            return;
        };

        let summary_bank = self.cache.bank(summary.bank_id).cloned();
        ui.label(RichText::new(&summary.name).monospace().size(16.0));
        ui.label(slot_label(summary_bank.as_ref(), summary.program_number));
        ui.label(format!("Categories: {} / {}", summary.category1, summary.category2));
        if !summary.checksum_ok {
            ui.colored_label(WARN, "⚠ Imported from a dump with a wrong checksum.");
        }
        let mut favourite = summary.favorite;
        if ui.checkbox(&mut favourite, "★ Favourite").changed() {
            if let Err(e) = db.set_favorite(summary.id, favourite) {
                actions.push(LibAction::Notice(Notice::error(format!("Cannot change favourite: {e}"))));
            }
        }

        if self.tags_for != Some(summary.id) {
            self.tags_for = Some(summary.id);
            self.tags_buffer.clone_from(&summary.tags);
        }
        ui.label("Tags");
        ui.add(TextEdit::singleline(&mut self.tags_buffer).desired_width(f32::INFINITY));
        let dirty = self.tags_buffer != summary.tags;
        if ui.add_enabled(dirty, Button::new("Save tags")).clicked() {
            match db.set_tags(summary.id, self.tags_buffer.trim()) {
                Ok(_) => actions.push(LibAction::Notice(Notice::info(format!("Tags of '{}' saved.", summary.name)))),
                Err(e) => actions.push(LibAction::Notice(Notice::error(format!("Cannot save tags: {e}")))),
            }
        }

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Load").on_hover_text("Load into the editor (not sent)").clicked() {
                load(db, summary.id, false, actions);
            }
            if ui.button("Send").on_hover_text("Load and send to the synth's edit buffer").clicked() {
                load(db, summary.id, true, actions);
            }
        });
        if summary_bank.as_ref().map(|b| b.id) == self.query.bank_id
            && ui.button("💾 Store editor patch here…").clicked()
        {
            self.selected_program = Some(summary.program_number);
            self.open_store_dialog(db, None);
        }
    }

    /// Modal-style windows: storing a patch (U4) and sending a bank (L4).
    pub fn show_dialogs(&mut self, ctx: &Context, db: &PatchDatabase, editor_patch: &Patch, actions: &mut Vec<LibAction>) {
        if self.store.is_some() || self.confirm_send_bank.is_some() {
            self.refresh(db);
        }
        self.store_dialog(ctx, db, editor_patch, actions);
        self.send_bank_dialog(ctx, actions);
    }

    fn store_dialog(&mut self, ctx: &Context, db: &PatchDatabase, editor_patch: &Patch, actions: &mut Vec<LibAction>) {
        let Some(mut dialog) = self.store.take() else { return };
        let patch = dialog.patch.clone().unwrap_or_else(|| editor_patch.clone());
        let mut open = true;
        let mut done = false;
        let mut picked_bank = None;
        Window::new("Store patch").collapsible(false).resizable(false).open(&mut open).show(ctx, |ui| {
            let source = if dialog.patch.is_some() { "edit-buffer dump" } else { "editor patch" };
            ui.label(format!("Store '{}' ({source}) in the library.", patch.name()));
            ui.horizontal(|ui| {
                ui.label("Bank");
                let current = self.query.bank_id.and_then(|id| self.cache.bank(id)).map_or("—".to_owned(), |b| b.name.clone());
                ComboBox::from_id_source("store-bank").selected_text(current).show_ui(ui, |ui| {
                    for bank in &self.cache.banks {
                        if ui.selectable_label(self.query.bank_id == Some(bank.id), &bank.name).clicked() {
                            picked_bank = Some(bank.id);
                        }
                    }
                });
                ui.label("Slot");
                ui.add(egui::DragValue::new(&mut dialog.program).clamp_range(0..=127));
                if ui.button("First free").clicked() {
                    if let Some(free) = first_free_slot(&self.cache.slots) {
                        dialog.program = free;
                    }
                }
            });
            let bank = self.query.bank_id.and_then(|id| self.cache.bank(id));
            let label = slot_label(bank, dialog.program);
            let occupant = self.cache.slots.get(dialog.program as usize).and_then(Option::as_ref);
            if bank.is_some_and(|b| b.is_rom) {
                ui.colored_label(WARN, "This bank is ROM on the synth; the library copy can't be sent back to it.");
            }
            let confirm = match occupant {
                Some(existing) => {
                    ui.colored_label(DANGER, format!("{label} holds '{}'. Storing will overwrite it.", existing.name));
                    Button::new(RichText::new(format!("Overwrite {label}")).color(DANGER))
                }
                None => {
                    ui.label(format!("{label} is empty."));
                    Button::new(format!("Store to {label}"))
                }
            };
            if let Some(error) = &dialog.error {
                ui.colored_label(DANGER, error);
            }
            ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
                if ui.add_enabled(bank.is_some(), confirm).clicked() {
                    let bank_id = bank.map(|b| b.id).unwrap_or_default();
                    match store_patch(db, bank_id, dialog.program, &patch) {
                        Ok(id) => {
                            actions.push(LibAction::Notice(Notice::info(format!("Stored '{}' to {label}.", patch.name()))));
                            self.selected_program = Some(dialog.program);
                            self.selected_id = Some(id);
                            done = true;
                        }
                        Err(e) => dialog.error = Some(format!("Store failed: {e}")),
                    }
                }
                if ui.button("Cancel").clicked() {
                    done = true;
                }
            });
        });
        if let Some(id) = picked_bank {
            self.select_bank(id);
        }
        if open && !done {
            self.store = Some(dialog);
        }
    }

    fn send_bank_dialog(&mut self, ctx: &Context, actions: &mut Vec<LibAction>) {
        let Some(bank_id) = self.confirm_send_bank else { return };
        let Some(bank) = self.cache.bank(bank_id).cloned() else {
            self.confirm_send_bank = None;
            return;
        };
        let letter = bank.bank_letter.clone().unwrap_or_default();
        let synth_bank = bank.bank_letter.as_deref().and_then(|l| SingleBank::ALL.into_iter().find(|b| b.letter().to_string() == l));
        let count = if self.query.bank_id == Some(bank_id) { self.cache.slots.iter().flatten().count() } else { 0 };
        let mut open = true;
        let mut close = false;
        Window::new(format!("Send bank {letter} to the synth")).collapsible(false).resizable(false).open(&mut open).show(ctx, |ui| {
            ui.colored_label(
                DANGER,
                format!(
                    "This overwrites programs in the synth's RAM bank {letter} irrevocably (manual p. 177). \
                     {count} stored slot(s) will be sent; empty library slots are skipped."
                ),
            );
            ui.label("Back up the synth's bank first: request it, then import it as a new library bank under 'Received from synth'.");
            ui.horizontal(|ui| {
                if let Some(synth_bank) = synth_bank {
                    if ui.button(format!("📥 Back up synth bank {letter}")).clicked() {
                        actions.push(LibAction::RequestBank(synth_bank));
                    }
                }
                if ui.add_enabled(count > 0, Button::new(RichText::new(format!("Send {count} patch(es)")).color(DANGER))).clicked() {
                    actions.push(LibAction::SendBank { bank_id });
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if !open || close {
            self.confirm_send_bank = None;
        }
    }
}

fn load(db: &PatchDatabase, id: i64, send: bool, actions: &mut Vec<LibAction>) {
    match db.get_patch_by_id(id) {
        Ok(Some(record)) => {
            let patch = Patch::from_bytes(&record.data);
            let origin = format!("library ({})", record.name);
            actions.push(if send { LibAction::SendToEditBuffer { patch, origin } } else { LibAction::Load { patch, origin } });
        }
        Ok(None) => actions.push(LibAction::Notice(Notice::error("That patch no longer exists."))),
        Err(e) => actions.push(LibAction::Notice(Notice::error(format!("Cannot load patch: {e}")))),
    }
}

/// One row of the slot table or the search results.
fn patch_row(ui: &mut Ui, label: &str, slot: Option<&PatchSummary>, selected: bool) -> Option<RowClick> {
    let mut click = None;
    ui.horizontal(|ui| {
        ui.add_sized([62.0, 18.0], egui::Label::new(RichText::new(label).monospace().color(DIM)));
        let Some(summary) = slot else {
            if ui.add_sized([150.0, 18.0], egui::SelectableLabel::new(selected, RichText::new("— empty —").color(DIM))).clicked() {
                click = Some(RowClick::Select { program: None, id: None });
            }
            return;
        };
        let star = if summary.favorite { RichText::new("★").color(WARN) } else { RichText::new("☆").color(DIM) };
        if ui.add(Button::new(star).frame(false)).on_hover_text("Favourite").clicked() {
            click = Some(RowClick::Favourite(summary.id, !summary.favorite));
        }
        let name = ui.add_sized([110.0, 18.0], egui::SelectableLabel::new(selected, RichText::new(&summary.name).monospace()));
        if name.clicked() {
            click = Some(RowClick::Select { program: Some(summary.program_number), id: Some(summary.id) });
        }
        if name.double_clicked() {
            click = Some(RowClick::Load(summary.id));
        }
        let categories = match summary.category2 {
            SoundCategory::Off => summary.category1.to_string(),
            c2 => format!("{} / {c2}", summary.category1),
        };
        ui.add_sized([150.0, 18.0], egui::Label::new(RichText::new(categories).size(11.0)));
        if !summary.checksum_ok {
            ui.colored_label(WARN, "⚠").on_hover_text("Imported from a dump with a wrong checksum");
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.small_button("Send").on_hover_text("Send to the synth's edit buffer").clicked() {
                click = Some(RowClick::Send(summary.id));
            }
            if ui.small_button("Load").on_hover_text("Load into the editor").clicked() {
                click = Some(RowClick::Load(summary.id));
            }
            ui.add(egui::Label::new(RichText::new(&summary.tags).color(DIM).size(11.0)).truncate(true));
        });
    });
    click
}

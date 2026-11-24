//! Patch librarian: banks, the 128 slots of a bank, search, favourites and tags, file
//! import/export, bank transfer to/from the synth, and storing the edited patch.

use egui::{Align, Button, ComboBox, Context, Layout, RichText, ScrollArea, TextEdit, Ui, Window};
use virus_db::{
    BadChecksumPolicy, ImportTarget, IncomingDump, IncomingMultiDump, PatchDatabase, PatchSummary, SoundCategory,
};
use virus_syx::{Patch, SingleBank};

use super::{Notice, ACCENT, DANGER, DIM, WARN};
use crate::comforts::LibrarySlot;
use crate::dialogs::FileJobs;
use crate::library::{
    first_free_slot, flagged_count, import_as_new_banks, plan_store, sendable_count, slot_label, store_patch,
    ImportOutcome, LibraryCache, LibraryQuery, MarkChecked, PendingImport, StorePlan, MARK_CHECKED_EXPLANATION,
};

mod files;
use files::PendingFile;

/// Pause between dumps when sending a bank, so the synth can write each program. The Virus sends
/// its own bank dumps ≈340 ms apart (SPEC §3.7), so ours shouldn't come faster.
pub const BANK_SEND_GAP_MS: u64 = 300;

/// Shown in the bank-send confirmations: clock merged into the Virus MIDI IN can't be seen by the editor.
pub const TRANSPORT_REMINDER: &str = "Stop the clock/transport on your sequencer first.";

/// What the librarian asks the app to do.
#[derive(Debug, Clone)]
pub enum LibAction {
    /// `select`: the library slot, for "Select on synth" (F7) once the load has happened (E6).
    Load { patch: Patch, origin: String, select: Option<LibrarySlot> },
    SendToEditBuffer { patch: Patch, origin: String },
    RequestBank(SingleBank),
    /// `Skip` leaves patches flagged ⚠ out, `KeepInvalid` sends them as stored.
    SendBank { bank_id: i64, policy: BadChecksumPolicy },
    CancelBatch,
    Notice(Notice),
    /// [comforts] The editor's own patch was stored in a library slot (clears "edited").
    StoredEditor { label: String },
    /// [F9] Multi edit-buffer dumps from a file import, for the Multi tab's list.
    LooseMultis(Vec<IncomingMultiDump>),
    /// "Fetch from synth first": request the edit buffer, adopt it, then store it in this slot.
    FetchThenStore { bank_id: i64, program: u8 },
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
    /// File dialogs and file I/O on a worker thread (U5).
    pub files: FileJobs,
    pending_file: Option<PendingFile>,
    /// F7: loading a library patch also selects it on the synth.
    pub select_on_synth: bool,
    /// An import that would overwrite stored patches, waiting for the user's choice (H1).
    pending_import: Option<PendingImport>,
    pending_from_received: bool,
    /// Bank bytes (`bb`) whose synth dump was imported into the library (L2 backup gate).
    backed_up: Vec<u8>,
    send_without_backup: bool,
    send_flagged: bool,
    export_policy: BadChecksumPolicy,
    /// [E6] The editor is in Compare: the session holds the original, so storing "the editor
    /// patch" would store that instead of the edits. Set by the app every frame.
    pub editor_comparing: bool,
    /// [R9] A "mark as checked" waiting for confirmation.
    mark_checked: Option<MarkChecked>,
    /// Remembered in settings.json: the Store dialog fetches the synth's edit buffer first.
    pub fetch_before_store: bool,
    /// Set by the app every frame: a MIDI output is connected, and the editor has unstored edits.
    pub output_connected: bool,
    pub editor_edited: bool,
}

/// [E6] Hover text for the store buttons while the editor compares.
const LEAVE_COMPARE_FIRST: &str = "Leave Compare first: the editor is playing the original patch, not your edits.";

impl LibrarianView {
    pub fn bank_id(&self) -> Option<i64> {
        self.query.bank_id
    }

    pub fn set_bank_id(&mut self, bank_id: Option<i64>) {
        self.query.bank_id = bank_id;
    }

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

    /// [E6] Why the editor's patch can't be stored right now, if it can't.
    fn editor_store_blocked(&self, dialog_patch: Option<&Patch>) -> Option<&'static str> {
        (dialog_patch.is_none() && self.editor_comparing).then_some(LEAVE_COMPARE_FIRST)
    }

    /// [E6] "Store editor patch here…", disabled while the editor compares.
    fn store_editor_button(&self, ui: &mut Ui) -> bool {
        ui.add_enabled(!self.editor_comparing, Button::new("💾 Store editor patch here…"))
            .on_disabled_hover_text(LEAVE_COMPARE_FIRST)
            .clicked()
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        db: &PatchDatabase,
        received: &mut Vec<IncomingDump>,
        actions: &mut Vec<LibAction>,
    ) {
        self.refresh(db);
        self.pending_import_dialog(ui.ctx(), db, received, actions);
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
        let busy = self.files.busy();
        if let Some(label) = busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(label).color(DIM));
            });
        }
        let idle = busy.is_none();
        if ui.add_enabled(idle, Button::new("📂 Import .syx / .mid / .bin…")).clicked() {
            self.start_import(&ui.ctx().clone(), actions);
        }
        ui.checkbox(&mut self.import_into_selected, "Import into the selected bank")
            .on_hover_text("Off: dumps go to the bank they were saved from (bb = A–D). On: all go into the selected bank.");
        ui.horizontal(|ui| {
            if ui.add_enabled(idle, Button::new("💾 Export bank .syx…")).clicked() {
                self.start_export(&ui.ctx().clone(), actions);
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
            let policy_text = match self.export_policy {
                BadChecksumPolicy::Skip => "⚠ skip",
                BadChecksumPolicy::KeepInvalid => "⚠ as stored",
                BadChecksumPolicy::Rechecksum => "⚠ fix",
            };
            ComboBox::from_id_source("export-flagged").width(78.0).selected_text(policy_text).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.export_policy, BadChecksumPolicy::Skip, "⚠ skip (default)");
                ui.selectable_value(&mut self.export_policy, BadChecksumPolicy::KeepInvalid, "⚠ as stored (bad checksum)");
                ui.selectable_value(&mut self.export_policy, BadChecksumPolicy::Rechecksum, "⚠ fix checksum (alters data)");
            })
            .response
            .on_hover_text("Patches flagged ⚠ (bad checksum, or migrated from an old library) are left out unless you choose otherwise.");
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
        ui.checkbox(&mut self.select_on_synth, "Select on synth (library bank A–D mirrors the synth's)")
            .on_hover_text(
                "When you load a patch from library bank A–D, also switch the edited part to the same bank and \
                 program on the synth (SysEx C32/C33). Only correct while the library bank holds the same \
                 sounds as the synth's bank; user banks are never selected.",
            );
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
            self.send_without_backup = false;
            self.send_flagged = false;
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

        let mut new_banks = false;
        let mut into_slots = false;
        ui.horizontal_wrapped(|ui| {
            new_banks = ui
                .button("Import as new library bank(s)")
                .on_hover_text("Safe backup: nothing is overwritten. One new bank per source bank.")
                .clicked();
            into_slots = ui
                .button("Import into library banks A–D")
                .on_hover_text("Overwrites the library slots they came from (asks first if any are occupied).")
                .clicked();
            if ui.button("Discard").clicked() {
                received.clear();
            }
        });
        if received.is_empty() {
            return;
        }
        if new_banks {
            let name = match banks.as_slice() {
                [(bank, _)] => format!("From synth: {}", describe(*bank)),
                _ => "From synth".to_owned(),
            };
            let result = import_as_new_banks(db, &name, received).map(|r| (r, name));
            self.finish_received(result, received, actions);
        } else if into_slots {
            match PendingImport::from_dumps(db, "received from synth", received, ImportTarget::OriginalBanks) {
                Ok(pending) if pending.needs_confirmation() => {
                    self.pending_import = Some(pending);
                    self.pending_from_received = true;
                }
                Ok(pending) => {
                    let result = pending.commit_overwrite(db).map(|o| (o.report, "banks A–D".to_owned()));
                    self.finish_received(result, received, actions);
                }
                Err(e) => actions.push(LibAction::Notice(Notice::error(e))),
            }
        }
    }

    fn finish_received(
        &mut self,
        result: Result<(virus_db::ImportReport, String), String>,
        received: &mut Vec<IncomingDump>,
        actions: &mut Vec<LibAction>,
    ) {
        match result {
            Ok((report, target)) => {
                actions.push(LibAction::Notice(Notice::info(format!(
                    "Imported {} received patch(es) into {target}.",
                    report.imported
                ))));
                for bb in crate::library::complete_banks(received) {
                    if !self.backed_up.contains(&bb) {
                        self.backed_up.push(bb);
                    }
                }
                self.loose.extend(report.loose);
                received.clear();
            }
            Err(e) => actions.push(LibAction::Notice(Notice::error(e))),
        }
    }

    /// Confirmation before an import overwrites stored patches (H1).
    fn pending_import_dialog(
        &mut self,
        ctx: &Context,
        db: &PatchDatabase,
        received: &mut Vec<IncomingDump>,
        actions: &mut Vec<LibAction>,
    ) {
        let Some(pending) = self.pending_import.take() else { return };
        let mut open = true;
        let mut choice = None;
        let mut cancelled = false;
        Window::new("Import would overwrite patches").collapsible(false).resizable(false).open(&mut open).show(ctx, |ui| {
            let multis = pending.multi_overwrites.len();
            let n = pending.plan.overwrites.len() + multis;
            ui.colored_label(DANGER, format!("{}: {n} occupied slot(s) would be overwritten.", pending.label));
            ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                for o in pending.plan.overwrites.iter().take(40) {
                    ui.label(
                        RichText::new(format!(
                            "{} {:03}: '{}' → '{}'",
                            o.slot.bank_name, o.slot.program, o.old_name, o.new_name
                        ))
                        .monospace()
                        .size(11.0),
                    );
                }
                if pending.plan.overwrites.len() > 40 {
                    ui.label(format!("… and {} more", pending.plan.overwrites.len() - 40));
                }
                // [F9] multis from the same file go to the Multi bank
                for o in pending.multi_overwrites.iter().take(40) {
                    ui.label(
                        RichText::new(format!("Multi M{:03}: '{}' → '{}'", o.slot, o.old_name, o.new_name))
                            .monospace()
                            .size(11.0),
                    );
                }
                if multis > 40 {
                    ui.label(format!("… and {} more multis", multis - 40));
                }
            });
            if !pending.plan.duplicates.is_empty() {
                ui.colored_label(
                    WARN,
                    format!("⚠ {} slot(s) are addressed by several dumps; the last one wins.", pending.plan.duplicates.len()),
                );
            }
            ui.horizontal(|ui| {
                let hover = if multis > 0 {
                    "Nothing existing is touched: singles go to new library banks, and multis whose Multi bank \
                     slot is occupied are left out."
                } else {
                    "Nothing existing is touched."
                };
                if ui.button("Import as new bank(s)").on_hover_text(hover).clicked() {
                    choice = Some(false);
                }
                if ui.add(Button::new(RichText::new(format!("Overwrite {n} slot(s)")).color(DANGER))).clicked() {
                    choice = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    cancelled = true;
                }
            });
        });
        if cancelled || !open {
            self.pending_from_received = false;
            return;
        }
        let Some(overwrite) = choice else {
            self.pending_import = Some(pending);
            return;
        };
        let name = if pending.label.is_empty() { "Imported".to_owned() } else { pending.label.clone() };
        let result = if overwrite {
            pending.commit_overwrite(db)
        } else {
            pending.commit_as_new_banks(db, &name)
        };
        if std::mem::take(&mut self.pending_from_received) {
            self.finish_received(result.map(|o| (o.report, name)), received, actions);
        } else {
            self.finish_import(&pending.label, result, actions);
        }
    }

    fn finish_import(&mut self, label: &str, result: Result<ImportOutcome, String>, actions: &mut Vec<LibAction>) {
        match result {
            Ok(outcome) => {
                let text = format!("{label}: {}", outcome.summary());
                actions.push(LibAction::Notice(if outcome.has_warnings() { Notice::warning(text) } else { Notice::info(text) }));
                self.loose = outcome.report.loose.clone();
                if let Some(multis) = outcome.multis.as_ref().filter(|m| !m.loose.is_empty()) {
                    actions.push(LibAction::LooseMultis(multis.loose.clone())); // [F9]
                }
                self.last_import = Some(outcome);
            }
            Err(e) => actions.push(LibAction::Notice(Notice::error(e))),
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
                    actions.push(LibAction::Load {
                        patch: patch.clone(),
                        origin: format!("edit-buffer dump ({part})"),
                        select: None,
                    });
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
            let flagged = flagged_count(&self.cache.slots);
            let mut mark_bank = None;
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{used} of 128 slots used")).color(DIM));
                // [R9] clear the ⚠ flags of the whole bank, after a confirmation
                if let Some(bank) = bank.filter(|_| flagged > 0) {
                    ui.colored_label(WARN, format!("⚠ {flagged} flagged"));
                    if ui
                        .small_button("✔ Mark all as checked…")
                        .on_hover_text("Clear the ⚠ flag of every flagged patch in this bank (asks first)")
                        .clicked()
                    {
                        mark_bank = Some(MarkChecked::Bank { id: bank.id, name: bank.name.clone(), flagged });
                    }
                }
            });
            if mark_bank.is_some() {
                self.mark_checked = mark_bank;
            }
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
                    if self.store_editor_button(ui) {
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
            ui.colored_label(WARN, "⚠ Flagged: imported with a wrong checksum, or migrated from an old library.");
            if ui.small_button("✔ Mark as checked…").on_hover_text("Clear the ⚠ flag (asks first)").clicked() {
                let label = slot_label(summary_bank.as_ref(), summary.program_number);
                self.mark_checked = Some(MarkChecked::Patch { id: summary.id, label }); // [R9]
            }
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
        if summary_bank.as_ref().map(|b| b.id) == self.query.bank_id && self.store_editor_button(ui) {
            self.selected_program = Some(summary.program_number);
            self.open_store_dialog(db, None);
        }
    }

    /// Modal-style windows: storing a patch (U4), sending a bank (L4), marking as checked (R9).
    pub fn show_dialogs(&mut self, ctx: &Context, db: &PatchDatabase, editor_patch: &Patch, actions: &mut Vec<LibAction>) {
        if self.store.is_some() || self.confirm_send_bank.is_some() {
            self.refresh(db);
        }
        self.store_dialog(ctx, db, editor_patch, actions);
        self.send_bank_dialog(ctx, actions);
        self.mark_checked_dialog(ctx, db, actions);
        self.poll_files(ctx, db, actions);
    }

    /// [R9] Explains what clearing the ⚠ flag means before doing it.
    fn mark_checked_dialog(&mut self, ctx: &Context, db: &PatchDatabase, actions: &mut Vec<LibAction>) {
        let Some(request) = self.mark_checked.take() else { return };
        let mut open = true;
        let mut answer = None;
        Window::new("Mark as checked").collapsible(false).resizable(false).open(&mut open).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.label(RichText::new(request.question()).strong());
            ui.label(MARK_CHECKED_EXPLANATION);
            ui.horizontal(|ui| {
                if ui.button("✔ Mark as checked").clicked() {
                    answer = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    answer = Some(false);
                }
            });
        });
        match answer {
            Some(true) => actions.push(LibAction::Notice(match request.apply(db) {
                Ok(text) => Notice::info(text),
                Err(e) => Notice::error(e),
            })),
            Some(false) => {}
            None if open => self.mark_checked = Some(request),
            None => {}
        }
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
            // [E6] Compare started while the dialog was open: the session holds the original.
            let blocked = self.editor_store_blocked(dialog.patch.as_ref());
            if let Some(reason) = blocked {
                ui.colored_label(WARN, reason);
            }
            let editor_patch = dialog.patch.is_none();
            if editor_patch {
                ui.add_enabled(
                    self.output_connected,
                    egui::Checkbox::new(&mut self.fetch_before_store, "Fetch from synth first"),
                )
                .on_hover_text(
                    "Other controllers can change the synth behind the editor's back. Request the synth's \
                     edit buffer and store that sound instead of the editor's copy (needs a MIDI output).",
                )
                .on_disabled_hover_text("Connect a MIDI output to fetch from the synth.");
                if self.fetch_before_store && self.output_connected && self.editor_edited {
                    ui.colored_label(WARN, "Your unsaved edits will be replaced by the synth's sound (Undo brings them back).");
                }
            }
            ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
                if ui.add_enabled(bank.is_some() && blocked.is_none(), confirm).clicked() {
                    let bank_id = bank.map(|b| b.id).unwrap_or_default();
                    if plan_store(self.fetch_before_store, editor_patch, self.output_connected) == StorePlan::FetchThenStore {
                        actions.push(LibAction::FetchThenStore { bank_id, program: dialog.program });
                        self.selected_program = Some(dialog.program);
                        done = true;
                    } else {
                        match store_patch(db, bank_id, dialog.program, &patch) {
                            Ok(id) => {
                                actions.push(LibAction::Notice(Notice::info(format!("Stored '{}' to {label}.", patch.name()))));
                                if dialog.patch.is_none() {
                                    actions.push(LibAction::StoredEditor { label: label.clone() }); // [comforts]
                                }
                                self.selected_program = Some(dialog.program);
                                self.selected_id = Some(id);
                                done = true;
                            }
                            Err(e) => dialog.error = Some(format!("Store failed: {e}")),
                        }
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
        let flagged = if self.query.bank_id == Some(bank_id) { flagged_count(&self.cache.slots) } else { 0 };
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
            ui.label(TRANSPORT_REMINDER);
            ui.label("Back up the synth's bank first: request it, then import it as a new library bank under 'Received from synth'.");
            let backed_up = synth_bank.is_some_and(|b| self.backed_up.contains(&b.to_raw()));
            if backed_up {
                ui.colored_label(ACCENT, format!("✔ Bank {letter} was backed up from the synth in this session."));
            } else {
                ui.checkbox(&mut self.send_without_backup, "Send without a backup (the synth's current bank is lost)");
            }
            if flagged > 0 {
                ui.colored_label(
                    WARN,
                    format!("⚠ {flagged} of the stored patches are flagged (bad checksum or migrated from an old library)."),
                );
                ui.label(RichText::new("Checked them? Use \"Mark all as checked…\" above the slot list.").color(DIM).size(11.0));
                ui.checkbox(&mut self.send_flagged, "Send them anyway (the synth may refuse or mis-load them)");
            }
            let n = sendable_count(count, flagged, self.send_flagged);
            let can_send = n > 0 && (backed_up || self.send_without_backup);
            ui.horizontal(|ui| {
                if let Some(synth_bank) = synth_bank {
                    if ui.button(format!("📥 Back up synth bank {letter}")).clicked() {
                        actions.push(LibAction::RequestBank(synth_bank));
                    }
                }
                if ui.add_enabled(can_send, Button::new(RichText::new(format!("Send {n} patch(es)")).color(DANGER))).clicked() {
                    let policy = if self.send_flagged { BadChecksumPolicy::KeepInvalid } else { BadChecksumPolicy::Skip };
                    actions.push(LibAction::SendBank { bank_id, policy });
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
            actions.push(if send {
                LibAction::SendToEditBuffer { patch, origin }
            } else {
                let select = LibrarySlot { bank_id: record.bank_id, program: record.program_number };
                LibAction::Load { patch, origin, select: Some(select) }
            });
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
            ui.colored_label(WARN, "⚠").on_hover_text("Flagged: imported with a wrong checksum, or migrated from an old library");
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

#[cfg(test)]
mod tests {
    use virus_db::{ImportTarget, PatchMeta};

    use super::*;

    fn bank_a(db: &PatchDatabase) -> i64 {
        db.get_banks().unwrap().into_iter().find(|b| b.bank_letter.as_deref() == Some("A")).unwrap().id
    }

    /// [E6] The synth selection rides inside the Load, so a held/cancelled replace can't send it.
    #[test]
    fn loading_a_library_patch_is_one_action_carrying_its_slot() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let a = bank_a(&db);
        let id = db.save_patch(a, 9, Patch::init().as_bytes(), &PatchMeta::default()).unwrap();
        let mut actions = Vec::new();
        load(&db, id, false, &mut actions);
        match actions.as_slice() {
            [LibAction::Load { select, .. }] => assert_eq!(*select, Some(LibrarySlot { bank_id: a, program: 9 })),
            other => panic!("unexpected {other:?}"),
        }
        actions.clear();
        load(&db, id, true, &mut actions);
        assert!(matches!(actions.as_slice(), [LibAction::SendToEditBuffer { .. }]));
    }

    /// [E6] While comparing, the session holds the original, so the editor patch can't be stored.
    #[test]
    fn storing_the_editor_patch_is_blocked_while_comparing() {
        let mut view = LibrarianView::default();
        assert_eq!(view.editor_store_blocked(None), None);
        view.editor_comparing = true;
        assert!(view.editor_store_blocked(None).unwrap().contains("Leave Compare first"));
        let dump = Patch::init();
        assert_eq!(view.editor_store_blocked(Some(&dump)), None, "an edit-buffer dump is not the editor patch");
    }

    /// [R9] Flagged patches show the per-patch and per-bank buttons and the dialog renders.
    #[test]
    fn flagged_patches_render_mark_as_checked_controls() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let dump = IncomingDump { bank: 1, program: 0, data: *Patch::init().as_bytes(), checksum_ok: false };
        db.import_single_dumps(&[dump], ImportTarget::OriginalBanks).unwrap();
        let a = bank_a(&db);
        let mut view = LibrarianView::default();
        view.set_bank_id(Some(a));
        view.refresh(&db);
        view.selected_program = Some(0);
        view.selected_id = view.cache.slots[0].as_ref().map(|s| s.id);
        view.mark_checked = Some(MarkChecked::Bank { id: a, name: "Bank A".into(), flagged: 1 });
        let ctx = Context::default();
        let (mut received, mut actions) = (Vec::new(), Vec::new());
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| view.show(ui, &db, &mut received, &mut actions));
                view.show_dialogs(ctx, &db, &Patch::init(), &mut actions);
            });
        }
        assert!(view.mark_checked.is_some(), "the dialog stays open until answered");
        assert!(!db.get_patch_by_slot(a, 0).unwrap().unwrap().checksum_ok, "nothing changes without confirmation");
    }
}

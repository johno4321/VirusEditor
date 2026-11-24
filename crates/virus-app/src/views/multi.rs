//! Multi tab (TODO F4/F5): the 16 parts of the Multi edit buffer, live edits via Page C, fetch
//! and send of the Multi edit buffer, and a librarian for the Multi bank.

use egui::{Button, ComboBox, Context, DragValue, Grid, RichText, ScrollArea, TextEdit, Ui, Window};
use std::time::Duration;
use virus_db::{BadChecksumPolicy, IncomingMultiDump, PatchDatabase};
use virus_midi::DUMP_GAP_MS;
use virus_syx::{
    multi_common_params, multi_part_params, Multi, MultiLocation, MultiParamDef, ValueKind, MULTI_BANK_SLOTS,
    MULTI_PARTS,
};

use super::librarian::TRANSPORT_REMINDER;
use super::{Notice, accent, DANGER, DIM, WARN};
use crate::dialogs::{FileJobs, Outcome, Task};
use crate::library::{
    sendable_count, ExportResult, ImportOutcome, MarkChecked, PendingImport, MARK_CHECKED_EXPLANATION, STORES_FLAGGED,
};
use crate::multi_session::{
    commit_multi_import, display_value, export_multi_bank_bytes, export_summary, is_complete_multi_bank,
    multi_backup_bytes, multi_import_has_warnings, multi_import_summary, prepare_multi_import,
    prepare_multi_import_bytes, store_edited_multi, store_multi, MultiConfirm, MultiLibraryCache, MultiSession,
    SingleNames,
};
use crate::session::{dump_batch_duration, EditPart, MidiSettings};

/// What the Multi tab asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultiAction {
    /// Send one SysEx message to the synth.
    Send(Vec<u8>),
    /// Switch to the sound editor (after picking a part to edit, F4).
    OpenEditor,
    Notice(Notice),
    /// [F9] Send the library's Multi bank to the synth, paced; flagged multis per `policy`.
    SendBank { policy: BadChecksumPolicy },
    /// [F9] Stop a running bank send.
    CancelBatch,
}

/// What the running file task is for, captured when the user clicked (CR-M4).
enum MultiFile {
    /// Backup of the received multis; the bytes are taken when the button is clicked.
    SaveReceived { bytes: Vec<u8>, count: usize, complete: bool },
    Import,
    Export { result: ExportResult },
}

/// An import into the Multi bank that would overwrite stored multis (CR-H2).
struct MultiImportDialog {
    pending: PendingImport,
    /// The dumps came from "Received from synth" (cleared once imported).
    from_received: bool,
}

#[derive(Default)]
pub struct MultiView {
    library: MultiLibraryCache,
    singles: SingleNames,
    selected_slot: Option<u8>,
    /// Multi edit-buffer dumps from the last file import; never put in a slot automatically.
    loose: Vec<IncomingMultiDump>,
    name_buffer: String,
    name_for: Option<String>,
    // [F9] sending the Multi bank
    confirm_send_bank: bool,
    /// A complete Multi bank from the synth was saved to a file in this session.
    backed_up: bool,
    send_without_backup: bool,
    send_flagged: bool,
    /// File dialogs and file I/O on a worker thread (CR-M4, like the Librarian's U5).
    pub files: FileJobs,
    pending_file: Option<MultiFile>,
    pending_import: Option<MultiImportDialog>,
    /// A delete or overwrite waiting for confirmation (CR-L2).
    confirm: Option<MultiConfirm>,
    /// What the Multi bank file export does with flagged multis (CR-H3).
    export_policy: BadChecksumPolicy,
    /// A "mark as checked" of a flagged multi waiting for confirmation (like R9 for singles).
    mark_checked: Option<MarkChecked>,
    /// Set by the app every frame: a bank or Multi bank send is running (CR-L11).
    pub batch_running: bool,
}

impl MultiView {
    /// [F9] Multi edit-buffer dumps found by the Librarian's file import.
    pub fn add_loose(&mut self, dumps: Vec<IncomingMultiDump>) {
        self.loose.extend(dumps);
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        db: &PatchDatabase,
        session: &mut MultiSession,
        settings: &mut MidiSettings,
        actions: &mut Vec<MultiAction>,
    ) {
        self.library.refresh(db);
        self.singles.refresh(db);
        egui::SidePanel::left("multi-library").resizable(false).exact_width(270.0).show_inside(ui, |ui| {
            ScrollArea::vertical().show(ui, |ui| self.library_panel(ui, db, session, settings, actions));
        });
        egui::CentralPanel::default().show_inside(ui, |ui| {
            ScrollArea::both().show(ui, |ui| self.editor(ui, session, settings, actions));
        });
        self.send_bank_dialog(ui.ctx(), settings, actions);
    }

    /// Finished file tasks and the confirmation windows. Call every frame, whichever tab is
    /// shown, so a file task started here is completed even after switching tabs.
    pub fn show_dialogs(
        &mut self,
        ctx: &Context,
        db: &PatchDatabase,
        session: &mut MultiSession,
        actions: &mut Vec<MultiAction>,
    ) {
        self.poll_files(ctx, db, session, actions);
        self.import_dialog(ctx, db, session, actions);
        self.confirm_dialog(ctx, db, session, actions);
        self.mark_checked_dialog(ctx, db, actions);
    }

    /// Explains what clearing a multi's ⚠ flag means before doing it (as R9 does for singles).
    fn mark_checked_dialog(&mut self, ctx: &Context, db: &PatchDatabase, actions: &mut Vec<MultiAction>) {
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
            Some(true) => report(request.apply(db), actions),
            Some(false) => {}
            None if open => self.mark_checked = Some(request),
            None => {}
        }
    }

    /// [F9] Confirmation before the Multi bank overwrites the synth's (like L4 for singles).
    fn send_bank_dialog(&mut self, ctx: &egui::Context, settings: &MidiSettings, actions: &mut Vec<MultiAction>) {
        if !self.confirm_send_bank {
            return;
        }
        let count = self.library.slots.iter().flatten().count();
        let flagged = self.library.slots.iter().flatten().filter(|m| !m.checksum_ok).count();
        let mut open = true;
        let mut close = false;
        Window::new("Send the Multi bank to the synth").collapsible(false).resizable(false).open(&mut open).show(ctx, |ui| {
            ui.set_max_width(440.0);
            ui.colored_label(
                DANGER,
                format!(
                    "This overwrites multis in the synth's Multi bank irrevocably. {count} stored multi(s) will be \
                     sent to their slots; empty library slots are skipped."
                ),
            );
            ui.label(format!(
                "Each dump is followed by {DUMP_GAP_MS} ms of silence (about {} s in all).",
                dump_batch_duration(sendable_count(count, flagged, self.send_flagged), Duration::from_millis(DUMP_GAP_MS))
                    .as_secs_f64()
                    .ceil()
            ));
            ui.label(TRANSPORT_REMINDER);
            ui.label(
                "Back up the synth's Multi bank first: request it, wait until all 128 multis have arrived, \
                 then use \"Save to file…\" under the received multis.",
            );
            if self.backed_up {
                ui.colored_label(accent(), "✔ The synth's Multi bank was saved to a file in this session.");
            } else {
                ui.checkbox(&mut self.send_without_backup, "Send without a backup (the synth's current multis are lost)");
            }
            if flagged > 0 {
                ui.colored_label(WARN, format!("⚠ {flagged} of the stored multis are flagged (wrong checksum)."));
                ui.checkbox(&mut self.send_flagged, "Send them anyway (the synth may refuse or mis-load them)");
            }
            let n = sendable_count(count, flagged, self.send_flagged);
            let can_send = n > 0 && (self.backed_up || self.send_without_backup);
            ui.horizontal(|ui| {
                if ui.button("📥 Request the synth's Multi bank").clicked() {
                    actions.push(MultiAction::Send(MultiSession::bank_request(settings)));
                }
                if ui.add_enabled(can_send, Button::new(RichText::new(format!("Send {n} multi(s)")).color(DANGER))).clicked() {
                    let policy = if self.send_flagged { BadChecksumPolicy::KeepInvalid } else { BadChecksumPolicy::Skip };
                    actions.push(MultiAction::SendBank { policy });
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if !open || close {
            self.confirm_send_bank = false;
        }
    }

    fn start_task(&mut self, ctx: &Context, task: Task, actions: &mut Vec<MultiAction>) -> bool {
        let label = task.label();
        let ctx = ctx.clone();
        match self.files.start(task, move || ctx.request_repaint()) {
            Ok(()) => {
                actions.push(MultiAction::Notice(Notice::info(label)));
                true
            }
            Err(_) => false,
        }
    }

    /// [F9] Saves the received multis as a `.syx` backup.
    fn start_save_received(&mut self, ctx: &Context, session: &MultiSession, actions: &mut Vec<MultiAction>) {
        let task = Task::PickExport { dir: self.files.export_dir.clone(), file_name: "Multi_bank_backup.syx".to_owned() };
        if self.start_task(ctx, task, actions) {
            self.pending_file = Some(MultiFile::SaveReceived {
                bytes: multi_backup_bytes(&session.received),
                count: session.received.len(),
                complete: is_complete_multi_bank(&session.received),
            });
        }
    }

    fn start_import(&mut self, ctx: &Context, actions: &mut Vec<MultiAction>) {
        if self.start_task(ctx, Task::PickImport { dir: self.files.import_dir.clone() }, actions) {
            self.pending_file = Some(MultiFile::Import);
        }
    }

    fn start_export(&mut self, ctx: &Context, actions: &mut Vec<MultiAction>) {
        let task = Task::PickExport { dir: self.files.export_dir.clone(), file_name: "Multi_bank.syx".to_owned() };
        if self.start_task(ctx, task, actions) {
            self.pending_file = Some(MultiFile::Export { result: ExportResult::default() });
        }
    }

    /// Applies a finished file task on the UI thread; the database stays on this thread.
    fn poll_files(&mut self, ctx: &Context, db: &PatchDatabase, session: &mut MultiSession, actions: &mut Vec<MultiAction>) {
        let Some(outcome) = self.files.poll() else { return };
        let Some(pending) = self.pending_file.take() else { return };
        match (pending, outcome) {
            (_, Outcome::Failed(e)) => actions.push(MultiAction::Notice(Notice::error(e))),
            (MultiFile::SaveReceived { bytes, count, complete }, Outcome::ExportPicked(Some(path))) => {
                self.files.export_dir = parent_of(&path);
                if self.start_task(ctx, Task::WriteFile { path, bytes }, actions) {
                    self.pending_file = Some(MultiFile::SaveReceived { bytes: Vec::new(), count, complete });
                }
            }
            (MultiFile::SaveReceived { count, complete, .. }, Outcome::Written { path, result }) => match result {
                Ok(()) => {
                    self.backed_up |= complete;
                    let note = if complete { "" } else { " (not all 128 multis had arrived, so it isn't a full backup)" };
                    actions.push(MultiAction::Notice(Notice::info(format!(
                        "Saved {count} received multi(s) to {}{note}.",
                        path.display()
                    ))));
                }
                Err(e) => actions.push(MultiAction::Notice(Notice::error(e))),
            },
            (MultiFile::Import, Outcome::ImportPicked(Some(path))) => {
                self.files.import_dir = parent_of(&path);
                if self.start_task(ctx, Task::ReadFile(path), actions) {
                    self.pending_file = Some(MultiFile::Import);
                }
            }
            (MultiFile::Import, Outcome::Read { path, result }) => {
                let label = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                match result.and_then(|bytes| prepare_multi_import_bytes(db, &label, &bytes)) {
                    Ok(pending) => self.import_or_ask(db, session, pending, false, actions),
                    Err(e) => actions.push(MultiAction::Notice(Notice::error(e))),
                }
            }
            (MultiFile::Export { .. }, Outcome::ExportPicked(Some(path))) => {
                self.files.export_dir = parent_of(&path);
                match export_multi_bank_bytes(db, self.export_policy) {
                    Ok((bytes, result)) => {
                        if self.start_task(ctx, Task::WriteFile { path, bytes }, actions) {
                            self.pending_file = Some(MultiFile::Export { result });
                        }
                    }
                    Err(e) => actions.push(MultiAction::Notice(Notice::error(e))),
                }
            }
            (MultiFile::Export { result }, Outcome::Written { path, result: written }) => {
                actions.push(MultiAction::Notice(match written {
                    Ok(()) => Notice::info(export_summary(&result, &path)),
                    Err(e) => Notice::error(e),
                }));
            }
            // A cancelled dialog, or an outcome that doesn't belong to the pending request.
            _ => {}
        }
    }

    /// Imports at once when nothing stored would be replaced; otherwise asks first (CR-H2).
    fn import_or_ask(
        &mut self,
        db: &PatchDatabase,
        session: &mut MultiSession,
        pending: PendingImport,
        from_received: bool,
        actions: &mut Vec<MultiAction>,
    ) {
        if pending.needs_confirmation() {
            self.pending_import = Some(MultiImportDialog { pending, from_received });
        } else {
            let result = commit_multi_import(db, &pending, true);
            self.finish_import(result, from_received, session, actions);
        }
    }

    fn finish_import(
        &mut self,
        result: Result<ImportOutcome, String>,
        from_received: bool,
        session: &mut MultiSession,
        actions: &mut Vec<MultiAction>,
    ) {
        match result {
            Ok(outcome) => {
                let text = multi_import_summary(&outcome);
                let warn = multi_import_has_warnings(&outcome) || !outcome.multis_left_out.is_empty();
                actions.push(MultiAction::Notice(if warn { Notice::warning(text) } else { Notice::info(text) }));
                if from_received {
                    session.received.clear();
                } else if let Some(report) = outcome.multis {
                    self.loose = report.loose;
                }
            }
            Err(e) => actions.push(MultiAction::Notice(Notice::error(e))),
        }
    }

    /// CR-H2: lists the occupied Multi bank slots an import would replace.
    fn import_dialog(&mut self, ctx: &Context, db: &PatchDatabase, session: &mut MultiSession, actions: &mut Vec<MultiAction>) {
        let Some(dialog) = self.pending_import.take() else { return };
        let mut open = true;
        let mut choice = None;
        let mut cancelled = false;
        Window::new("Import would overwrite multis").collapsible(false).resizable(false).open(&mut open).show(ctx, |ui| {
            let overwrites = &dialog.pending.multi_overwrites;
            ui.colored_label(
                DANGER,
                format!("{}: {} occupied Multi bank slot(s) would be overwritten.", dialog.pending.label, overwrites.len()),
            );
            ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                for o in overwrites.iter().take(40) {
                    ui.label(
                        RichText::new(format!("M{:03}: '{}' → '{}'", o.slot, o.old_name, o.new_name)).monospace().size(11.0),
                    );
                }
                if overwrites.len() > 40 {
                    ui.label(format!("… and {} more", overwrites.len() - 40));
                }
            });
            ui.horizontal(|ui| {
                if ui.add(Button::new(RichText::new("Overwrite").color(DANGER))).clicked() {
                    choice = Some(true);
                }
                if ui.button("Only empty slots").on_hover_text("Occupied slots are left as they are.").clicked() {
                    choice = Some(false);
                }
                if ui.button("Cancel").clicked() {
                    cancelled = true;
                }
            });
        });
        if cancelled || !open {
            return;
        }
        let Some(overwrite) = choice else {
            self.pending_import = Some(dialog);
            return;
        };
        let result = commit_multi_import(db, &dialog.pending, overwrite);
        self.finish_import(result, dialog.from_received, session, actions);
    }

    /// CR-L2: Delete / Overwrite on the Multi tab ask first.
    fn confirm_dialog(&mut self, ctx: &Context, db: &PatchDatabase, session: &MultiSession, actions: &mut Vec<MultiAction>) {
        let Some(request) = self.confirm.take() else { return };
        let mut open = true;
        let mut answer = None;
        Window::new(request.title()).collapsible(false).resizable(false).open(&mut open).show(ctx, |ui| {
            ui.set_max_width(400.0);
            ui.label(request.question(session));
            if matches!(request, MultiConfirm::Overwrite { .. }) && session.flagged() {
                ui.colored_label(WARN, STORES_FLAGGED);
            }
            ui.horizontal(|ui| {
                if ui.add(Button::new(RichText::new(request.confirm_label()).color(DANGER))).clicked() {
                    answer = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    answer = Some(false);
                }
            });
        });
        match answer {
            Some(true) => report(request.apply(db, session), actions),
            Some(false) => {}
            None if open => self.confirm = Some(request),
            None => {}
        }
    }

    fn library_panel(
        &mut self,
        ui: &mut Ui,
        db: &PatchDatabase,
        session: &mut MultiSession,
        settings: &MidiSettings,
        actions: &mut Vec<MultiAction>,
    ) {
        ui.label(RichText::new("Synth").strong().color(accent()));
        ui.horizontal_wrapped(|ui| {
            if ui.button("📥 Fetch multi").on_hover_text("Request the Multi edit buffer (31 00 00)").clicked() {
                actions.push(MultiAction::Send(MultiSession::fetch_request(settings)));
            }
            let can_send = session.multi().is_some();
            if ui
                .add_enabled(can_send, egui::Button::new("📤 Send multi"))
                .on_hover_text("Send the edited multi to the Multi edit buffer (11 00 00)")
                .clicked()
            {
                if let Some(dump) = session.edit_buffer_dump(settings) {
                    actions.push(MultiAction::Send(dump));
                }
            }
            if ui.button("Request bank").on_hover_text("Request all 128 multis (33 01)").clicked() {
                actions.push(MultiAction::Send(MultiSession::bank_request(settings)));
            }
        });
        if !session.received.is_empty() {
            let idle = self.files.busy().is_none();
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("{} multi dump(s) received", session.received.len()));
                if ui.button("Import into Multi bank").on_hover_text("Asks first if occupied slots would be overwritten").clicked() {
                    match prepare_multi_import(db, "received from synth", session.received.clone()) {
                        Ok(pending) => self.import_or_ask(db, session, pending, true, actions),
                        Err(e) => actions.push(MultiAction::Notice(Notice::error(e))),
                    }
                }
                let save = ui.add_enabled(idle, Button::new("Save to file…").small());
                if save.on_hover_text("Back up the received multis as a .syx file").clicked() {
                    self.start_save_received(&ui.ctx().clone(), session, actions);
                }
                if ui.small_button("Discard").clicked() {
                    session.received.clear();
                }
            });
        }

        ui.separator();
        ui.label(RichText::new("Files").strong().color(accent()));
        let busy = self.files.busy();
        if let Some(label) = busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(label).color(DIM));
            });
        }
        let idle = busy.is_none();
        ui.horizontal_wrapped(|ui| {
            let import = ui.add_enabled(idle, Button::new("Import multis…"));
            if import.on_hover_text("Multi dumps from a .syx / .mid file (asks before overwriting)").clicked() {
                self.start_import(&ui.ctx().clone(), actions);
            }
            if ui.add_enabled(idle, Button::new("Export Multi bank…")).clicked() {
                self.start_export(&ui.ctx().clone(), actions);
            }
            let policy_text = match self.export_policy {
                BadChecksumPolicy::Skip => "⚠ skip",
                BadChecksumPolicy::KeepInvalid => "⚠ as stored",
                BadChecksumPolicy::Rechecksum => "⚠ fix",
            };
            ComboBox::from_id_source("multi-export-flagged").width(78.0).selected_text(policy_text).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.export_policy, BadChecksumPolicy::Skip, "⚠ skip (default)");
                ui.selectable_value(&mut self.export_policy, BadChecksumPolicy::KeepInvalid, "⚠ as stored (bad checksum)");
                ui.selectable_value(&mut self.export_policy, BadChecksumPolicy::Rechecksum, "⚠ fix checksum (alters data)");
            })
            .response
            .on_hover_text("Multis flagged ⚠ (bad checksum) are left out of the exported file unless you choose otherwise.");
        });

        if !self.loose.is_empty() {
            ui.separator();
            ui.label(RichText::new("Multi edit-buffer dumps from the file").strong());
            let mut remove = None;
            for (index, dump) in self.loose.iter().enumerate() {
                let multi = Multi::from_bytes(&dump.data);
                let flagged = !dump.checksum_ok;
                ui.horizontal(|ui| {
                    let name = if flagged { format!("{}  ⚠", multi.name()) } else { multi.name() };
                    ui.label(RichText::new(name).monospace());
                    if ui.small_button("Load").clicked() {
                        load(session, &mut self.name_for, multi.clone(), "file (edit buffer)", flagged, actions);
                    }
                    let keep = ui.small_button("Keep");
                    let keep = if flagged {
                        keep.on_hover_text("Store as an unslotted multi (stays flagged ⚠)")
                    } else {
                        keep.on_hover_text("Store as an unslotted multi")
                    };
                    if keep.clicked() {
                        let stored = store_multi(db, None, &multi, dump.checksum_ok);
                        report(stored.map(|_| "Kept in the library.").map_err(|e| format!("Library error: {e}")), actions);
                        remove = Some(index);
                    }
                    if ui.small_button("✖").clicked() {
                        remove = Some(index);
                    }
                });
            }
            if let Some(index) = remove {
                self.loose.remove(index);
            }
        }

        ui.separator();
        ui.label(RichText::new("Multi bank").strong().color(accent()));
        if let Some(error) = &self.library.error {
            ui.colored_label(WARN, error);
        }
        // [F9] the whole Multi bank to the synth, after a confirmation
        ui.horizontal_wrapped(|ui| {
            let stored = self.library.slots.iter().flatten().count();
            let send = ui.add_enabled(stored > 0, Button::new("📤 Send Multi bank to synth…"));
            if send.on_hover_text("Overwrites the synth's Multi bank (asks first)").clicked() {
                self.confirm_send_bank = true;
                self.send_without_backup = false;
                self.send_flagged = false;
            }
            let cancel = ui.add_enabled(self.batch_running, Button::new("⏹ Cancel sending").small());
            if cancel.on_disabled_hover_text("Nothing is being sent.").clicked() {
                actions.push(MultiAction::CancelBatch);
            }
        });
        if session.multi().is_some() {
            let slot = self.selected_slot.or_else(|| first_free(&self.library)).unwrap_or(0);
            let occupant = self.library.slots.get(slot as usize).and_then(Option::as_ref);
            let label = match occupant {
                Some(existing) => format!("💾 Overwrite M{slot:03} ({})…", existing.name),
                None => format!("💾 Store in M{slot:03}"),
            };
            if session.flagged() {
                ui.colored_label(WARN, STORES_FLAGGED);
            }
            if ui.button(label).on_hover_text("Store the edited multi in the library's Multi bank").clicked() {
                match occupant {
                    Some(existing) => {
                        self.confirm = Some(MultiConfirm::Overwrite { slot, old_name: existing.name.clone() });
                    }
                    None => report(store_edited_multi(db, session, slot), actions),
                }
                self.selected_slot = Some(slot);
            }
        }
        let selected = self.selected_slot.and_then(|s| self.library.slots.get(s as usize).cloned().flatten());
        if let Some(summary) = selected {
            if !summary.checksum_ok {
                ui.colored_label(WARN, "⚠ Flagged: imported or received with a wrong checksum.");
                if ui.small_button("✔ Mark as checked…").on_hover_text("Clear the ⚠ flag (asks first)").clicked() {
                    let label = summary.slot.map_or_else(|| "unslotted".to_owned(), |s| format!("M{s:03}"));
                    self.mark_checked = Some(MarkChecked::Multi { id: summary.id, label });
                }
            }
            ui.horizontal(|ui| {
                if ui.button("Load").on_hover_text("Load into the Multi editor (not sent)").clicked() {
                    load_record(db, summary.id, session, &mut self.name_for, false, settings, actions);
                }
                if ui.button("Send to synth").on_hover_text("Load and send to the Multi edit buffer").clicked() {
                    load_record(db, summary.id, session, &mut self.name_for, true, settings, actions);
                }
                if ui.button("Delete…").on_hover_text("Delete from the library (asks first)").clicked() {
                    let label = summary.slot.map_or_else(|| "unslotted".to_owned(), |s| format!("M{s:03}"));
                    self.confirm = Some(MultiConfirm::Delete { id: summary.id, label, name: summary.name.clone() });
                }
            });
        }
        let mut picked = None;
        for slot in 0..MULTI_BANK_SLOTS as u8 {
            let entry = self.library.slots.get(slot as usize).and_then(Option::as_ref);
            let mut text = format!("M{slot:03}  {}", entry.map_or("—", |m| m.name.as_str()));
            if entry.is_some_and(|m| !m.checksum_ok) {
                text.push_str("  ⚠");
            }
            let rich = if entry.is_some() { RichText::new(text).monospace() } else { RichText::new(text).monospace().color(DIM) };
            let response = ui.selectable_label(self.selected_slot == Some(slot), rich);
            if response.clicked() {
                picked = Some(slot);
            }
            if response.double_clicked() {
                if let Some(entry) = entry {
                    load_record(db, entry.id, session, &mut self.name_for, false, settings, actions);
                }
            }
        }
        if picked.is_some() {
            self.selected_slot = picked;
        }

        if !self.library.loose.is_empty() {
            ui.separator();
            ui.label(RichText::new("Unslotted multis").strong());
            for summary in self.library.loose.clone() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&summary.name).monospace());
                    if ui.small_button("Load").clicked() {
                        load_record(db, summary.id, session, &mut self.name_for, false, settings, actions);
                    }
                    if ui.small_button("✖").on_hover_text("Delete from the library (asks first)").clicked() {
                        self.confirm =
                            Some(MultiConfirm::Delete { id: summary.id, label: "unslotted".to_owned(), name: summary.name.clone() });
                    }
                });
            }
        }
    }

    fn editor(&mut self, ui: &mut Ui, session: &mut MultiSession, settings: &mut MidiSettings, actions: &mut Vec<MultiAction>) {
        let Some(multi) = session.multi().cloned() else {
            ui.add_space(20.0);
            ui.label(RichText::new("No multi loaded.").strong());
            ui.label("Fetch the Multi edit buffer from the synth, or load a multi from the Multi bank on the left.");
            return;
        };

        // Header: name (live C5–C14), origin, unsent warning.
        if self.name_for.as_deref() != Some(&multi.name()) {
            self.name_buffer = multi.name();
            self.name_for = Some(multi.name());
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("Multi").strong().color(accent()));
            let response = ui.add(TextEdit::singleline(&mut self.name_buffer).char_limit(10).desired_width(110.0).font(egui::TextStyle::Monospace));
            if response.changed() {
                for message in session.set_name(settings, &self.name_buffer) {
                    actions.push(MultiAction::Send(message));
                }
                self.name_for = session.multi().map(Multi::name);
            }
            ui.label(RichText::new(format!("from {}", session.origin)).color(DIM));
            if session.flagged() {
                ui.colored_label(WARN, "⚠ flagged").on_hover_text(STORES_FLAGGED);
            }
            if session.unsent_changes {
                ui.colored_label(WARN, "⚠ Tempo/delay changes are applied only when you send the multi.");
            }
            if let EditPart::Multi(part) = settings.part {
                let multi_channel = session.edited_part_channel(settings.part);
                let channel = crate::audition::auto_channel(settings.channel, settings.part, multi_channel);
                ui.label(RichText::new(format!("Editing part {} (MIDI ch {})", part + 1, channel + 1)).color(DIM));
            }
        });

        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            for def in multi_common_params() {
                ui.vertical(|ui| {
                    let label = match def.location {
                        MultiLocation::Common(15) => format!("{} ({} BPM)", def.short, multi.clock_tempo_bpm()),
                        _ => def.short.to_owned(),
                    };
                    ui.label(RichText::new(label).size(10.5)).on_hover_text(hover(def, multi.get(def, 0)));
                    let mut value = multi.get(def, 0);
                    // Only user changes count: drawing a DragValue may clamp the value (RV-M1).
                    let changed = value_control(ui, def, 0, &mut value).changed();
                    if changed && value != multi.get(def, 0) {
                        if let Some(message) = session.edit(settings, def, 0, value) {
                            actions.push(MultiAction::Send(message));
                        }
                    }
                });
            }
        });

        ui.separator();
        let multi = session.multi().cloned().unwrap_or(multi);
        Grid::new("multi-parts").striped(true).spacing([6.0, 3.0]).show(ui, |ui| {
            ui.label(RichText::new("Part").strong());
            for def in multi_part_params() {
                ui.label(RichText::new(def.short).strong()).on_hover_text(def.name);
                if def.name == "Part Program" {
                    ui.label(RichText::new("Single").strong()).on_hover_text("Name from the library's bank A–D, if stored there");
                }
            }
            ui.label("");
            ui.end_row();

            for part in 0..MULTI_PARTS as u8 {
                let editing = settings.part == EditPart::Multi(part);
                let label = RichText::new(format!("{:>2}", part + 1)).monospace();
                ui.label(if editing { label.color(accent()).strong() } else { label });
                for def in multi_part_params() {
                    let raw = multi.get(def, part);
                    let mut value = raw;
                    let response = value_control(ui, def, part, &mut value).on_hover_text(hover(def, raw));
                    if response.changed() && value != raw {
                        if let Some(message) = session.edit(settings, def, part, value) {
                            actions.push(MultiAction::Send(message));
                        }
                    }
                    if def.name == "Part Program" {
                        let p = multi.part(part);
                        match self.singles.name(p.bank, p.program) {
                            Some(name) => ui.label(RichText::new(name).monospace()),
                            None => ui.label(RichText::new("?").color(DIM)),
                        };
                    }
                }
                if ui
                    .small_button("✎ Edit")
                    .on_hover_text("Edit this part's single: selects the part and fetches its edit buffer (30 00 pp)")
                    .clicked()
                {
                    settings.part = EditPart::Multi(part);
                    actions.push(MultiAction::Send(MultiSession::part_fetch_request(settings, part)));
                    actions.push(MultiAction::OpenEditor);
                    actions.push(MultiAction::Notice(Notice::info(format!("Editing Multi part {}.", part + 1))));
                }
                ui.end_row();
            }
        });
    }
}

fn first_free(library: &MultiLibraryCache) -> Option<u8> {
    library.slots.iter().position(Option::is_none).and_then(|i| u8::try_from(i).ok())
}

fn load(
    session: &mut MultiSession,
    name_for: &mut Option<String>,
    multi: Multi,
    origin: &str,
    flagged: bool,
    actions: &mut Vec<MultiAction>,
) {
    let flag = if flagged { " It is flagged ⚠." } else { "" };
    actions.push(MultiAction::Notice(Notice::info(format!(
        "Loaded multi '{}' from {origin} (not sent).{flag}",
        multi.name()
    ))));
    session.load(multi, origin, flagged);
    *name_for = None;
}

fn load_record(
    db: &PatchDatabase,
    id: i64,
    session: &mut MultiSession,
    name_for: &mut Option<String>,
    send: bool,
    settings: &MidiSettings,
    actions: &mut Vec<MultiAction>,
) {
    match db.get_multi_by_id(id) {
        Ok(Some(record)) => {
            let origin = match record.slot {
                Some(slot) => format!("library M{slot:03}"),
                None => "library".to_owned(),
            };
            load(session, name_for, Multi::from_bytes(&record.data), &origin, !record.checksum_ok, actions);
            if send {
                if let Some(dump) = session.edit_buffer_dump(settings) {
                    actions.push(MultiAction::Send(dump));
                }
            }
        }
        Ok(None) => actions.push(MultiAction::Notice(Notice::warning("That multi no longer exists."))),
        Err(e) => actions.push(MultiAction::Notice(Notice::error(format!("Cannot load the multi: {e}")))),
    }
}

fn report<T: std::fmt::Display>(result: Result<T, String>, actions: &mut Vec<MultiAction>) {
    actions.push(MultiAction::Notice(match result {
        Ok(text) => Notice::info(text.to_string()),
        Err(e) => Notice::error(e),
    }));
}

fn parent_of(path: &std::path::Path) -> Option<std::path::PathBuf> {
    path.parent().filter(|p| !p.as_os_str().is_empty()).map(std::path::Path::to_path_buf)
}

fn hover(def: &MultiParamDef, raw: u8) -> String {
    let mut text = format!("{}\nValue: {}", def.name, display_value(def, raw));
    match def.live {
        Some(live) => text.push_str(&format!("\nSent live as {:?} {}", live.page, live.number)),
        None => text.push_str("\nNo parameter change: sent with the whole multi"),
    }
    if raw < def.min || raw > def.max {
        text.push_str(&format!("\nOutside the documented range {}..{}; kept as is.", def.min, def.max));
    }
    text
}

/// A compact control for one Multi value. Values beyond a list show as `#n` and are kept.
fn value_control(ui: &mut Ui, def: &MultiParamDef, part: u8, value: &mut u8) -> egui::Response {
    match def.kind {
        ValueKind::Toggle => {
            let mut on = *value != 0;
            let text = if *value > 1 { format!("#{value}") } else { String::new() };
            let response = ui.checkbox(&mut on, text);
            if response.changed() {
                *value = u8::from(on);
            }
            response
        }
        ValueKind::Choice(texts) => {
            let known = (*value as usize) < texts.len();
            let selected = display_value(def, *value);
            let before = *value;
            let mut response = ComboBox::from_id_source(("multi", def.name, part))
                .width(if texts.len() > 4 { 72.0 } else { 40.0 })
                .selected_text(if known { RichText::new(selected) } else { RichText::new(selected).color(WARN) })
                .show_ui(ui, |ui| {
                    for (raw, text) in texts.iter().enumerate() {
                        ui.selectable_value(value, raw as u8, *text);
                    }
                    if !known {
                        let current = *value;
                        ui.selectable_value(value, current, RichText::new(format!("#{current} (undocumented)")).color(WARN));
                    }
                })
                .response;
            // The combo's own response doesn't report picks made in its popup.
            if *value != before {
                response.mark_changed();
            }
            response
        }
        _ => {
            let def = *def;
            ui.add(
                DragValue::new(value)
                    .clamp_range(0..=127)
                    .speed(0.25)
                    .custom_formatter(move |n, _| display_value(&def, n as u8)),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use virus_db::PatchMeta;
    use virus_syx::{encode_multi_dump, DEVICE_ID_OMNI};

    use super::*;
    use crate::dialogs::Backend;
    use crate::multi_session::store_multi;

    /// Answers every dialog at once and records what was written.
    struct Fake {
        file: Vec<u8>,
        written: Mutex<Option<(PathBuf, Vec<u8>)>>,
    }

    impl Backend for Fake {
        fn run(&self, task: Task) -> Outcome {
            match task {
                Task::PickImport { .. } => Outcome::ImportPicked(Some(PathBuf::from("in").join("multis.syx"))),
                Task::ReadFile(path) => Outcome::Read { path, result: Ok(self.file.clone()) },
                Task::PickExport { file_name, .. } => Outcome::ExportPicked(Some(PathBuf::from("out").join(file_name))),
                Task::WriteFile { path, bytes } => {
                    *self.written.lock().unwrap() = Some((path.clone(), bytes));
                    Outcome::Written { path, result: Ok(()) }
                }
            }
        }
    }

    fn view_with(file: Vec<u8>) -> (MultiView, Arc<Fake>) {
        let fake = Arc::new(Fake { file, written: Mutex::new(None) });
        (MultiView { files: FileJobs::with_backend(fake.clone()), ..Default::default() }, fake)
    }

    fn multi(name: &str) -> Multi {
        let mut multi = Multi::from_bytes(&[0; 256]);
        multi.set_name(name);
        multi
    }

    fn pump(view: &mut MultiView, ctx: &Context, db: &PatchDatabase, session: &mut MultiSession, actions: &mut Vec<MultiAction>) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while view.files.busy().is_some() || view.pending_file.is_some() {
            view.poll_files(ctx, db, session, actions);
            assert!(Instant::now() < deadline, "file task never finished");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn texts(actions: &[MultiAction]) -> Vec<String> {
        actions
            .iter()
            .filter_map(|a| match a {
                MultiAction::Notice(n) => Some(n.text.clone()),
                _ => None,
            })
            .collect()
    }

    /// [CR-M4, CR-H2] The file is read on the worker; an occupied slot makes the import wait.
    #[test]
    fn importing_a_file_over_an_occupied_slot_waits_for_the_user() {
        let db = PatchDatabase::open_in_memory().unwrap();
        store_multi(&db, Some(3), &multi("Mine"), true).unwrap();
        let (mut view, _) = view_with(encode_multi_dump(DEVICE_ID_OMNI, 1, 3, multi("Theirs").as_bytes()));
        let (ctx, mut session, mut actions) = (Context::default(), MultiSession::default(), Vec::new());

        view.start_import(&ctx, &mut actions);
        assert!(view.files.busy().is_some());
        pump(&mut view, &ctx, &db, &mut session, &mut actions);

        assert!(view.pending_import.is_some(), "{:?}", texts(&actions));
        assert_eq!(db.get_multi_by_slot(3).unwrap().unwrap().name, "Mine");
        assert_eq!(view.files.import_dir, Some(PathBuf::from("in")));
    }

    #[test]
    fn importing_a_file_into_empty_slots_happens_at_once() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let (mut view, _) = view_with(encode_multi_dump(DEVICE_ID_OMNI, 1, 3, multi("Theirs").as_bytes()));
        let (ctx, mut session, mut actions) = (Context::default(), MultiSession::default(), Vec::new());
        view.start_import(&ctx, &mut actions);
        pump(&mut view, &ctx, &db, &mut session, &mut actions);
        assert!(view.pending_import.is_none());
        assert_eq!(db.get_multi_by_slot(3).unwrap().unwrap().name, "Theirs");
    }

    /// [CR-H2] "Import into Multi bank" for received multis asks before overwriting, too.
    #[test]
    fn importing_received_multis_over_an_occupied_slot_waits_and_keeps_the_list() {
        let db = PatchDatabase::open_in_memory().unwrap();
        store_multi(&db, Some(3), &multi("Mine"), true).unwrap();
        let (mut view, _) = view_with(Vec::new());
        let mut session = MultiSession::default();
        session.received.push(IncomingMultiDump { bank: 1, program: 3, data: *multi("Theirs").as_bytes(), checksum_ok: true });
        let pending = prepare_multi_import(&db, "received from synth", session.received.clone()).unwrap();
        let mut actions = Vec::new();
        view.import_or_ask(&db, &mut session, pending, true, &mut actions);
        assert!(view.pending_import.is_some());
        assert_eq!(session.received.len(), 1, "kept until the user decides");
        assert_eq!(db.get_multi_by_slot(3).unwrap().unwrap().name, "Mine");
    }

    /// [CR-M4, CR-H3] The export is written on the worker and leaves flagged multis out by default.
    #[test]
    fn exporting_leaves_flagged_multis_out_and_says_so() {
        let db = PatchDatabase::open_in_memory().unwrap();
        db.save_multi(Some(0), multi("Fine").as_bytes(), &PatchMeta::default(), true).unwrap();
        db.save_multi(Some(1), multi("Damaged").as_bytes(), &PatchMeta::default(), false).unwrap();
        let (mut view, fake) = view_with(Vec::new());
        let (ctx, mut session, mut actions) = (Context::default(), MultiSession::default(), Vec::new());

        view.start_export(&ctx, &mut actions);
        pump(&mut view, &ctx, &db, &mut session, &mut actions);

        let (path, bytes) = fake.written.lock().unwrap().clone().expect("a file was written");
        assert_eq!(path, PathBuf::from("out").join("Multi_bank.syx"));
        assert_eq!(bytes.len(), 267, "only the clean multi");
        let last = texts(&actions).pop().unwrap();
        assert!(last.contains("Exported 1 multi(s)") && last.contains("1 flagged ⚠ multi(s) left out"), "{last}");
    }

    /// [CR-M4] Saving the received multis marks a complete Multi bank as backed up.
    #[test]
    fn saving_a_complete_received_bank_counts_as_a_backup() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let (mut view, fake) = view_with(Vec::new());
        let mut session = MultiSession::default();
        for program in 0..128u8 {
            session.received.push(IncomingMultiDump { bank: 1, program, data: *multi("M").as_bytes(), checksum_ok: true });
        }
        let (ctx, mut actions) = (Context::default(), Vec::new());
        view.start_save_received(&ctx, &session, &mut actions);
        pump(&mut view, &ctx, &db, &mut session, &mut actions);
        assert!(view.backed_up);
        assert_eq!(fake.written.lock().unwrap().as_ref().unwrap().1.len(), 128 * 267);
    }

    /// [CR-L2] Delete and Overwrite dialogs render and change nothing until confirmed.
    #[test]
    fn confirmations_render_and_wait() {
        let db = PatchDatabase::open_in_memory().unwrap();
        let id = store_multi(&db, Some(3), &multi("Mine"), true).unwrap();
        let mut view = MultiView { confirm: Some(MultiConfirm::Delete { id, label: "M003".into(), name: "Mine".into() }), ..Default::default() };
        let mut session = MultiSession::default();
        let ctx = Context::default();
        let mut actions = Vec::new();
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| view.show_dialogs(ctx, &db, &mut session, &mut actions));
        }
        assert!(view.confirm.is_some());
        assert!(db.get_multi_by_slot(3).unwrap().is_some());
    }
}
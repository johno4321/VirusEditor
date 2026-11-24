//! Multi tab (TODO F4/F5): the 16 parts of the Multi edit buffer, live edits via Page C, fetch
//! and send of the Multi edit buffer, and a librarian for the Multi bank.

use egui::{Button, ComboBox, DragValue, Grid, RichText, ScrollArea, TextEdit, Ui, Window};
use virus_db::{BadChecksumPolicy, IncomingMultiDump, PatchDatabase};
use virus_syx::{
    multi_common_params, multi_part_params, Multi, MultiLocation, MultiParamDef, ValueKind, MULTI_BANK_SLOTS,
    MULTI_PARTS,
};

use super::{Notice, ACCENT, DANGER, DIM, WARN};
use crate::library::sendable_count;
use crate::multi_session::{
    display_value, export_multi_bank_file, import_multi_file, import_summary, is_complete_multi_bank,
    multi_backup_bytes, store_multi, MultiLibraryCache, MultiSession, SingleNames, MULTI_BANK_SEND_GAP_MS,
};
use crate::session::{EditPart, MidiSettings};

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
                "The dumps go out {MULTI_BANK_SEND_GAP_MS} ms apart (about {} s).",
                (count as u64 * MULTI_BANK_SEND_GAP_MS).div_ceil(1000)
            ));
            ui.label(
                "Back up the synth's Multi bank first: request it, wait until all 128 multis have arrived, \
                 then use \"Save to file…\" under the received multis.",
            );
            if self.backed_up {
                ui.colored_label(ACCENT, "✔ The synth's Multi bank was saved to a file in this session.");
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

    /// [F9] Saves the received multis as a `.syx` backup.
    fn save_received(&mut self, session: &MultiSession, actions: &mut Vec<MultiAction>) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("SysEx", &["syx"])
            .set_file_name("Multi_bank_backup.syx")
            .save_file()
        else {
            return;
        };
        match std::fs::write(&path, multi_backup_bytes(&session.received)) {
            Ok(()) => {
                let complete = is_complete_multi_bank(&session.received);
                self.backed_up |= complete;
                let note = if complete { "" } else { " (not all 128 multis had arrived, so it isn't a full backup)" };
                actions.push(MultiAction::Notice(Notice::info(format!(
                    "Saved {} received multi(s) to {}{note}.",
                    session.received.len(),
                    path.display()
                ))));
            }
            Err(e) => actions.push(MultiAction::Notice(Notice::error(format!("Cannot write {}: {e}", path.display())))),
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
        ui.label(RichText::new("Synth").strong().color(ACCENT));
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
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("{} multi dump(s) received", session.received.len()));
                if ui.button("Import into Multi bank").clicked() {
                    match db.import_multi_dumps(&session.received) {
                        Ok(report) => {
                            actions.push(MultiAction::Notice(Notice::info(import_summary(&report))));
                            session.received.clear();
                        }
                        Err(e) => actions.push(MultiAction::Notice(Notice::error(format!("Multi import failed: {e}")))),
                    }
                }
                if ui.small_button("Save to file…").on_hover_text("Back up the received multis as a .syx file").clicked() {
                    self.save_received(session, actions);
                }
                if ui.small_button("Discard").clicked() {
                    session.received.clear();
                }
            });
        }

        ui.separator();
        ui.label(RichText::new("Files").strong().color(ACCENT));
        ui.horizontal_wrapped(|ui| {
            if ui.button("Import multis…").on_hover_text("Multi dumps from a .syx / .mid file").clicked() {
                self.import_from_file(db, actions);
            }
            if ui.button("Export Multi bank…").clicked() {
                export_to_file(db, actions);
            }
        });

        if !self.loose.is_empty() {
            ui.separator();
            ui.label(RichText::new("Multi edit-buffer dumps from the file").strong());
            let mut remove = None;
            for (index, dump) in self.loose.iter().enumerate() {
                let multi = Multi::from_bytes(&dump.data);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(multi.name()).monospace());
                    if ui.small_button("Load").clicked() {
                        load(session, &mut self.name_for, multi.clone(), "file (edit buffer)", actions);
                    }
                    if ui.small_button("Keep").on_hover_text("Store as an unslotted multi").clicked() {
                        report_store(store_multi(db, None, &multi).map(|_| "Kept in the library."), actions);
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
        ui.label(RichText::new("Multi bank").strong().color(ACCENT));
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
            if ui.small_button("⏹ Cancel sending").clicked() {
                actions.push(MultiAction::CancelBatch);
            }
        });
        if let Some(multi) = session.multi() {
            let slot = self.selected_slot.or_else(|| first_free(&self.library)).unwrap_or(0);
            let occupant = self.library.slots.get(slot as usize).and_then(Option::as_ref);
            let label = match occupant {
                Some(existing) => format!("💾 Overwrite M{slot:03} ({})", existing.name),
                None => format!("💾 Store in M{slot:03}"),
            };
            if ui.button(label).on_hover_text("Store the edited multi in the library's Multi bank").clicked() {
                report_store(store_multi(db, Some(slot), multi).map(|_| "Stored."), actions);
                self.selected_slot = Some(slot);
            }
        }
        let selected = self.selected_slot.and_then(|s| self.library.slots.get(s as usize).cloned().flatten());
        if let Some(summary) = selected {
            ui.horizontal(|ui| {
                if ui.button("Load").on_hover_text("Load into the Multi editor (not sent)").clicked() {
                    load_record(db, summary.id, session, &mut self.name_for, false, settings, actions);
                }
                if ui.button("Send to synth").on_hover_text("Load and send to the Multi edit buffer").clicked() {
                    load_record(db, summary.id, session, &mut self.name_for, true, settings, actions);
                }
                if ui.button("Delete").clicked() {
                    report_store(db.delete_multi(summary.id).map(|_| "Deleted."), actions);
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
                    if ui.small_button("✖").on_hover_text("Delete from the library").clicked() {
                        report_store(db.delete_multi(summary.id).map(|_| "Deleted."), actions);
                    }
                });
            }
        }
    }

    fn import_from_file(&mut self, db: &PatchDatabase, actions: &mut Vec<MultiAction>) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("SysEx / MIDI", &["syx", "mid", "midi", "bin"])
            .add_filter("All files", &["*"])
            .pick_file()
        else {
            return;
        };
        match import_multi_file(db, &path) {
            Ok(report) => {
                let text = import_summary(&report);
                let warn = report.bad_checksum > 0 || report.skipped > 0;
                actions.push(MultiAction::Notice(if warn { Notice::warning(text) } else { Notice::info(text) }));
                self.loose = report.loose;
            }
            Err(e) => actions.push(MultiAction::Notice(Notice::error(e))),
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
            ui.label(RichText::new("Multi").strong().color(ACCENT));
            let response = ui.add(TextEdit::singleline(&mut self.name_buffer).char_limit(10).desired_width(110.0).font(egui::TextStyle::Monospace));
            if response.changed() {
                for message in session.set_name(settings, &self.name_buffer) {
                    actions.push(MultiAction::Send(message));
                }
                self.name_for = session.multi().map(Multi::name);
            }
            ui.label(RichText::new(format!("from {}", session.origin)).color(DIM));
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
                ui.label(if editing { label.color(ACCENT).strong() } else { label });
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
    actions: &mut Vec<MultiAction>,
) {
    actions.push(MultiAction::Notice(Notice::info(format!(
        "Loaded multi '{}' from {origin} (not sent).",
        multi.name()
    ))));
    session.load(multi, origin);
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
            load(session, name_for, Multi::from_bytes(&record.data), &origin, actions);
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

fn report_store<T: std::fmt::Display, E: std::fmt::Display>(result: Result<T, E>, actions: &mut Vec<MultiAction>) {
    actions.push(MultiAction::Notice(match result {
        Ok(text) => Notice::info(text.to_string()),
        Err(e) => Notice::error(format!("Library error: {e}")),
    }));
}

fn export_to_file(db: &PatchDatabase, actions: &mut Vec<MultiAction>) {
    let Some(path) = rfd::FileDialog::new().add_filter("SysEx", &["syx"]).set_file_name("Multi_bank.syx").save_file() else {
        return;
    };
    actions.push(MultiAction::Notice(match export_multi_bank_file(db, &path) {
        Ok(count) => Notice::info(format!("Exported {count} multi(s) to {}.", path.display())),
        Err(e) => Notice::error(e),
    }));
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

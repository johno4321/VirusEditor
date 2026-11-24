//! The eframe application: wires the MIDI worker, the session, the database and the panels.

use std::time::Instant;

use eframe::{egui, App, Frame};
use egui::{CentralPanel, Color32, RichText, TopBottomPanel, Visuals};
use virus_db::PatchDatabase;
use virus_midi::{MidiCommand, MidiEvent, MidiService, ReleaseTrigger};
use virus_syx::Patch;

use crate::audition::Audition;
use crate::comforts::Comforts;
use crate::session::{Activity, EditPart, Incoming, Ports, Session};
use crate::storage::{open_library, LibraryLocation};
use crate::views::comforts::{ComfortAction, ComfortsView};
use crate::views::librarian::{LibAction, LibrarianView, BANK_SEND_GAP_MS};
use crate::views::{editor::EditorView, midi_bar, modulation, Edit, Notice, ACCENT, DANGER, DIM};

/// [comforts] A pending replacement of the edited patch, held until the user confirms (E5).
enum Replace {
    Load { patch: Patch, origin: String },
    SendToEditBuffer { patch: Patch, origin: String },
    Init,
    Fetch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Editor,
    Modulation,
    Librarian,
}

pub struct VirusApp {
    session: Session,
    ports: Ports,
    leds: Activity,
    midi: Option<MidiService>,
    db: PatchDatabase,
    location: LibraryLocation,
    tab: Tab,
    editor: EditorView,
    librarian: LibrarianView,
    audition: Audition,
    notice: Notice,
    /// Where the edited patch came from, for the header.
    origin: String,
    /// [comforts] undo/redo, compare, original patch (E1–E5).
    comforts: Comforts,
    comfort_ui: ComfortsView,
    pending_replace: Option<Replace>,
    /// Edited part and device the editor was last in sync with (RV-M5).
    last_target: (EditPart, u8),
}

impl VirusApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut visuals = Visuals::dark();
        visuals.panel_fill = Color32::from_rgb(22, 24, 28);
        visuals.window_fill = Color32::from_rgb(26, 28, 34);
        visuals.extreme_bg_color = Color32::from_rgb(14, 15, 18);
        visuals.selection.bg_fill = Color32::from_rgb(200, 60, 40);
        visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, Color32::from_rgb(240, 160, 60));
        cc.egui_ctx.set_visuals(visuals);

        let mut notice = Notice::info("Ready. Select the MIDI ports, or let auto-detect find the Virus.");
        let midi = match MidiService::spawn() {
            Ok(service) => {
                // Repaint as soon as MIDI arrives instead of polling every frame (U2).
                let ctx = cc.egui_ctx.clone();
                service.set_event_notifier(Box::new(move || ctx.request_repaint()));
                Some(service)
            }
            Err(e) => {
                notice = Notice::error(format!("MIDI unavailable: {e}"));
                None
            }
        };

        let (db, location) = open_library();
        if let LibraryLocation::InMemory { reason } = &location {
            notice = Notice::error(format!("Library is in memory only and will NOT be saved: {reason}"));
        }

        Self {
            session: Session::default(),
            ports: Ports::default(),
            leds: Activity::default(),
            midi,
            db,
            location,
            tab: Tab::default(),
            editor: EditorView::default(),
            librarian: LibrarianView::default(),
            audition: Audition::new(),
            notice,
            origin: "init".to_owned(),
            comforts: Comforts::new(Patch::init()),
            comfort_ui: ComfortsView::default(),
            pending_replace: None,
            last_target: (EditPart::default(), 0),
        }
    }

    fn command(&mut self, command: MidiCommand) {
        match &self.midi {
            Some(midi) => {
                if let Err(e) = midi.send_command(command) {
                    self.notice = Notice::error(format!("MIDI worker stopped: {e}"));
                }
            }
            None => self.notice = Notice::error("MIDI is unavailable."),
        }
    }

    /// Sends one message. The Tx LED lights only when the worker reports `Sent` (R2).
    fn send(&mut self, message: Vec<u8>) {
        if !self.ports.output_connected() {
            self.comforts.mark_unsynced(); // [comforts] nothing reaches the synth
        }
        if self.midi.is_some() && !self.ports.output_connected() {
            self.notice = Notice::warning("No MIDI output connected: nothing was sent.");
            return;
        }
        self.command(MidiCommand::Send(message));
    }

    /// Sends audition notes; silently dropped without an output (the strip is disabled then).
    fn send_notes(&mut self, messages: Vec<Vec<u8>>) {
        if !self.ports.output_connected() {
            return;
        }
        for message in messages {
            self.command(MidiCommand::Send(message));
        }
    }

    /// Note offs must go out before the output is closed or changed (SPEC §5.9).
    fn release_notes_before(&mut self, commands: &[MidiCommand]) {
        let closes_output = commands.iter().any(|c| {
            matches!(
                c,
                MidiCommand::Disconnect | MidiCommand::DisconnectOutput | MidiCommand::ConnectOutput(_)
            )
        });
        if closes_output && self.audition.is_active() {
            let offs = self.audition.release_all(ReleaseTrigger::Disconnect);
            self.send_notes(offs);
        }
    }

    fn poll_midi(&mut self) {
        let Some(midi) = &self.midi else { return };
        let events: Vec<MidiEvent> = std::iter::from_fn(|| midi.try_recv_event()).collect();
        let now = Instant::now();
        for event in events {
            match event {
                MidiEvent::PortListUpdated { inputs, outputs } => {
                    let commands = self.ports.on_port_list(inputs, outputs);
                    self.release_notes_before(&commands);
                    for command in commands {
                        self.command(command);
                    }
                }
                MidiEvent::ConnectionStatus { input, output } => {
                    self.notice = Notice::info(format!(
                        "MIDI In: {} | Out: {}",
                        input.as_deref().unwrap_or("none"),
                        output.as_deref().unwrap_or("none")
                    ));
                    self.ports.on_status(input, output);
                }
                MidiEvent::Sent { .. } => self.leds.tx(now),
                MidiEvent::Error(e) => self.notice = Notice::error(format!("MIDI: {e}")),
                incoming => {
                    self.leds.rx(now);
                    self.handle_incoming(&incoming);
                }
            }
        }
    }

    fn handle_incoming(&mut self, event: &MidiEvent) {
        // [comforts] what a replacing dump would discard (RV-M5)
        let previous = self.comforts.edited(&self.session).clone();
        let was_dirty = self.comforts.is_edited(&self.session);
        match self.session.handle_midi(event) {
            Incoming::PatchLoaded { name, checksum_ok } => {
                self.origin = "synth".to_owned();
                if was_dirty {
                    self.comforts.adopt_dump(&previous, &self.session, 0.0);
                } else {
                    self.comforts.reset(self.session.patch().clone(), true);
                }
                self.notice = if checksum_ok {
                    Notice::info(format!("Received '{name}' from the synth."))
                } else {
                    Notice::warning(format!("Received '{name}' from the synth with a wrong checksum."))
                };
                if was_dirty {
                    self.notice = Notice::warning(format!(
                        "Received '{name}' from the synth; your unsaved edits were replaced — Undo brings them back."
                    ));
                }
            }
            Incoming::DumpQueued { bank, program, checksum_ok } => {
                let count = self.session.received.len();
                let flag = if checksum_ok { "" } else { " (wrong checksum)" };
                self.notice = Notice::info(format!(
                    "Received dump bb={bank:02X} ss={program}{flag}; {count} waiting under Librarian → Received from synth."
                ));
            }
            Incoming::ParamChanged { .. } | Incoming::Ignored => {}
        }
    }

    fn apply_edits(&mut self, edits: Vec<Edit>) {
        for edit in edits {
            match edit {
                Edit::Param { page, number, value } => {
                    if let Some(message) = self.session.edit(page, number, value) {
                        self.send(message);
                    }
                }
                Edit::Name(name) => {
                    for message in self.session.set_name(&name) {
                        self.send(message);
                    }
                }
            }
        }
    }

    fn load(&mut self, patch: Patch, origin: String) {
        self.notice = Notice::info(format!(
            "Loaded '{}' from {origin} into the editor (not sent). Press Send to put it on the synth.",
            patch.name()
        ));
        self.comforts.reset(patch.clone(), false); // [comforts] new original + new history; synth still holds the old sound
        self.session.load_patch(patch);
        self.origin = origin;
    }

    /// [comforts] Sends the edited patch to the synth's edit buffer and records that it is there.
    fn send_edited_patch(&mut self) {
        self.send(self.session.edit_buffer_dump(self.session.patch()));
        if self.ports.output_connected() {
            self.comforts.mark_sent();
        }
    }

    /// [comforts] Replaces the edited patch, asking first if it has unstored edits (E5).
    fn request_replace(&mut self, action: Replace) {
        if self.comforts.comparing() {
            self.comfort_action(ComfortAction::ToggleCompare, 0.0);
        }
        if self.comforts.is_edited(&self.session) {
            self.pending_replace = Some(action);
        } else {
            self.perform_replace(action);
        }
    }

    fn perform_replace(&mut self, action: Replace) {
        match action {
            Replace::Load { patch, origin } => self.load(patch, origin),
            Replace::Init => self.load(Patch::init(), "init".to_owned()),
            Replace::Fetch => self.send(self.session.fetch_request()),
            Replace::SendToEditBuffer { patch, origin } => {
                let message = self.session.edit_buffer_dump(&patch);
                self.load(patch, origin);
                self.send(message);
                if self.ports.output_connected() {
                    self.comforts.mark_sent();
                    self.notice = Notice::info(format!(
                        "Sent '{}' to the synth's edit buffer ({}).",
                        self.session.patch().name(),
                        self.session.settings.part.label()
                    ));
                }
            }
        }
    }

    /// [comforts] The "discard unstored edits?" dialog.
    fn replace_dialog(&mut self, ctx: &egui::Context) {
        if self.pending_replace.is_none() {
            return;
        }
        let mut choice = None;
        egui::Window::new("Replace the edited patch?").collapsible(false).resizable(false).show(ctx, |ui| {
            ui.label(format!(
                "'{}' has edits that are not stored in the library. Replacing it discards them (and the undo history).",
                self.comforts.edited(&self.session).name()
            ));
            ui.horizontal(|ui| {
                if ui.button("Replace").clicked() {
                    choice = Some(true);
                }
                if ui.button("Store first…").clicked() {
                    self.librarian.open_store_dialog(&self.db, None);
                    choice = Some(false);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(false);
                }
            });
        });
        match choice {
            Some(true) => {
                if let Some(action) = self.pending_replace.take() {
                    self.perform_replace(action);
                }
            }
            Some(false) => self.pending_replace = None,
            None => {}
        }
    }

    /// [comforts] Undo/redo/compare/section actions; every change goes to the synth.
    fn comfort_action(&mut self, action: ComfortAction, now: f64) {
        let messages = match action {
            ComfortAction::Undo => self.comforts.undo(&mut self.session),
            ComfortAction::Redo => self.comforts.redo(&mut self.session),
            ComfortAction::SendNow => {
                self.send_edited_patch();
                if self.ports.output_connected() {
                    self.notice = Notice::info(format!("Sent '{}' to the synth's edit buffer.", self.session.patch().name()));
                }
                Vec::new()
            }
            ComfortAction::ToggleCompare => {
                let messages = self.comforts.toggle_compare(&mut self.session);
                self.notice = if self.comforts.comparing() {
                    Notice::info("Compare: playing the original patch. Press Compare again to return to your edits.")
                } else {
                    Notice::info("Compare ended: back to the edited patch.")
                };
                messages
            }
            ComfortAction::Section { clip, verb } => {
                let messages = self.comforts.paste_section(&mut self.session, &clip, verb, now);
                self.notice = Notice::info(format!("{verb} {}: {} byte(s) changed.", clip.section, messages.len()));
                messages
            }
        };
        for message in messages {
            self.send(message);
        }
    }

    /// [comforts] Ctrl+Z undo, Ctrl+Y / Ctrl+Shift+Z redo (left to text fields while one has focus).
    fn comfort_shortcuts(&mut self, ctx: &egui::Context) -> Option<ComfortAction> {
        if ctx.wants_keyboard_input() {
            return None;
        }
        use egui::{Key, Modifiers};
        let redo = ctx.input_mut(|i| {
            i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || i.consume_key(Modifiers::COMMAND, Key::Y)
        });
        if redo {
            return Some(ComfortAction::Redo);
        }
        ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Z)).then_some(ComfortAction::Undo)
    }

    fn handle_library_action(&mut self, action: LibAction) {
        match action {
            LibAction::Load { patch, origin } => self.request_replace(Replace::Load { patch, origin }),
            LibAction::SendToEditBuffer { patch, origin } => {
                self.request_replace(Replace::SendToEditBuffer { patch, origin });
            }
            LibAction::StoredEditor { label } => {
                self.comforts.mark_stored(self.session.patch().clone()); // [comforts]
                self.origin = label;
            }
            LibAction::RequestBank(bank) => {
                self.send(self.session.bank_request(bank));
                if self.ports.output_connected() {
                    self.notice = Notice::info(format!(
                        "Requested bank {}; the dumps will appear under Received from synth.",
                        bank.letter()
                    ));
                }
            }
            LibAction::SendBank { bank_id } => self.send_bank(bank_id),
            LibAction::CancelBatch => {
                self.command(MidiCommand::CancelBatch);
                self.notice = Notice::info("Pending bank transfer cancelled.");
            }
            LibAction::Notice(notice) => self.notice = notice,
        }
    }

    fn send_bank(&mut self, bank_id: i64) {
        if !self.ports.output_connected() {
            self.notice = Notice::warning("No MIDI output connected: the bank was not sent.");
            return;
        }
        match self.db.export_bank_dumps(bank_id, self.session.settings.device_id, None) {
            Ok(messages) => {
                let count = messages.len();
                self.command(MidiCommand::SendBatch { messages, gap_ms: BANK_SEND_GAP_MS });
                self.notice = Notice::info(format!("Sending {count} patch(es) to the synth…"));
            }
            Err(e) => self.notice = Notice::error(format!("Cannot send the bank: {e}")),
        }
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("ACCESS VIRUS").strong().size(16.0).color(ACCENT));
            ui.label(RichText::new("Rack Editor & Librarian").size(12.0).color(DIM));
            ui.separator();
            ui.selectable_value(&mut self.tab, Tab::Editor, "🎹 Sound Editor");
            ui.selectable_value(&mut self.tab, Tab::Modulation, "🔀 Modulation");
            ui.selectable_value(&mut self.tab, Tab::Librarian, "📚 Librarian");
            ui.separator();
            ui.label(RichText::new(self.session.patch().name()).monospace().strong())
                .on_hover_text(format!("Edited patch, from {}", self.origin));

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let store = egui::Button::new("💾 Store…");
                // [comforts] the store dialog saves the session patch, which is the original while comparing
                if ui
                    .add_enabled(!self.comforts.comparing(), store)
                    .on_hover_text("Store the edited patch in a library slot")
                    .clicked()
                {
                    self.librarian.open_store_dialog(&self.db, None);
                }
                if ui.button("📤 Send").on_hover_text("Send the edited patch to the synth's edit buffer").clicked() {
                    self.send_edited_patch();
                }
                if ui.button("📥 Fetch").on_hover_text("Request the synth's edit buffer").clicked() {
                    self.request_replace(Replace::Fetch);
                }
                if ui.button("Init").on_hover_text("Load a neutral init sound (not sent)").clicked() {
                    self.request_replace(Replace::Init);
                }
            });
        });
        match &self.location {
            LibraryLocation::InMemory { reason } => {
                ui.colored_label(DANGER, format!("⚠ The library is in memory only — nothing will be saved ({reason})."));
            }
            LibraryLocation::File(_) => {}
        }
    }
}

impl App for VirusApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut Frame) {
        self.poll_midi();
        let shortcut = self.comfort_shortcuts(ctx); // [comforts]
        let now_secs = ctx.input(|i| i.time);
        if let Some(action) = shortcut {
            self.comfort_action(action, now_secs);
        }

        TopBottomPanel::top("header").show(ctx, |ui| {
            ui.add_space(4.0);
            self.header(ui);
            ui.add_space(4.0);
        });
        // [comforts] undo/redo, compare and edited badge on every tab
        let bar_actions = TopBottomPanel::top("comforts")
            .show(ctx, |ui| {
                ui.add_space(2.0);
                let actions = self.comfort_ui.bar(ui, &self.comforts, &self.session);
                ui.add_space(2.0);
                actions
            })
            .inner;
        for action in bar_actions {
            self.comfort_action(action, now_secs);
        }
        let midi_available = self.midi.is_some();
        let commands = TopBottomPanel::bottom("midi-bar")
            .show(ctx, |ui| {
                ui.add_space(2.0);
                let commands = midi_bar::show(
                    ui,
                    &mut self.ports,
                    &mut self.session.settings,
                    &self.leds,
                    &self.notice,
                    midi_available,
                );
                if let LibraryLocation::File(path) = &self.location {
                    ui.label(RichText::new(format!("Library: {}", path.display())).size(10.0).color(DIM));
                }
                commands
            })
            .inner;
        self.release_notes_before(&commands);
        // [comforts] a different edited part/device holds a different sound (RV-M5)
        let target = (self.session.settings.part, self.session.settings.device_id);
        if target != self.last_target {
            self.last_target = target;
            self.comforts.mark_unsynced();
            self.notice = Notice::warning("Edited part changed: the editor no longer matches the synth. Send or Fetch.");
        }
        for command in commands {
            self.command(command);
        }

        // Above the MIDI bar, on every tab.
        let target = self.audition.sync_target(self.session.settings.channel, self.session.settings.part);
        self.send_notes(target);
        let output_connected = self.ports.output_connected();
        let now_ms = (ctx.input(|i| i.time) * 1000.0) as u64;
        let notes = TopBottomPanel::bottom("audition")
            .show(ctx, |ui| {
                ui.add_space(2.0);
                let notes = self.audition.show(ui, now_ms, output_connected);
                ui.add_space(2.0);
                notes
            })
            .inner;
        self.send_notes(notes);

        let mut edits = Vec::new();
        let mut actions = Vec::new();
        let comparing = self.comforts.comparing(); // [comforts] editing is disabled while comparing
        CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Editor => {
                ui.add_enabled_ui(!comparing, |ui| self.editor.show(ui, self.session.patch(), &mut edits));
            }
            Tab::Modulation => {
                ui.add_enabled_ui(!comparing, |ui| modulation::show(ui, self.session.patch(), &mut edits));
            }
            Tab::Librarian => self.librarian.show(ui, &self.db, &mut self.session.received, &mut actions),
        });
        self.librarian.show_dialogs(ctx, &self.db, self.session.patch(), &mut actions);
        // [comforts] snapshot around the edits so undo records exact byte changes
        let before = self.session.patch().clone();
        let recorded = if comparing { Vec::new() } else { edits.clone() };
        self.apply_edits(if comparing { Vec::new() } else { edits });
        self.comforts.record(&before, &self.session, &recorded, now_secs);
        if ctx.input(|i| i.pointer.any_released()) {
            self.comforts.seal();
        }
        let window_actions = self.comfort_ui.windows(ctx, &self.comforts, &self.session, &self.db);
        for action in window_actions {
            self.comfort_action(action, now_secs);
        }
        self.replace_dialog(ctx);
        for action in actions {
            self.handle_library_action(action);
        }

        // Only keep repainting while an activity LED is lit.
        if let Some(remaining) = self.leds.next_change(Instant::now()) {
            ctx.request_repaint_after(remaining);
        }
    }

    /// Releases sounding notes and waits briefly for the worker to send them, so quitting
    /// never leaves a note hanging on the synth.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let offs = self.audition.release_all(ReleaseTrigger::Exit);
        let Some(midi) = &self.midi else { return };
        if offs.is_empty() || !self.ports.output_connected() {
            return;
        }
        let mut pending = offs.len();
        for message in offs {
            if midi.send_command(MidiCommand::Send(message)).is_err() {
                return;
            }
        }
        let deadline = Instant::now() + std::time::Duration::from_millis(300);
        while pending > 0 {
            let left = deadline.saturating_duration_since(Instant::now());
            match midi.event_receiver().recv_timeout(left) {
                Ok(MidiEvent::Sent { .. } | MidiEvent::Error(_)) => pending -= 1,
                Ok(_) => {}
                Err(_) => break,
            }
        }
    }
}

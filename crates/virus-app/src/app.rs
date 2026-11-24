//! The eframe application: wires the MIDI worker, the session, the database and the panels.

use std::time::Instant;

use eframe::{egui, App, Frame};
use egui::{CentralPanel, Color32, RichText, TopBottomPanel, Visuals};
use virus_db::PatchDatabase;
use virus_midi::{MidiCommand, MidiEvent, MidiService, ReleaseTrigger};
use virus_syx::Patch;

use crate::audition::Audition;
use crate::session::{Activity, Incoming, Ports, Session};
use crate::storage::{open_library, LibraryLocation};
use crate::views::librarian::{LibAction, LibrarianView, BANK_SEND_GAP_MS};
use crate::views::{editor::EditorView, midi_bar, modulation, Edit, Notice, ACCENT, DANGER, DIM};

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
        match self.session.handle_midi(event) {
            Incoming::PatchLoaded { name, checksum_ok } => {
                self.origin = "synth".to_owned();
                self.notice = if checksum_ok {
                    Notice::info(format!("Received '{name}' from the synth."))
                } else {
                    Notice::warning(format!("Received '{name}' from the synth with a wrong checksum."))
                };
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
        self.notice = Notice::info(format!("Loaded '{}' from {origin} into the editor (not sent).", patch.name()));
        self.session.load_patch(patch);
        self.origin = origin;
    }

    fn handle_library_action(&mut self, action: LibAction) {
        match action {
            LibAction::Load { patch, origin } => self.load(patch, origin),
            LibAction::SendToEditBuffer { patch, origin } => {
                let message = self.session.edit_buffer_dump(&patch);
                self.load(patch, origin);
                self.send(message);
                if self.ports.output_connected() {
                    self.notice = Notice::info(format!(
                        "Sent '{}' to the synth's edit buffer ({}).",
                        self.session.patch().name(),
                        self.session.settings.part.label()
                    ));
                }
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
                if ui.button("💾 Store…").on_hover_text("Store the edited patch in a library slot").clicked() {
                    self.librarian.open_store_dialog(&self.db, None);
                }
                if ui.button("📤 Send").on_hover_text("Send the edited patch to the synth's edit buffer").clicked() {
                    self.send(self.session.edit_buffer_dump(self.session.patch()));
                }
                if ui.button("📥 Fetch").on_hover_text("Request the synth's edit buffer").clicked() {
                    self.send(self.session.fetch_request());
                }
                if ui.button("Init").on_hover_text("Load a neutral init sound (not sent)").clicked() {
                    self.load(Patch::init(), "init".to_owned());
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

        TopBottomPanel::top("header").show(ctx, |ui| {
            ui.add_space(4.0);
            self.header(ui);
            ui.add_space(4.0);
        });

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
        CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Editor => self.editor.show(ui, self.session.patch(), &mut edits),
            Tab::Modulation => modulation::show(ui, self.session.patch(), &mut edits),
            Tab::Librarian => self.librarian.show(ui, &self.db, &mut self.session.received, &mut actions),
        });
        self.librarian.show_dialogs(ctx, &self.db, self.session.patch(), &mut actions);
        self.apply_edits(edits);
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

//! The eframe application: wires the MIDI worker, the session, the database and the panels.

use std::time::Instant;

use eframe::{egui, App, Frame};
use egui::{CentralPanel, RichText, TopBottomPanel};
use virus_db::PatchDatabase;
use virus_midi::{MidiCommand, MidiEvent, MidiService, ReleaseTrigger};
use virus_syx::Patch;

use crate::audition::Audition;
use crate::session::{Activity, Incoming, MidiSettings, Ports, Session};
use crate::settings::{self, FolderPrefs, Loaded, Settings, SettingsStore, WindowPrefs};
use crate::storage::{open_library, LibraryLocation};
use crate::theme::{self, DARK_RED, ON_RED};
use crate::synth_select::select_messages;
use crate::views::librarian::{LibAction, LibrarianView, BANK_SEND_GAP_MS};
use crate::views::{editor::EditorView, midi_bar, modulation, Edit, Notice, DANGER, DIM, WARN};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Editor,
    Modulation,
    Librarian,
}

impl Tab {
    fn key(self) -> &'static str {
        match self {
            Tab::Editor => "editor",
            Tab::Modulation => "modulation",
            Tab::Librarian => "librarian",
        }
    }

    fn from_key(key: &str) -> Self {
        match key {
            "modulation" => Tab::Modulation,
            "librarian" => Tab::Librarian,
            _ => Tab::Editor,
        }
    }
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
    /// Settings between runs (G3).
    settings_store: SettingsStore,
    /// Saved ports, applied when the first port list arrives; kept in the file until then.
    pending_ports: Option<settings::MidiPrefs>,
    window: Option<WindowPrefs>,
    /// Name of the selected library bank, looked up only when the selection changes.
    bank_name: (Option<i64>, Option<String>),
    /// Shown under the header until dismissed (e.g. a corrupt settings file was set aside).
    settings_warning: Option<String>,
}

impl VirusApp {
    pub fn new(cc: &eframe::CreationContext<'_>, store: SettingsStore, loaded: Loaded) -> Self {
        theme::apply(&cc.egui_ctx);

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

        let mut app = Self {
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
            settings_store: store,
            pending_ports: None,
            window: None,
            bank_name: (None, None),
            settings_warning: loaded.warning,
        };
        app.apply_settings(&loaded.settings);
        app
    }

    fn apply_settings(&mut self, s: &Settings) {
        self.session.settings = MidiSettings { channel: s.midi.channel, device_id: s.midi.device_id, part: s.edit_part() };
        self.pending_ports = Some(s.midi.clone());
        let audition = &mut self.audition;
        audition.open = s.audition.open;
        audition.set_octave(s.audition.octave);
        audition.settings.velocity = s.audition.velocity;
        audition.settings.velocity_from_position = s.audition.velocity_from_position;
        audition.settings.audition_notes.clone_from(&s.audition.notes);
        audition.settings.audition_length_ms = s.audition.length_ms;
        self.tab = Tab::from_key(&s.ui.tab);
        self.librarian.select_on_synth = s.ui.select_on_synth;
        if let (Some(name), Ok(banks)) = (&s.ui.bank, self.db.get_banks()) {
            self.librarian.set_bank_id(banks.iter().find(|b| &b.name == name).map(|b| b.id));
        }
        self.librarian.files.import_dir = FolderPrefs::existing(&s.folders.import);
        self.librarian.files.export_dir = FolderPrefs::existing(&s.folders.export);
        self.window = s.window;
    }

    /// Everything worth keeping between runs, as it is now.
    fn snapshot(&mut self) -> Settings {
        let bank_id = self.librarian.bank_id();
        if self.bank_name.0 != bank_id {
            let name = bank_id.and_then(|id| self.db.get_banks().ok()?.into_iter().find(|b| b.id == id)).map(|b| b.name);
            self.bank_name = (bank_id, name);
        }
        let mut midi = match &self.pending_ports {
            Some(saved) => saved.clone(),
            None => settings::MidiPrefs {
                input: self.ports.selected_input.clone(),
                output: self.ports.selected_output.clone(),
                auto_connect: self.ports.auto_connect,
                ..Default::default()
            },
        };
        midi.channel = self.session.settings.channel;
        midi.device_id = self.session.settings.device_id;
        midi.part = self.session.settings.part.raw();
        let mut s = Settings {
            midi,
            audition: settings::AuditionPrefs {
                open: self.audition.open,
                octave: self.audition.octave(),
                velocity_from_position: self.audition.settings.velocity_from_position,
                velocity: self.audition.settings.velocity,
                notes: self.audition.settings.audition_notes.clone(),
                length_ms: self.audition.settings.audition_length_ms,
            },
            ui: settings::UiPrefs {
                tab: self.tab.key().to_owned(),
                bank: self.bank_name.1.clone(),
                select_on_synth: self.librarian.select_on_synth,
            },
            folders: FolderPrefs {
                import: FolderPrefs::from_path(&self.librarian.files.import_dir),
                export: FolderPrefs::from_path(&self.librarian.files.export_dir),
            },
            window: self.window,
            ..Settings::default()
        };
        s.sanitize();
        s
    }

    fn save_settings(&mut self, ctx: Option<&egui::Context>, force: bool) {
        let current = self.snapshot();
        if let Some(error) = self.settings_store.save_if_changed(&current, Instant::now(), force) {
            self.notice = Notice::error(error);
        }
        // A rate-limited change must still be written when no other repaint comes along.
        if let Some(ctx) = ctx.filter(|_| self.settings_store.is_dirty(&current)) {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
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
                    let mut commands = Vec::new();
                    if let Some(saved) = self.pending_ports.take() {
                        commands = settings::restore_ports(&mut self.ports, &saved, &inputs, &outputs);
                    }
                    commands.extend(self.ports.on_port_list(inputs, outputs));
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
            LibAction::SelectOnSynth { bank_id, program } => self.select_on_synth(bank_id, program),
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

    /// F7: switches the edited part to the stored program a loaded library patch mirrors.
    fn select_on_synth(&mut self, bank_id: i64, program: u8) {
        if !self.librarian.select_on_synth || !self.ports.output_connected() {
            return;
        }
        let Ok(banks) = self.db.get_banks() else { return };
        let Some(bank) = banks.into_iter().find(|b| b.id == bank_id) else { return };
        let messages = select_messages(
            self.session.settings.device_id,
            self.session.settings.part,
            bank.bank_letter.as_deref(),
            program,
        );
        if messages.is_empty() {
            return;
        }
        for message in messages {
            self.send(message);
        }
        self.notice = Notice::info(format!(
            "Selected {} on the synth ({}); the editor shows the library copy.",
            crate::library::slot_label(Some(&bank), program),
            self.session.settings.part.label()
        ));
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
            ui.visuals_mut().widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, DARK_RED);
            ui.label(RichText::new("ACCESS VIRUS").strong().size(18.0).color(ON_RED));
            ui.label(RichText::new("Rack Editor & Librarian").size(12.0).color(ON_RED));
            ui.separator();
            theme::tab(ui, &mut self.tab, Tab::Editor, "🎹 Sound Editor");
            theme::tab(ui, &mut self.tab, Tab::Modulation, "🔀 Modulation");
            theme::tab(ui, &mut self.tab, Tab::Librarian, "📚 Librarian");
            ui.separator();
            theme::lcd_frame()
                .show(ui, |ui| ui.label(theme::lcd_text(self.session.patch().name())))
                .response
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
                theme::lcd_frame().show(ui, |ui| {
                    ui.colored_label(DANGER, format!("⚠ The library is in memory only — nothing will be saved ({reason})."));
                });
            }
            LibraryLocation::File(_) => {}
        }
        if self.settings_warning.is_some() {
            ui.horizontal(|ui| {
                if let Some(warning) = &self.settings_warning {
                    ui.colored_label(WARN, format!("⚠ {warning}"));
                }
                if ui.small_button("✖").on_hover_text("Dismiss").clicked() {
                    self.settings_warning = None;
                }
            });
        }
    }
}

impl App for VirusApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut Frame) {
        self.poll_midi();

        TopBottomPanel::top("header").frame(theme::header_frame()).show(ctx, |ui| {
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

        self.window = WindowPrefs::capture(&ctx.input(|i| i.viewport().clone()), self.window);
        self.save_settings(Some(ctx), false);
    }

    /// Releases sounding notes and waits briefly for the worker to send them, so quitting
    /// never leaves a note hanging on the synth.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_settings(None, true);
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

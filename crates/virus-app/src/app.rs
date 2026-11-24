//! The eframe application: wires the MIDI worker, the session, the database and the panels.

use std::time::{Duration, Instant};

use eframe::{egui, App, Frame};
use egui::{CentralPanel, RichText, TopBottomPanel};
use virus_db::{BadChecksumPolicy, PatchDatabase};
use virus_midi::{MidiCommand, MidiEvent, MidiService, ReleaseTrigger};
use virus_syx::Patch;

use crate::audition::Audition;
use crate::comforts::{Comforts, Replace, ReplaceRequest};
use crate::session::{Activity, EditPart, Incoming, MidiSettings, PacedSend, Ports, Session};
use crate::settings::{self, FolderPrefs, Loaded, Settings, SettingsStore, WindowPrefs};
use crate::storage::{adopt_legacy_library, open_library, open_library_at, pending_legacy_library, LibraryLocation};
use crate::theme::{self, DARK_RED, ON_RED};
use crate::synth_select::select_messages;
use crate::views::comforts::{ComfortAction, ComfortsView};
use crate::views::librarian::{LibAction, LibrarianView, BANK_SEND_GAP_MS};
use crate::views::{editor::EditorView, midi_bar, modulation, Edit, Notice, DANGER, DIM, WARN};
// [multi] Multi tab (F4/F5)
use crate::multi_session::{MultiSession, MULTI_BANK_SEND_GAP_MS};
use crate::views::multi::{MultiAction, MultiView};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Editor,
    Modulation,
    Librarian,
    Multi, // [multi]
}

impl Tab {
    fn key(self) -> &'static str {
        match self {
            Tab::Editor => "editor",
            Tab::Modulation => "modulation",
            Tab::Librarian => "librarian",
            Tab::Multi => "multi",
        }
    }

    fn from_key(key: &str) -> Self {
        match key {
            "modulation" => Tab::Modulation,
            "librarian" => Tab::Librarian,
            "multi" => Tab::Multi,
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
    paced: PacedSend,
    /// An old v1 library found next to the exe / in the working directory (M3).
    legacy: Option<std::path::PathBuf>,
    /// [comforts] undo/redo, compare, original patch (E1–E5).
    comforts: Comforts,
    comfort_ui: ComfortsView,
    pending_replace: ReplaceRequest,
    /// Edited part and device the editor was last in sync with (RV-M5).
    last_target: (EditPart, u8),
    // [multi] Multi tab (F4/F5)
    multi: MultiSession,
    multi_view: MultiView,
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

        let legacy = pending_legacy_library(&db, &location);
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
            paced: PacedSend::default(),
            legacy,
            comforts: Comforts::new(Patch::init()),
            comfort_ui: ComfortsView::default(),
            pending_replace: ReplaceRequest::default(),
            last_target: (EditPart::default(), 0),
            multi: MultiSession::default(),    // [multi]
            multi_view: MultiView::default(), // [multi]
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
        if !self.ports.output_connected() {
            self.comforts.mark_unsynced(); // [comforts] nothing reaches the synth
        }
        if self.midi.is_some() && !self.ports.output_connected() {
            self.notice = Notice::warning("No MIDI output connected: nothing was sent.");
            return;
        }
        let Some(message) = self.paced.submit(message, Instant::now()) else { return };
        self.command(MidiCommand::Send(message));
    }

    /// Sends the live messages that waited for a bank transfer to finish.
    fn flush_paced(&mut self) {
        for message in self.paced.drain_ready(Instant::now()) {
            self.command(MidiCommand::Send(message));
        }
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
        // [comforts] what a replacing dump would discard (RV-M5)
        let previous = self.comforts.edited(&self.session).clone();
        let was_dirty = self.comforts.is_edited(&self.session);
        // [multi] Multi dumps and Multi parameter changes; the event still goes to the session.
        let multi = self.multi.handle_midi(event, &self.session.settings);
        if let Some(notice) = multi.notice(self.multi.received.len()) {
            self.notice = notice;
        }
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
        // [E6] held until confirmed; a cancelled Load never selects on the synth
        let edited = self.comforts.is_edited(&self.session);
        if let Some(action) = self.pending_replace.request(action, edited) {
            self.perform_replace(action);
        }
    }

    fn perform_replace(&mut self, action: Replace) {
        match action {
            Replace::Load { patch, origin, select } => {
                self.load(patch, origin);
                if let Some(slot) = select {
                    self.select_on_synth(slot.bank_id, slot.program); // [E6]
                }
            }
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
        let Some(pending) = self.pending_replace.pending() else { return };
        let warning = pending.warning(&self.comforts.edited(&self.session).name()); // [E6]
        let mut choice = None;
        egui::Window::new("Replace the edited patch?").collapsible(false).resizable(false).show(ctx, |ui| {
            ui.label(warning);
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
        if let Some(action) = choice.and_then(|replace| self.pending_replace.resolve(replace)) {
            self.perform_replace(action);
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
            LibAction::Load { patch, origin, select } => self.request_replace(Replace::Load { patch, origin, select }),
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
            LibAction::SendBank { bank_id, policy } => self.send_bank(bank_id, policy),
            LibAction::CancelBatch => {
                self.command(MidiCommand::CancelBatch);
                self.paced.cancel();
                self.notice = Notice::info("Pending bank transfer cancelled.");
            }
            LibAction::Notice(notice) => self.notice = notice,
            LibAction::LooseMultis(dumps) => self.multi_view.add_loose(dumps), // [F9]
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

    fn send_bank(&mut self, bank_id: i64, policy: BadChecksumPolicy) {
        if !self.ports.output_connected() {
            self.notice = Notice::warning("No MIDI output connected: the bank was not sent.");
            return;
        }
        match self.db.export_bank(bank_id, self.session.settings.device_id, None, policy) {
            Ok(export) => {
                let count = export.frames.len();
                if count == 0 {
                    self.notice = Notice::warning("Nothing to send: every stored patch was left out (flagged ⚠).");
                    return;
                }
                self.paced.start(Instant::now(), count, Duration::from_millis(BANK_SEND_GAP_MS));
                self.command(MidiCommand::SendBatch { messages: export.frames, gap_ms: BANK_SEND_GAP_MS });
                let mut text = format!("Sending {count} patch(es) to the synth…");
                if export.skipped_flagged > 0 {
                    text.push_str(&format!(" {} flagged ⚠ patch(es) left out.", export.skipped_flagged));
                }
                self.notice = Notice::info(text);
            }
            Err(e) => self.notice = Notice::error(format!("Cannot send the bank: {e}")),
        }
    }

    /// [F9] Sends the library's Multi bank, paced like single banks.
    fn send_multi_bank(&mut self, policy: BadChecksumPolicy) {
        if !self.ports.output_connected() {
            self.notice = Notice::warning("No MIDI output connected: the Multi bank was not sent.");
            return;
        }
        match self.db.export_multi_bank(self.session.settings.device_id, policy) {
            Ok(export) if export.frames.is_empty() => {
                self.notice = Notice::warning("Nothing to send: the Multi bank is empty or every multi is flagged ⚠.");
            }
            Ok(export) => {
                let count = export.frames.len();
                self.paced.start(Instant::now(), count, Duration::from_millis(MULTI_BANK_SEND_GAP_MS));
                self.command(MidiCommand::SendBatch { messages: export.frames, gap_ms: MULTI_BANK_SEND_GAP_MS });
                let mut text = format!("Sending {count} multi(s) to the synth's Multi bank…");
                if export.skipped_flagged > 0 {
                    text.push_str(&format!(" {} flagged ⚠ multi(s) left out.", export.skipped_flagged));
                }
                self.notice = Notice::info(text);
            }
            Err(e) => self.notice = Notice::error(format!("Cannot send the Multi bank: {e}")),
        }
    }

    /// Offers to copy and migrate a v1 library found next to the exe / in the working directory (M3).
    fn legacy_prompt(&mut self, ctx: &egui::Context) {
        let Some(legacy) = self.legacy.clone() else { return };
        let LibraryLocation::File(target) = self.location.clone() else {
            self.legacy = None;
            return;
        };
        let mut accept = false;
        let mut dismiss = false;
        egui::Window::new("Existing library found").collapsible(false).resizable(false).show(ctx, |ui| {
            ui.label(format!("An older library was found at {}.", legacy.display()));
            ui.label(format!("The library is now stored at {}, and is empty.", target.display()));
            ui.label("Copy it there and upgrade it? The original file is left untouched, and a .v1.bak backup is kept.");
            ui.horizontal(|ui| {
                accept = ui.button("Copy and upgrade").clicked();
                dismiss = ui.button("Not now").clicked();
            });
        });
        if dismiss {
            self.legacy = None;
        }
        if !accept {
            return;
        }
        self.legacy = None;
        // Release the file handle on the target before it is replaced.
        let old = std::mem::replace(
            &mut self.db,
            PatchDatabase::open_in_memory().expect("an in-memory SQLite database can always be opened"),
        );
        drop(old);
        match adopt_legacy_library(&legacy, &target) {
            Ok(db) => {
                let backup = db.v1_backup().map(|p| format!(" Backup: {}.", p.display())).unwrap_or_default();
                self.db = db;
                self.notice = Notice::info(format!(
                    "Imported the old library. Its patches are flagged ⚠ until you re-check them.{backup}"
                ));
            }
            Err(e) => {
                let (db, _) = open_library_at(Ok(target));
                self.db = db;
                self.notice = Notice::error(format!("Could not adopt the old library: {e}"));
            }
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
            theme::tab(ui, &mut self.tab, Tab::Multi, "🎛 Multi"); // [multi]
            ui.separator();
            theme::lcd_frame()
                .show(ui, |ui| ui.label(theme::lcd_text(self.session.patch().name())))
                .response
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
                theme::watermark(ui, ui.available_rect_before_wrap()); // [V2] only in the empty gap
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
        let shortcut = self.comfort_shortcuts(ctx); // [comforts]
        let now_secs = ctx.input(|i| i.time);
        if let Some(action) = shortcut {
            self.comfort_action(action, now_secs);
        }

        TopBottomPanel::top("header").frame(theme::header_frame()).show(ctx, |ui| {
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
        let settings = self.session.settings;
        let multi_channel = self.multi.edited_part_channel(settings.part); // [multi] N9
        let target = self.audition.sync_target(settings.channel, settings.part, multi_channel);
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
        self.librarian.editor_comparing = comparing; // [E6]
        let mut multi_actions = Vec::new(); // [multi]
        CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Editor => {
                ui.add_enabled_ui(!comparing, |ui| self.editor.show(ui, self.session.patch(), &mut edits));
            }
            Tab::Modulation => {
                ui.add_enabled_ui(!comparing, |ui| modulation::show(ui, self.session.patch(), &mut edits));
            }
            Tab::Librarian => self.librarian.show(ui, &self.db, &mut self.session.received, &mut actions),
            // [multi]
            Tab::Multi => self.multi_view.show(ui, &self.db, &mut self.multi, &mut self.session.settings, &mut multi_actions),
        });
        self.librarian.show_dialogs(ctx, &self.db, self.session.patch(), &mut actions);
        self.legacy_prompt(ctx);
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
        // [multi] Multi tab actions
        for action in multi_actions {
            match action {
                MultiAction::Send(message) => self.send(message),
                MultiAction::OpenEditor => self.tab = Tab::Editor,
                MultiAction::Notice(notice) => self.notice = notice,
                MultiAction::SendBank { policy } => self.send_multi_bank(policy), // [F9]
                MultiAction::CancelBatch => self.handle_library_action(LibAction::CancelBatch),
            }
        }
        self.flush_paced();
        if let Some(wait) = self.paced.next_wakeup(Instant::now()) {
            ctx.request_repaint_after(wait.max(Duration::from_millis(20)));
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

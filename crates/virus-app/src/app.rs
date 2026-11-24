//! The eframe application: wires the MIDI worker, the session, the database and the panels.

use std::time::{Duration, Instant};

use eframe::{egui, App, Frame};
use egui::{CentralPanel, RichText, TopBottomPanel};
use virus_db::{BadChecksumPolicy, PatchDatabase};
use virus_midi::{MidiCommand, MidiEvent, MidiService, ReleaseTrigger, DUMP_GAP_MS};
use virus_syx::Patch;

use crate::audition::Audition;
use crate::comforts::{Comforts, Replace, ReplaceRequest};
use crate::dialogs::SharedFolders;
use crate::library::{slot_label, store_patch, EditorFlag, PendingStore};
use crate::session::{Activity, EditPart, Incoming, MidiSettings, PacedSend, Ports, Session, Submitted, Ticket, PORT_POLL_INTERVAL};
use crate::settings::{self, FolderPrefs, Loaded, Settings, SettingsStore, WindowPrefs};
use crate::storage::{adopt_legacy_library, open_library, open_library_at, pending_legacy_library, LibraryLocation};
use crate::theme::{self, ThemeKind};
use crate::synth_select::select_messages;
use crate::views::comforts::{ComfortAction, ComfortsView};
use crate::views::librarian::{LibAction, LibrarianView};
use crate::views::{editor::EditorView, midi_bar, modulation, Edit, Notice, DIM, WARN};
// [multi] Multi tab (F4/F5)
use crate::multi_session::MultiSession;
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
    /// A store waiting for the synth's edit-buffer dump ("Fetch from synth first"). It exists
    /// only once the request has gone out; its timeout starts then (CR-M6).
    pending_store: Option<PendingStore>,
    /// A "Fetch from synth first" store whose request still waits in `PacedSend`.
    store_waiting: Option<WaitingStore>,
    /// A bank send is running (from its start until the worker reports `BatchFinished`).
    batch_running: bool,
    /// [CR-H1] The edited patch came from a flagged ⚠ source; storing it keeps the flag.
    editor_flag: EditorFlag,
    /// [CR-M5] The legacy library the user answered "Not now" for (saved in settings).
    legacy_dismissed: Option<String>,
    /// The panel look, chosen from the header's 🎨 menu (saved in settings).
    theme: ThemeKind,
    /// Import/export folders shared by the Librarian and the Multi tab (saved in settings).
    folders: SharedFolders,
}

/// A store whose edit-buffer request is still queued in `PacedSend`.
#[derive(Debug, Clone, Copy)]
struct WaitingStore {
    bank_id: i64,
    program: u8,
    ticket: Ticket,
    since: Instant,
}

/// How long a store's fetch request may wait for a quiet line before the store is given up.
const STORE_REQUEST_WAIT: Duration = Duration::from_secs(10);

impl VirusApp {
    pub fn new(cc: &eframe::CreationContext<'_>, store: SettingsStore, loaded: Loaded) -> Self {
        let mut notice = Notice::info("Ready. Select the MIDI ports, or let auto-detect find the Virus.");
        let midi = match MidiService::spawn() {
            Ok(service) => {
                // Repaint as soon as MIDI arrives instead of polling every frame (U2).
                let ctx = cc.egui_ctx.clone();
                service.set_event_notifier(Box::new(move || ctx.request_repaint()));
                // Notice ports that vanish, return or are renamed without a manual rescan (R10).
                let _ = service.send_command(MidiCommand::SetPortPolling(Some(PORT_POLL_INTERVAL)));
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

        let legacy = pending_legacy_library(&db, &location, loaded.settings.ui.legacy_prompt_dismissed.as_deref());
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
            pending_store: None,
            store_waiting: None,
            batch_running: false,
            editor_flag: EditorFlag::default(),
            legacy_dismissed: None,
            theme: ThemeKind::default(),
            folders: SharedFolders::default(),
        };
        app.apply_settings(&loaded.settings);
        theme::apply(&cc.egui_ctx, app.theme);
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
        self.librarian.fetch_before_store = s.ui.fetch_before_store;
        self.legacy_dismissed.clone_from(&s.ui.legacy_prompt_dismissed);
        self.theme = ThemeKind::from_key(&s.ui.theme);
        if let (Some(name), Ok(banks)) = (&s.ui.bank, self.db.get_banks()) {
            self.librarian.set_bank_id(banks.iter().find(|b| &b.name == name).map(|b| b.id));
        }
        self.folders = SharedFolders {
            import: FolderPrefs::existing(&s.folders.import),
            export: FolderPrefs::existing(&s.folders.export),
        };
        self.folders.offer(&mut self.librarian.files, &mut self.multi_view.files);
        self.window = s.window;
    }

    /// Everything worth keeping between runs, as it is now.
    fn snapshot(&mut self) -> Settings {
        self.folders.sync(&mut self.librarian.files, &mut self.multi_view.files);
        let bank_id = self.librarian.bank_id();
        if self.bank_name.0 != bank_id {
            let name = bank_id.and_then(|id| self.db.get_banks().ok()?.into_iter().find(|b| b.id == id)).map(|b| b.name);
            self.bank_name = (bank_id, name);
        }
        let mut midi = match &self.pending_ports {
            Some(saved) => saved.clone(),
            None => settings::MidiPrefs {
                input: self.ports.wanted_input.clone(),
                output: self.ports.wanted_output.clone(),
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
                fetch_before_store: self.librarian.fetch_before_store,
                legacy_prompt_dismissed: self.legacy_dismissed.clone(),
                theme: self.theme.key().to_owned(),
            },
            folders: FolderPrefs {
                import: FolderPrefs::from_path(&self.folders.import),
                export: FolderPrefs::from_path(&self.folders.export),
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

    /// Sends the live messages that waited for a bank transfer to finish. A waiting store's
    /// request that goes out now starts its answer timeout.
    fn flush_paced(&mut self) {
        let now = Instant::now();
        for message in self.paced.drain_ready(now) {
            self.command(MidiCommand::Send(message));
        }
        if let Some(waiting) = self.store_waiting {
            if !self.paced.is_queued(waiting.ticket) {
                self.store_waiting = None;
                self.pending_store = Some(PendingStore::new(waiting.bank_id, waiting.program, now));
            }
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

    /// Header Panic: stops the audition and sends All Notes Off / All Sound Off on the synth
    /// channel and, in Multi mode, on every enabled part's channel.
    fn panic(&mut self) {
        let channels = self.multi.panic_channels(&self.session.settings);
        let messages = self.audition.panic_on(&channels);
        self.send_notes(messages);
        self.notice = Notice::info(format!(
            "Panic sent on channel(s) {}.",
            channels.iter().map(|c| (c + 1).to_string()).collect::<Vec<_>>().join(", ")
        ));
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
                // Every message out (edits, bank sends, audition notes) or in delays requests (G4).
                MidiEvent::Sent { .. } => {
                    self.leds.tx(now);
                    self.paced.note_activity(now);
                }
                MidiEvent::BatchFinished { sent, dropped } => {
                    self.paced.finish(now);
                    self.batch_running = false;
                    self.notice = if dropped == 0 {
                        Notice::info(format!("Bank send finished: {sent} sent."))
                    } else {
                        Notice::warning(format!("Bank send aborted after {sent} sent; {dropped} not sent."))
                    };
                }
                MidiEvent::Error(e) => self.notice = Notice::error(format!("MIDI: {e}")),
                incoming => {
                    self.leds.rx(now);
                    self.paced.note_activity(now);
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
                self.editor_flag.received(checksum_ok, was_dirty); // [CR-H1]
                self.complete_pending_store(checksum_ok);
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
                self.editor_flag.loaded(); // [CR-H1]
                self.load(patch, origin);
                if let Some(slot) = select {
                    self.select_on_synth(slot.bank_id, slot.program); // [E6]
                }
            }
            Replace::Init => {
                self.editor_flag.init(); // [CR-H1]
                self.load(Patch::init(), "init".to_owned());
            }
            Replace::Fetch => self.send(self.session.fetch_request()),
            Replace::SendToEditBuffer { patch, origin } => {
                self.editor_flag.loaded(); // [CR-H1]
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
            LibAction::Load { patch, origin, select, flagged } => {
                self.editor_flag.request(flagged); // [CR-H1]
                self.request_replace(Replace::Load { patch, origin, select });
            }
            LibAction::SendToEditBuffer { patch, origin, flagged } => {
                self.editor_flag.request(flagged); // [CR-H1]
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
                // A running batch ends with the worker's BatchFinished, which keeps the gap after
                // the last dump; with none running nothing else will release waiting messages.
                if !self.batch_running {
                    self.paced.cancel();
                }
                self.notice = Notice::info("Pending bank transfer cancelled.");
            }
            LibAction::Notice(notice) => self.notice = notice,
            LibAction::LooseMultis(dumps) => self.multi_view.add_loose(dumps), // [F9]
            LibAction::FetchThenStore { bank_id, program } => self.fetch_then_store(bank_id, program),
        }
    }

    /// Requests the synth's edit buffer for a store. The answer timeout starts only when the
    /// request actually goes out (`PacedSend` may hold it for a quiet line, CR-M6).
    fn fetch_then_store(&mut self, bank_id: i64, program: u8) {
        if !self.ports.output_connected() || self.batch_running {
            self.notice = Notice::warning("Cannot fetch from the synth now (no MIDI output, or a bank send is running): nothing was stored.");
            return;
        }
        if let Some(previous) = self.store_waiting.take() {
            self.paced.remove(previous.ticket);
        }
        let now = Instant::now();
        match self.paced.submit_tracked(self.session.fetch_request(), now) {
            Submitted::Now(request) => {
                self.pending_store = Some(PendingStore::new(bank_id, program, now));
                self.command(MidiCommand::Send(request));
            }
            Submitted::Queued(ticket) => {
                self.pending_store = None;
                self.store_waiting = Some(WaitingStore { bank_id, program, ticket, since: now });
            }
        }
        self.notice = Notice::info("Fetching the synth's edit buffer; it is stored when it arrives…");
    }

    /// Gives up on a store whose request never got a quiet line (and withdraws the request), or
    /// whose edit-buffer dump never came; nothing is stored then.
    fn expire_pending_store(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        if let Some(waiting) = self.store_waiting {
            let deadline = waiting.since + STORE_REQUEST_WAIT;
            if now >= deadline {
                self.paced.remove(waiting.ticket);
                self.store_waiting = None;
                self.notice = Notice::warning("The MIDI line never went quiet for the fetch request: nothing was stored.");
            } else {
                ctx.request_repaint_after(deadline - now);
            }
        }
        let Some(pending) = self.pending_store else { return };
        if pending.expired(now) {
            self.pending_store = None;
            self.notice = Notice::warning("The synth did not send its edit buffer: nothing was stored.");
        } else {
            ctx.request_repaint_after(pending.deadline() - now);
        }
    }

    /// Stores the just-adopted edit buffer in the slot chosen in the Store dialog.
    fn complete_pending_store(&mut self, checksum_ok: bool) {
        let Some(pending) = self.pending_store.take() else { return };
        if !checksum_ok {
            self.notice = Notice::warning("The synth's edit buffer arrived with a wrong checksum: nothing was stored.");
            return;
        }
        let patch = self.session.patch().clone();
        match store_patch(&self.db, pending.bank_id, pending.program, &patch, !self.editor_flag.flagged()) {
            Ok(_) => {
                let bank = self.db.get_banks().ok().and_then(|banks| banks.into_iter().find(|b| b.id == pending.bank_id));
                let label = slot_label(bank.as_ref(), pending.program);
                self.notice = Notice::info(format!("Stored '{}' (fetched from the synth) to {label}.", patch.name()));
                self.comforts.mark_stored(patch);
                self.origin = label;
            }
            Err(e) => self.notice = Notice::error(format!("Store failed: {e}")),
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
                self.paced.start(Instant::now(), count, Duration::from_millis(DUMP_GAP_MS));
                self.batch_running = true;
                self.command(MidiCommand::SendBatch { messages: export.frames, gap_ms: DUMP_GAP_MS });
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
                self.paced.start(Instant::now(), count, Duration::from_millis(DUMP_GAP_MS));
                self.batch_running = true;
                self.command(MidiCommand::SendBatch { messages: export.frames, gap_ms: DUMP_GAP_MS });
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
            ui.label(
                "Copy it there and upgrade it? The original file is left untouched, a .v1.bak backup is kept, \
                 and the current (empty) library is backed up as .pre-adopt.bak first.",
            );
            ui.horizontal(|ui| {
                accept = ui.button("Copy and upgrade").clicked();
                dismiss = ui.button("Not now").on_hover_text("Don't ask again for this file").clicked();
            });
        });
        if dismiss {
            self.legacy = None;
            self.legacy_dismissed = Some(legacy.to_string_lossy().into_owned()); // [CR-M5] saved in settings
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
            Ok(adopted) => {
                let backup = adopted.db.v1_backup().map(|p| format!(" Backup: {}.", p.display())).unwrap_or_default();
                let replaced = adopted
                    .pre_adopt_backup
                    .as_ref()
                    .map(|p| format!(" The previous library was saved as {}.", p.display()))
                    .unwrap_or_default();
                self.db = adopted.db;
                self.notice = Notice::info(format!(
                    "Imported the old library. Its patches are flagged ⚠ until you re-check them.{backup}{replaced}"
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
            ui.visuals_mut().widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, theme::palette().header_edge);
            theme::logo(ui).on_hover_text("Access Virus");
            ui.label(theme::app_name("Rack Editor & Librarian"));
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
                ui.menu_button("🎨", |ui| {
                    for kind in ThemeKind::ALL {
                        if ui.radio(self.theme == kind, kind.label()).clicked() {
                            self.theme = kind;
                            theme::apply(ui.ctx(), kind);
                            ui.close_menu();
                        }
                    }
                })
                .response
                .on_hover_text("Theme");
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
                if ui
                    .add_enabled(self.ports.output_connected(), egui::Button::new("🛑 Panic"))
                    .on_hover_text(
                        "Silence the synth: note offs, All Notes Off and All Sound Off on its channel (every enabled \
                         part's channel in Multi mode). It doesn't stop a running bank send: use ⏹ Cancel sending.",
                    )
                    .clicked()
                {
                    self.panic();
                }
            });
        });
        match &self.location {
            LibraryLocation::InMemory { reason } => {
                theme::lcd_frame().show(ui, |ui| {
                    ui.colored_label(theme::palette().lcd_alert, format!("⚠ The library is in memory only — nothing will be saved ({reason})."));
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
        self.batch_running = self.paced.batch_running(Instant::now());
        self.expire_pending_store(ctx);
        // "Fetch from synth first" needs the output, and no bank send in the way (CR-M6).
        self.librarian.output_connected = self.ports.output_connected();
        self.librarian.batch_running = self.batch_running;
        self.multi_view.batch_running = self.batch_running; // [CR-L11]
        self.librarian.editor_edited = self.comforts.is_edited(&self.session);
        self.librarian.editor_flagged = self.editor_flag.flagged(); // [CR-H1]
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
        self.multi_view.show_dialogs(ctx, &self.db, &mut self.multi, &mut multi_actions); // [CR-M4/H2/L2]
        self.folders.sync(&mut self.librarian.files, &mut self.multi_view.files);
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

//! Root eframe::App implementation for the Access Virus Editor & Librarian.

use eframe::{egui, App, Frame};
use egui::{Color32, RichText, TopBottomPanel, CentralPanel, Visuals};
use std::time::Duration;
use virus_db::{PatchDatabase, SoundCategory};
use virus_midi::engine::MidiService;
use virus_midi::events::{MidiCommand, MidiEvent};
use virus_syx::domain::{Page, PageA, PageB, Patch};

use crate::views::librarian_panel::LibrarianAction;
use crate::views::{LibrarianPanel, MatrixPanel, MidiBar, SynthPanel};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppTab {
    #[default]
    SynthEditor,
    ModulationMatrix,
    Librarian,
}

pub struct VirusApp {
    pub patch: Patch,
    pub midi_service: Option<MidiService>,
    pub db: PatchDatabase,
    pub midi_bar: MidiBar,
    pub librarian_panel: LibrarianPanel,
    pub active_tab: AppTab,
}

impl VirusApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Configure classic Access Virus dark theme
        let mut visuals = Visuals::dark();
        visuals.panel_fill = Color32::from_rgb(22, 24, 28);
        visuals.window_fill = Color32::from_rgb(26, 28, 34);
        visuals.extreme_bg_color = Color32::from_rgb(14, 15, 18);
        visuals.selection.bg_fill = Color32::from_rgb(200, 60, 40); // Classic Virus red
        visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(240, 160, 60));
        cc.egui_ctx.set_visuals(visuals);

        // Spawn background MIDI engine
        let midi_service = match MidiService::spawn() {
            Ok(service) => Some(service),
            Err(e) => {
                eprintln!("Warning: Failed to start MIDI service: {}", e);
                None
            }
        };

        // Initialize SQLite patch database (or in-memory fallback)
        let db = PatchDatabase::open("virus_library.db")
            .unwrap_or_else(|_| PatchDatabase::open_in_memory().expect("In-memory SQLite failed"));

        Self {
            patch: Patch::new("Init Patch"),
            midi_service,
            db,
            midi_bar: MidiBar::default(),
            librarian_panel: LibrarianPanel::default(),
            active_tab: AppTab::SynthEditor,
        }
    }

    /// Processes incoming events from the MIDI subsystem.
    fn poll_midi_events(&mut self) {
        if let Some(ref service) = self.midi_service {
            while let Some(event) = service.try_recv_event() {
                match event {
                    MidiEvent::PortListUpdated { inputs, outputs } => {
                        self.midi_bar.input_ports = inputs.clone();
                        self.midi_bar.output_ports = outputs.clone();

                        // Auto-detect and connect to "Virus" / "Access" ports if not yet connected
                        if self.midi_bar.selected_input_idx.is_none() {
                            if let Some((idx, _)) = inputs.iter().enumerate().find(|(_, name)| {
                                let l = name.to_lowercase();
                                l.contains("virus") || l.contains("access") || l.contains("osr1")
                            }) {
                                self.midi_bar.selected_input_idx = Some(idx);
                                let _ = service.send_command(MidiCommand::ConnectInput(idx));
                            }
                        }

                        if self.midi_bar.selected_output_idx.is_none() {
                            if let Some((idx, _)) = outputs.iter().enumerate().find(|(_, name)| {
                                let l = name.to_lowercase();
                                l.contains("virus") || l.contains("access") || l.contains("osr1")
                            }) {
                                self.midi_bar.selected_output_idx = Some(idx);
                                let _ = service.send_command(MidiCommand::ConnectOutput(idx));
                            }
                        }
                    }
                    MidiEvent::ConnectionStatus {
                        input_connected,
                        input_name,
                        output_connected,
                        output_name,
                    } => {
                        self.midi_bar.input_connected = input_connected;
                        self.midi_bar.output_connected = output_connected;

                        let in_str = input_name.as_deref().unwrap_or("None");
                        let out_str = output_name.as_deref().unwrap_or("None");
                        self.midi_bar.status_message = format!("In: {} | Out: {}", in_str, out_str);
                    }
                    MidiEvent::ParamReceived { page, param, value, .. } => {
                        self.midi_bar.trigger_rx();
                        match page {
                            Page::PageA => {
                                if (param as usize) < self.patch.page_a.raw.len() {
                                    self.patch.page_a.raw[param as usize] = value;
                                    self.patch.page_a = PageA::from_bytes(&self.patch.page_a.raw);
                                }
                            }
                            Page::PageB => {
                                if (param as usize) < self.patch.page_b.raw.len() {
                                    self.patch.page_b.raw[param as usize] = value;
                                    self.patch.page_b = PageB::from_bytes(&self.patch.page_b.raw);
                                }
                            }
                            _ => {}
                        }
                    }
                    MidiEvent::PatchReceived { patch, .. } => {
                        self.midi_bar.trigger_rx();
                        self.patch = *patch;
                        self.midi_bar.status_message = format!("Loaded Patch: {}", self.patch.name());
                    }
                    MidiEvent::Error(err) => {
                        self.midi_bar.status_message = format!("MIDI Error: {}", err);
                    }
                    _ => {}
                }
            }
        }
    }
}

impl App for VirusApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut Frame) {
        // Poll incoming MIDI queue
        self.poll_midi_events();

        let dt = ctx.input(|i| i.stable_dt.min(0.1));

        // --- Top Navigation Header Bar ---
        TopBottomPanel::top("top_header_panel").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("ACCESS VIRUS")
                        .strong()
                        .size(16.0)
                        .color(Color32::from_rgb(220, 70, 40)),
                );
                ui.label(
                    RichText::new("Rack / B / Classic Editor & Librarian")
                        .size(12.0)
                        .color(Color32::from_rgb(160, 165, 180)),
                );

                ui.separator();

                // Navigation Tabs
                if ui.selectable_label(self.active_tab == AppTab::SynthEditor, "🎹 Sound Engine").clicked() {
                    self.active_tab = AppTab::SynthEditor;
                }
                if ui.selectable_label(self.active_tab == AppTab::ModulationMatrix, "🔀 Mod Matrix & Arp").clicked() {
                    self.active_tab = AppTab::ModulationMatrix;
                }
                if ui.selectable_label(self.active_tab == AppTab::Librarian, "📚 Patch Librarian").clicked() {
                    self.active_tab = AppTab::Librarian;
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Transmit Active Patch to Hardware Single Edit Buffer
                    if ui.button("📤 Send to Synth Buffer").on_hover_text("Transmit complete 256-byte patch dump to synth edit buffer").clicked() {
                        if let Some(ref service) = self.midi_service {
                            let _ = service.send_command(MidiCommand::SendPatchDump {
                                device_id: self.midi_bar.device_id,
                                bank: 0x00,
                                prog: 0x40, // Single Edit Buffer
                                patch: Box::new(self.patch.clone()),
                            });
                            self.midi_bar.trigger_tx();
                        }
                    }

                    // Save to active bank
                    if ui.button("💾 Store Active to DB").on_hover_text("Save active patch to current bank in librarian").clicked() {
                        let bank_id = self.librarian_panel.selected_bank_id;
                        let _ = self.db.save_patch(
                            bank_id,
                            0,
                            &mut self.patch,
                            SoundCategory::Off,
                            SoundCategory::Off,
                            "",
                            false,
                        );
                        self.midi_bar.status_message = format!("Saved '{}' to Bank ID {}", self.patch.name(), bank_id);
                    }
                });
            });
            ui.add_space(4.0);
        });

        // --- Bottom MIDI Status Bar ---
        TopBottomPanel::bottom("bottom_midi_panel").show(ctx, |ui| {
            if let Some(cmd) = self.midi_bar.show(ui, dt) {
                if let Some(ref service) = self.midi_service {
                    let _ = service.send_command(cmd);
                }
            }
        });

        // --- Central Workspace View ---
        CentralPanel::default().show(ctx, |ui| {
            match self.active_tab {
                AppTab::SynthEditor => {
                    let changes = SynthPanel::show(ui, &mut self.patch);
                    if !changes.is_empty() {
                        if let Some(ref service) = self.midi_service {
                            for (page_code, param, val) in changes {
                                let page = if page_code == 0x71 { Page::PageB } else { Page::PageA };
                                let _ = service.send_command(MidiCommand::SendParam {
                                    device_id: self.midi_bar.device_id,
                                    page,
                                    part: 0x40, // Single Edit Buffer
                                    param,
                                    value: val,
                                });
                                self.midi_bar.trigger_tx();
                            }
                        }
                    }
                }
                AppTab::ModulationMatrix => {
                    let changes = MatrixPanel::show(ui, &mut self.patch);
                    if !changes.is_empty() {
                        if let Some(ref service) = self.midi_service {
                            for (_page_code, param, val) in changes {
                                let _ = service.send_command(MidiCommand::SendParam {
                                    device_id: self.midi_bar.device_id,
                                    page: Page::PageB,
                                    part: 0x40, // Single Edit Buffer
                                    param,
                                    value: val,
                                });
                                self.midi_bar.trigger_tx();
                            }
                        }
                    }
                }
                AppTab::Librarian => {
                    if let Some(action) = self.librarian_panel.show(ui, &self.db, &mut self.patch) {
                        match action {
                            LibrarianAction::LoadPatch(loaded) => {
                                self.patch = loaded;
                                self.midi_bar.status_message = format!("Loaded '{}' into editor", self.patch.name());
                            }
                            LibrarianAction::SendPatchToEditBuffer(patch_to_send) => {
                                if let Some(ref service) = self.midi_service {
                                    let _ = service.send_command(MidiCommand::SendPatchDump {
                                        device_id: self.midi_bar.device_id,
                                        bank: 0x00,
                                        prog: 0x40, // Single Edit Buffer
                                        patch: Box::new(patch_to_send),
                                    });
                                    self.midi_bar.trigger_tx();
                                    self.midi_bar.status_message = "Sent patch to synth edit buffer.".to_string();
                                }
                            }
                            LibrarianAction::StoreActiveToSlot(prog) => {
                                let bank_id = self.librarian_panel.selected_bank_id;
                                let _ = self.db.save_patch(
                                    bank_id,
                                    prog,
                                    &mut self.patch,
                                    SoundCategory::Off,
                                    SoundCategory::Off,
                                    "",
                                    false,
                                );
                                self.midi_bar.status_message = format!("Stored active patch to slot {:03}", prog);
                            }
                            LibrarianAction::StatusMessage(msg) => {
                                self.midi_bar.status_message = msg;
                            }
                        }
                    }
                }
            }
        });

        // Request continuous repaint for smooth UI controls and LED countdowns
        ctx.request_repaint_after(Duration::from_millis(30));
    }
}

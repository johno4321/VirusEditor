//! Bottom status bar for MIDI port selection, connection state, Device ID, and activity LEDs.

use egui::{pos2, vec2, Color32, ComboBox, Frame, Pos2, RichText, Stroke, Ui};
use virus_midi::events::MidiCommand;

pub struct MidiBar {
    pub input_ports: Vec<String>,
    pub output_ports: Vec<String>,
    pub selected_input_idx: Option<usize>,
    pub selected_output_idx: Option<usize>,
    pub input_connected: bool,
    pub output_connected: bool,
    pub device_id: u8,
    pub rx_active: f32, // Timer countdown for LED glow
    pub tx_active: f32, // Timer countdown for LED glow
    pub status_message: String,
}

impl Default for MidiBar {
    fn default() -> Self {
        Self {
            input_ports: Vec::new(),
            output_ports: Vec::new(),
            selected_input_idx: None,
            selected_output_idx: None,
            input_connected: false,
            output_connected: false,
            device_id: 0x00, // Device 1 (0x00)
            rx_active: 0.0,
            tx_active: 0.0,
            status_message: "Ready. Select MIDI ports.".to_string(),
        }
    }
}

impl MidiBar {
    /// Renders the MIDI bottom toolbar. Returns any command to be dispatched to the MIDI thread.
    pub fn show(&mut self, ui: &mut Ui, dt: f32) -> Option<MidiCommand> {
        let mut command = None;

        // Decrement LED glow timers
        self.rx_active = (self.rx_active - dt).max(0.0);
        self.tx_active = (self.tx_active - dt).max(0.0);

        Frame::canvas(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                // Refresh button
                if ui.button("⟳ Refresh").on_hover_text("Rescan MIDI ports").clicked() {
                    command = Some(MidiCommand::RefreshPorts);
                }

                ui.separator();

                // MIDI In Dropdown
                ui.label(RichText::new("MIDI In:").strong());
                let in_label = if let Some(idx) = self.selected_input_idx {
                    self.input_ports.get(idx).cloned().unwrap_or_else(|| "None".to_string())
                } else {
                    "None".to_string()
                };

                ComboBox::from_id_source("midi_in_combo")
                    .selected_text(in_label)
                    .show_ui(ui, |ui| {
                        for (idx, port) in self.input_ports.iter().enumerate() {
                            let mut is_sel = self.selected_input_idx == Some(idx);
                            if ui.selectable_value(&mut is_sel, true, port).clicked() {
                                self.selected_input_idx = Some(idx);
                                command = Some(MidiCommand::ConnectInput(idx));
                            }
                        }
                    });

                // MIDI Out Dropdown
                ui.label(RichText::new("MIDI Out:").strong());
                let out_label = if let Some(idx) = self.selected_output_idx {
                    self.output_ports.get(idx).cloned().unwrap_or_else(|| "None".to_string())
                } else {
                    "None".to_string()
                };

                ComboBox::from_id_source("midi_out_combo")
                    .selected_text(out_label)
                    .show_ui(ui, |ui| {
                        for (idx, port) in self.output_ports.iter().enumerate() {
                            let mut is_sel = self.selected_output_idx == Some(idx);
                            if ui.selectable_value(&mut is_sel, true, port).clicked() {
                                self.selected_output_idx = Some(idx);
                                command = Some(MidiCommand::ConnectOutput(idx));
                            }
                        }
                    });

                ui.separator();

                // Device ID (0..15 -> Device 1..16, 16 -> Omni)
                ui.label(RichText::new("Dev ID:").strong());
                let dev_text = if self.device_id == 0x10 {
                    "Omni (All)".to_string()
                } else {
                    format!("Device {}", self.device_id + 1)
                };
                ComboBox::from_id_source("dev_id_combo")
                    .selected_text(dev_text)
                    .show_ui(ui, |ui| {
                        for id in 0..=15 {
                            let mut is_sel = self.device_id == id;
                            if ui.selectable_value(&mut is_sel, true, format!("Device {}", id + 1)).clicked() {
                                self.device_id = id;
                            }
                        }
                        let mut is_omni = self.device_id == 0x10;
                        if ui.selectable_value(&mut is_omni, true, "Omni (0x10)").clicked() {
                            self.device_id = 0x10;
                        }
                    });

                ui.separator();

                // Request Edit Buffer Dump
                if ui.button("📥 Fetch Synth Buffer").on_hover_text("Request active Single Edit Buffer dump from hardware").clicked() {
                    command = Some(MidiCommand::RequestPatchDump {
                        device_id: self.device_id,
                        bank: 0x00,
                        prog: 0x40, // Single Edit Buffer
                    });
                    self.tx_active = 0.25;
                }

                ui.separator();

                // Activity LEDs
                draw_led(ui, "Rx", self.rx_active > 0.0, Color32::from_rgb(50, 220, 80));
                draw_led(ui, "Tx", self.tx_active > 0.0, Color32::from_rgb(255, 160, 40));

                ui.separator();

                // Status Message
                ui.label(
                    RichText::new(&self.status_message)
                        .size(11.0)
                        .color(Color32::from_rgb(160, 165, 180)),
                );
            });
        });

        command
    }

    pub fn trigger_rx(&mut self) {
        self.rx_active = 0.25;
    }

    pub fn trigger_tx(&mut self) {
        self.tx_active = 0.25;
    }
}

/// Helper to render an illuminated round LED indicator with text label.
fn draw_led(ui: &mut Ui, label: &str, active: bool, active_color: Color32) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(12.0, 12.0), egui::Sense::hover());
        let painter = ui.painter();
        let center = rect.center();

        let fill_color = if active {
            active_color
        } else {
            Color32::from_rgb(30, 35, 42)
        };
        let stroke_color = if active {
            Color32::WHITE
        } else {
            Color32::from_rgb(60, 65, 75)
        };

        painter.circle(center, 5.0, fill_color, Stroke::new(1.0, stroke_color));

        // Glow ring when illuminated
        if active {
            let glow_color = Color32::from_rgba_unmultiplied(active_color.r(), active_color.g(), active_color.b(), 60);
            painter.circle_filled(center, 8.0, glow_color);
        }

        ui.label(RichText::new(label).size(10.0).color(Color32::from_rgb(140, 145, 160)));
    });
}

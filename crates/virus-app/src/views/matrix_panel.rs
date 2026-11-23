//! 6-Slot Modulation Matrix and Arpeggiator editor (Page B).

use egui::{Color32, ComboBox, Frame, Grid, RichText, ScrollArea, Ui};
use virus_syx::domain::patch::Patch;
use virus_syx::domain::types::{ArpMode, ModDestination, ModSource};

use crate::widgets::knob::bipolar_knob;

pub struct MatrixPanel;

const ALL_SOURCES: [ModSource; 16] = [
    ModSource::Off,
    ModSource::Lfo1,
    ModSource::Lfo2,
    ModSource::FilterEnv,
    ModSource::AmpEnv,
    ModSource::Velocity,
    ModSource::Aftertouch,
    ModSource::ModWheel,
    ModSource::Breath,
    ModSource::Foot,
    ModSource::PitchBend,
    ModSource::MidiVolume,
    ModSource::Pan,
    ModSource::Expression,
    ModSource::Assign1,
    ModSource::Assign2,
];

const ALL_DESTINATIONS: [ModDestination; 17] = [
    ModDestination::Off,
    ModDestination::Pitch1,
    ModDestination::Pitch2,
    ModDestination::Pitch1And2,
    ModDestination::PulseWidth1,
    ModDestination::PulseWidth2,
    ModDestination::Osc2Detune,
    ModDestination::FmAmount,
    ModDestination::Cutoff1,
    ModDestination::Cutoff2,
    ModDestination::Cutoff1And2,
    ModDestination::Resonance1,
    ModDestination::Resonance2,
    ModDestination::FilterBalance,
    ModDestination::Pan,
    ModDestination::Lfo1Rate,
    ModDestination::Lfo2Rate,
];

impl MatrixPanel {
    /// Renders the 6-slot Modulation Matrix and Arpeggiator panel.
    /// Returns real-time parameter changes: `(0x71, param_number, value)`.
    pub fn show(ui: &mut Ui, patch: &mut Patch) -> Vec<(u8, u8, u8)> {
        let mut changes = Vec::new();

        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(8.0);

                // --- Arpeggiator Section ---
                Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading(RichText::new("Arpeggiator Engine").color(Color32::from_rgb(240, 200, 140)));
                    ui.add_space(4.0);

                    ui.horizontal_wrapped(|ui| {
                        // Mode
                        ui.vertical(|ui| {
                            ui.label(RichText::new("Mode").strong());
                            ComboBox::from_id_source("arp_mode")
                                .selected_text(format!("{:?}", patch.page_b.arp_mode))
                                .show_ui(ui, |ui| {
                                    for mode in [ArpMode::Off, ArpMode::Up, ArpMode::Down, ArpMode::UpDown, ArpMode::AsPlayed, ArpMode::Random, ArpMode::Chord] {
                                        if ui.selectable_value(&mut patch.page_b.arp_mode, mode, format!("{:?}", mode)).changed() {
                                            changes.push((0x71, 0, patch.page_b.arp_mode.to_raw()));
                                        }
                                    }
                                });
                        });

                        ui.add_space(16.0);

                        // Pattern selector (0..63)
                        ui.vertical(|ui| {
                            ui.label(RichText::new("Pattern").strong());
                            let mut pattern = patch.page_b.arp_pattern;
                            if ui.add(egui::DragValue::new(&mut pattern).clamp_range(0..=63)).changed() {
                                patch.page_b.arp_pattern = pattern;
                                changes.push((0x71, 1, pattern));
                            }
                        });

                        ui.add_space(16.0);

                        // Clock / Resolution (1/8, 1/16, etc.)
                        ui.vertical(|ui| {
                            ui.label(RichText::new("Clock").strong());
                            let mut clock = patch.page_b.arp_clock;
                            if ui.add(egui::DragValue::new(&mut clock).clamp_range(0..=127)).changed() {
                                patch.page_b.arp_clock = clock;
                                changes.push((0x71, 2, clock));
                            }
                        });

                        ui.add_space(16.0);

                        // Swing
                        if bipolar_knob(ui, &mut patch.page_b.arp_swing, "Swing").changed() {
                            changes.push((0x71, 4, patch.page_b.arp_swing.to_raw()));
                        }
                    });
                });

                ui.add_space(12.0);

                // --- 6-Slot Modulation Matrix ---
                Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading(RichText::new("6-Slot Modulation Matrix").color(Color32::from_rgb(240, 200, 140)));
                    ui.label(
                        RichText::new("Connect modulation sources to up to 2 distinct destinations per slot with bipolar attenuation.")
                            .size(11.0)
                            .color(Color32::from_rgb(140, 145, 160)),
                    );
                    ui.add_space(8.0);

                    Grid::new("mod_matrix_grid")
                        .striped(true)
                        .min_col_width(120.0)
                        .spacing([14.0, 10.0])
                        .show(ui, |ui| {
                            // Header
                            ui.label(RichText::new("Slot").strong());
                            ui.label(RichText::new("Source").strong());
                            ui.label(RichText::new("Amount (-64..+63)").strong());
                            ui.label(RichText::new("Destination 1").strong());
                            ui.label(RichText::new("Destination 2").strong());
                            ui.end_row();

                            for i in 0..6 {
                                let base_offset = (16 + (i * 4)) as u8;

                                // Slot label
                                ui.label(RichText::new(format!("#{}", i + 1)).strong().color(Color32::from_rgb(220, 70, 40)));

                                // Source dropdown
                                ComboBox::from_id_source(format!("mod_src_{}", i))
                                    .selected_text(format!("{:?}", patch.page_b.matrix[i].source))
                                    .show_ui(ui, |ui| {
                                        for src in ALL_SOURCES {
                                            if ui.selectable_value(&mut patch.page_b.matrix[i].source, src, format!("{:?}", src)).changed() {
                                                changes.push((0x71, base_offset, patch.page_b.matrix[i].source.to_raw()));
                                            }
                                        }
                                    });

                                // Amount knob
                                if bipolar_knob(ui, &mut patch.page_b.matrix[i].amount, "").changed() {
                                    changes.push((0x71, base_offset + 1, patch.page_b.matrix[i].amount.to_raw()));
                                }

                                // Destination 1
                                ComboBox::from_id_source(format!("mod_dest1_{}", i))
                                    .selected_text(format!("{:?}", patch.page_b.matrix[i].dest1))
                                    .show_ui(ui, |ui| {
                                        for dst in ALL_DESTINATIONS {
                                            if ui.selectable_value(&mut patch.page_b.matrix[i].dest1, dst, format!("{:?}", dst)).changed() {
                                                changes.push((0x71, base_offset + 2, patch.page_b.matrix[i].dest1.to_raw()));
                                            }
                                        }
                                    });

                                // Destination 2
                                ComboBox::from_id_source(format!("mod_dest2_{}", i))
                                    .selected_text(format!("{:?}", patch.page_b.matrix[i].dest2))
                                    .show_ui(ui, |ui| {
                                        for dst in ALL_DESTINATIONS {
                                            if ui.selectable_value(&mut patch.page_b.matrix[i].dest2, dst, format!("{:?}", dst)).changed() {
                                                changes.push((0x71, base_offset + 3, patch.page_b.matrix[i].dest2.to_raw()));
                                            }
                                        }
                                    });

                                ui.end_row();
                            }
                        });
                });
            });

        changes
    }
}

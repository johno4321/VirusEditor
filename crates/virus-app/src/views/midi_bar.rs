//! Bottom bar: MIDI ports by name, synth channel, device ID, edited part, Rx/Tx LEDs, status.

use std::time::Instant;

use crate::theme;
use egui::{vec2, Color32, ComboBox, RichText, Stroke, Ui};
use virus_midi::MidiCommand;
use virus_syx::DEVICE_ID_OMNI;

use super::{Notice, DANGER};
use crate::session::{Activity, EditPart, MidiSettings, Ports};

/// Draws the bar and returns the commands for the MIDI worker.
pub fn show(
    ui: &mut Ui,
    ports: &mut Ports,
    settings: &mut MidiSettings,
    leds: &Activity,
    notice: &Notice,
    midi_available: bool,
) -> Vec<MidiCommand> {
    let mut commands = Vec::new();
    ui.horizontal_wrapped(|ui| {
        ui.add_enabled_ui(midi_available, |ui| {
            if ui.button("⟳").on_hover_text("Rescan MIDI ports").clicked() {
                commands.push(MidiCommand::RefreshPorts);
            }
            if let Some(name) = port_combo(ui, "In", &ports.inputs, &ports.selected_input, &ports.connected_input) {
                commands.push(ports.select_input(name));
            }
            if let Some(name) = port_combo(ui, "Out", &ports.outputs, &ports.selected_output, &ports.connected_output) {
                commands.push(ports.select_output(name));
            }
        });
        ui.separator();

        ui.label("Ch");
        ComboBox::from_id_source("midi-channel")
            .width(40.0)
            .selected_text((settings.channel + 1).to_string())
            .show_ui(ui, |ui| {
                for channel in 0..16 {
                    ui.selectable_value(&mut settings.channel, channel, (channel + 1).to_string());
                }
            })
            .response
            .on_hover_text("The synth's MIDI channel. Incoming CC / Poly Pressure is accepted only on this channel.");

        ui.label("Device");
        let device_text = |id: u8| if id == DEVICE_ID_OMNI { "Omni".to_owned() } else { (id + 1).to_string() };
        ComboBox::from_id_source("device-id")
            .width(56.0)
            .selected_text(device_text(settings.device_id))
            .show_ui(ui, |ui| {
                for id in (0..16).chain([DEVICE_ID_OMNI]) {
                    ui.selectable_value(&mut settings.device_id, id, device_text(id));
                }
            })
            .response
            .on_hover_text("SysEx device ID (global C93). Default since OS 1.50: 1.");

        ui.label("Part");
        ComboBox::from_id_source("edit-part")
            .width(100.0)
            .selected_text(settings.part.label())
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut settings.part, EditPart::Single, EditPart::Single.label());
                for part in 0..16 {
                    ui.selectable_value(&mut settings.part, EditPart::Multi(part), EditPart::Multi(part).label());
                }
            })
            .response
            .on_hover_text("Edit buffer for edits, fetch and send: Single mode (0x40) or a Multi part.");
        ui.separator();

        let now = Instant::now();
        let led_on = theme::palette().led_on;
        led(ui, "Rx", leds.rx_lit(now), led_on);
        led(ui, "Tx", leds.tx_lit(now), led_on);
        ui.separator();
        ui.label(RichText::new(&notice.text).size(11.0).color(notice.colour()));
    });
    commands
}

/// Returns `Some(selection)` when the user picked a port (`Some(None)` = disconnect).
fn port_combo(
    ui: &mut Ui,
    label: &str,
    available: &[String],
    selected: &Option<String>,
    connected: &Option<String>,
) -> Option<Option<String>> {
    let mut picked = None;
    let ok = selected.is_some() && selected == connected;
    let text = match selected {
        Some(name) if ok => RichText::new(name),
        Some(name) => RichText::new(format!("{name} (not connected)")).color(DANGER),
        None => RichText::new("None"),
    };
    ui.label(RichText::new(format!("MIDI {label}")).strong());
    ComboBox::from_id_source(("midi-port", label))
        .width(170.0)
        .selected_text(text)
        .show_ui(ui, |ui| {
            if ui.selectable_label(selected.is_none(), "None").clicked() {
                picked = Some(None);
            }
            for name in available {
                if ui
                    .selectable_label(selected.as_ref() == Some(name), name)
                    .clicked()
                {
                    picked = Some(Some(name.clone()));
                }
            }
        });
    picked
}

fn led(ui: &mut Ui, label: &str, lit: bool, colour: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(12.0, 12.0), egui::Sense::hover());
    let painter = ui.painter();
    if lit {
        painter.circle_filled(rect.center(), 8.0, colour.gamma_multiply(0.25));
    }
    let p = theme::palette();
    painter.circle(
        rect.center(),
        5.0,
        if lit { colour } else { p.led_off },
        Stroke::new(1.0_f32, if lit { Color32::WHITE } else { p.edge }),
    );
    ui.label(RichText::new(label).size(10.0).color(theme::DIM));
}

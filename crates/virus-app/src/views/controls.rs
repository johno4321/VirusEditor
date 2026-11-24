//! One control per parameter kind, generated from the `virus-syx` parameter table (P7).

use egui::{ComboBox, DragValue, RichText, Ui};
use virus_syx::{format_value, Page, ParamDef, Patch, ValueKind};

use super::{Edit, DIM, WARN};
use crate::widgets::Knob;

const CONTROL_WIDTH: f32 = 112.0;

/// `A40` / `B123`.
pub fn param_id(def: &ParamDef) -> String {
    let page = match def.page {
        Page::PageA => 'A',
        Page::PageB => 'B',
        Page::PageC => 'C',
    };
    format!("{page}{}", def.number)
}

/// A compact label for knobs; the full name is in the tooltip.
pub fn short_label(def: &ParamDef) -> String {
    const PREFIXES: &[&str] = &[
        "Filter Env ", "Amp Env ", "Osc1 ", "Osc2 ", "Lfo1 ", "Lfo2 ", "Lfo3 ", "Unison ", "Chorus ",
        "Phaser ", "Arp ", "Analog Boost ",
    ];
    const ABBREVIATIONS: &[(&str, &str)] = &[
        ("Suboscillator", "Sub"),
        ("Ringmodulator", "Ring Mod"),
        ("Mainvolume", "Main Vol"),
        (" Volume", " Vol"),
        ("Resonance", "Reso"),
        ("Filter", "Flt"),
        (" Amount", " Amt"),
        ("Velocity", "Vel"),
        ("Keyfollow", "KeyFlw"),
        ("Keytrigger", "KeyTrig"),
        ("Panorama", "Pan"),
        ("Pulsewidth", "PW"),
        ("PulseWidth", "PW"),
        ("Semitone", "Semi"),
        ("Sustain Time", "Sus Time"),
        ("Intensity", "Int"),
        ("Fade-In Time", "Fade-In"),
    ];
    let mut label = PREFIXES
        .iter()
        .find_map(|p| def.name.strip_prefix(p))
        .unwrap_or(def.name)
        .to_owned();
    for (long, short) in ABBREVIATIONS {
        label = label.replace(long, short);
    }
    if label.chars().count() > 12 {
        label = label.chars().take(11).collect::<String>() + "…";
    }
    label
}

fn tooltip(def: &ParamDef, raw: u8) -> String {
    let mut text = format!("{} ({})\nValue: {}", def.name, param_id(def), format_value(def, raw));
    if raw < def.min || raw > def.max {
        text.push_str(&format!("\nOutside the documented range {}..{}; kept as is.", def.min, def.max));
    }
    if def.flags.multi_single {
        text.push_str("\nIn Multi mode this is stored with the Multi.");
    }
    text
}

/// Draws the control for `def` and records an [`Edit`] when the user changes it.
pub fn param_control(ui: &mut Ui, patch: &Patch, def: &'static ParamDef, edits: &mut Vec<Edit>) {
    let raw = patch.get(def.page, def.number);
    let mut value = raw;
    let response = match def.kind {
        ValueKind::Unipolar => ui.add(Knob::unipolar(&mut value, &short_label(def)).range(def.min, def.max)),
        ValueKind::Bipolar => ui.add(Knob::bipolar(&mut value, &short_label(def))),
        ValueKind::Toggle => labelled(ui, def, |ui| {
            let mut on = value != 0;
            let text = if value > 1 { format!("#{value}") } else { String::new() };
            if ui.checkbox(&mut on, text).changed() {
                value = u8::from(on);
            }
        }),
        ValueKind::Choice(texts) => labelled(ui, def, |ui| choice_combo(ui, def, texts, &mut value)),
        ValueKind::Note => labelled(ui, def, |ui| {
            ui.add(DragValue::new(&mut value).clamp_range(0..=127).custom_formatter(|n, _| format_value(def, n as u8)));
        }),
        ValueKind::Raw | ValueKind::NameChar => labelled(ui, def, |ui| {
            ui.add(DragValue::new(&mut value).clamp_range(0..=127));
        }),
    };
    response.on_hover_text(tooltip(def, raw));
    if value != raw {
        edits.push(Edit::Param { page: def.page, number: def.number, value });
    }
}

fn labelled(ui: &mut Ui, def: &ParamDef, add: impl FnOnce(&mut Ui)) -> egui::Response {
    ui.vertical(|ui| {
        ui.set_width(CONTROL_WIDTH);
        ui.label(RichText::new(short_label(def)).size(10.5));
        add(ui);
    })
    .response
}

/// Combo box over a value list. Values beyond the list show as `#n` and stay selected (P3).
pub fn choice_combo(ui: &mut Ui, def: &ParamDef, texts: &[&str], value: &mut u8) {
    let selected = format_value(def, *value);
    let known = (*value as usize) < texts.len();
    ComboBox::from_id_source(("param", def.page, def.number))
        .width(CONTROL_WIDTH - 8.0)
        .selected_text(if known { RichText::new(selected) } else { RichText::new(selected).color(WARN) })
        .show_ui(ui, |ui| {
            for (raw, text) in texts.iter().enumerate() {
                ui.selectable_value(value, raw as u8, *text);
            }
            if !known {
                let current = *value;
                ui.selectable_value(value, current, RichText::new(format!("#{current} (undocumented)")).color(WARN));
            }
        });
}

/// Raw destination number with the unverified name greyed next to it (Q1).
pub fn destination_control(
    ui: &mut Ui,
    patch: &Patch,
    def: &'static ParamDef,
    hints: &[&str],
    edits: &mut Vec<Edit>,
) {
    let raw = patch.get(def.page, def.number);
    let mut value = raw;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Dest").size(10.5));
        ui.add(DragValue::new(&mut value).clamp_range(0..=127))
            .on_hover_text(format!("{} ({}): raw value {raw}", def.name, param_id(def)));
        let hint = hints.get(value as usize).map_or_else(|| "no name".to_owned(), |name| format!("≈ {name}?"));
        ui.label(RichText::new(hint).italics().color(DIM))
            .on_hover_text("Unverified: the printed list order may not be the raw value mapping (open question Q1).");
    });
    if value != raw {
        edits.push(Edit::Param { page: def.page, number: def.number, value });
    }
}

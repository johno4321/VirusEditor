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
    let (response, changed) = match def.kind {
        ValueKind::Unipolar => {
            let r = ui.add(Knob::unipolar(&mut value, &short_label(def)).range(def.min, def.max));
            let changed = r.changed();
            (r, changed)
        }
        ValueKind::Bipolar => {
            let r = ui.add(Knob::bipolar(&mut value, &short_label(def)));
            let changed = r.changed();
            (r, changed)
        }
        ValueKind::Toggle => labelled(ui, def, |ui| {
            let mut on = value != 0;
            let text = if value > 1 { format!("#{value}") } else { String::new() };
            let changed = ui.checkbox(&mut on, text).changed();
            if changed {
                value = u8::from(on);
            }
            changed
        }),
        ValueKind::Choice(texts) => labelled(ui, def, |ui| {
            let before = value;
            choice_combo(ui, def, texts, &mut value);
            value != before
        }),
        ValueKind::Note => labelled(ui, def, |ui| {
            let r = ui.add(DragValue::new(&mut value).clamp_range(0..=127).custom_formatter(|n, _| format_value(def, n as u8)));
            user_changed(&r)
        }),
        ValueKind::Raw | ValueKind::NameChar => labelled(ui, def, |ui| {
            let r = ui.add(DragValue::new(&mut value).clamp_range(0..=127));
            user_changed(&r)
        }),
    };
    response.on_hover_text(tooltip(def, raw));
    // Out-of-range stored values are rewritten by DragValue's clamp without any interaction;
    // `user_changed` keeps those from becoming edits (M1).
    if changed && value != raw {
        edits.push(Edit::Param { page: def.page, number: def.number, value });
    }
}

/// A `DragValue` marks itself changed when `clamp_range` rewrites an out-of-range stored value
/// (e.g. a byte > 127 from an imported patch), with no interaction at all (M1). Only count a
/// change while the user is dragging, typing or committing.
fn user_changed(response: &egui::Response) -> bool {
    response.changed() && (response.dragged() || response.has_focus() || response.lost_focus() || response.clicked())
}

fn labelled(ui: &mut Ui, def: &ParamDef, add: impl FnOnce(&mut Ui) -> bool) -> (egui::Response, bool) {
    let mut changed = false;
    let response = ui
        .vertical(|ui| {
            ui.set_width(CONTROL_WIDTH);
            ui.label(RichText::new(short_label(def)).size(10.5));
            changed = add(ui);
        })
        .response;
    (response, changed)
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
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Dest").size(10.5));
        let response = ui.add(DragValue::new(&mut value).clamp_range(0..=127));
        changed = user_changed(&response);
        response.on_hover_text(format!("{} ({}): raw value {raw}", def.name, param_id(def)));
        let hint = hints.get(value as usize).map_or_else(|| "no name".to_owned(), |name| format!("≈ {name}?"));
        ui.label(RichText::new(hint).italics().color(DIM))
            .on_hover_text("Unverified: the printed list order may not be the raw value mapping (open question Q1).");
    });
    if changed && value != raw {
        edits.push(Edit::Param { page: def.page, number: def.number, value });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use virus_syx::SINGLE_PARAMS;

    /// Renders every control for a patch holding `value` in every documented byte, twice,
    /// without any user input, and returns the edits that came out.
    fn render_all(value: impl Fn(&ParamDef) -> u8) -> Vec<Edit> {
        let mut bytes = *Patch::init().as_bytes();
        for def in SINGLE_PARAMS {
            let offset = if def.page == Page::PageB { 128 } else { 0 };
            bytes[offset + def.number as usize] = value(def);
        }
        let patch = Patch::from_bytes(&bytes);
        let ctx = egui::Context::default();
        let mut edits = Vec::new();
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    for def in SINGLE_PARAMS.iter().filter(|d| d.page != Page::PageC) {
                        param_control(ui, &patch, def, &mut edits);
                        destination_control(ui, &patch, def, &[], &mut edits);
                    }
                });
            });
        }
        edits
    }

    #[test]
    fn untouched_controls_never_emit_edits() {
        assert!(render_all(|_| 200).is_empty(), "bytes above 127 must not be rewritten by clamping");
        assert!(render_all(|d| d.max.saturating_add(1)).is_empty(), "values past the documented max");
        assert!(render_all(|d| d.min).is_empty());
        assert!(render_all(|d| d.max).is_empty());
        assert!(render_all(|_| 127).is_empty());
    }
}

//! Assign 1/2/3, LFO1/LFO2 assign and the Definable knobs (B62–B82, P4).
//!
//! Destination numbers are edited raw; the printed name lists are shown only as greyed,
//! unverified hints (open question Q1).

use egui::{RichText, ScrollArea, Ui};
use virus_syx::params::{ASSIGN_LAYOUT, LFO_ASSIGN_LAYOUT};
use virus_syx::{
    single_param, Page, ParamDef, Patch, UNVERIFIED_ASSIGN_DESTINATION_NAMES,
    UNVERIFIED_DEFINABLE_DESTINATION_NAMES,
};

use super::controls::{destination_control, param_control};
use super::{Edit, DIM};

const DEFINABLES: [u8; 2] = [62, 63];

fn def(number: u8) -> &'static ParamDef {
    single_param(Page::PageB, number).expect("Assign/Definable rows are in the parameter table")
}

/// Every parameter this panel edits.
#[cfg(test)]
pub fn shown_params() -> Vec<(Page, u8)> {
    let assign = ASSIGN_LAYOUT.iter().flat_map(|slot| {
        std::iter::once(slot.source).chain(slot.targets.iter().flat_map(|&(d, a)| [d, a]))
    });
    let lfo = LFO_ASSIGN_LAYOUT.iter().flat_map(|&(d, a)| [d, a]);
    assign
        .chain(lfo)
        .chain(DEFINABLES)
        .map(|n| (Page::PageB, n))
        .collect()
}

pub fn show(ui: &mut Ui, patch: &Patch, edits: &mut Vec<Edit>) {
    ui.label(
        RichText::new(
            "Destination names are unverified (open question Q1): the printed list order may not match the \
             raw values the synth uses. The raw number is what is stored and sent.",
        )
        .italics()
        .color(DIM),
    );
    ui.add_space(4.0);
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (index, slot) in ASSIGN_LAYOUT.iter().enumerate() {
                    group(ui, &format!("Assign {}", index + 1), |ui| {
                        param_control(ui, patch, def(slot.source), edits);
                        for &(destination, amount) in slot.targets {
                            target(
                                ui,
                                patch,
                                destination,
                                amount,
                                UNVERIFIED_ASSIGN_DESTINATION_NAMES,
                                edits,
                            );
                        }
                    });
                }
                for (index, &(destination, amount)) in LFO_ASSIGN_LAYOUT.iter().enumerate() {
                    group(ui, &format!("LFO {} Assign", index + 1), |ui| {
                        target(
                            ui,
                            patch,
                            destination,
                            amount,
                            UNVERIFIED_ASSIGN_DESTINATION_NAMES,
                            edits,
                        );
                    });
                }
                group(ui, "Definable Knobs (Single)", |ui| {
                    for number in DEFINABLES {
                        ui.label(RichText::new(def(number).name).size(10.5));
                        destination_control(
                            ui,
                            patch,
                            def(number),
                            UNVERIFIED_DEFINABLE_DESTINATION_NAMES,
                            edits,
                        );
                    }
                });
            });
        });
}

fn group(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    crate::theme::section(ui, title, Some(250.0), add);
}

fn target(
    ui: &mut Ui,
    patch: &Patch,
    destination: u8,
    amount: u8,
    hints: &[&str],
    edits: &mut Vec<Edit>,
) {
    ui.horizontal(|ui| {
        param_control(ui, patch, def(amount), edits);
        ui.vertical(|ui| {
            ui.add_space(14.0);
            destination_control(ui, patch, def(destination), hints, edits);
        });
    });
}

//! The text summary `virus fetch` prints for a patch.

use virus_syx::params::{ASSIGN_LAYOUT, LFO_ASSIGN_LAYOUT};
use virus_syx::{
    single_param, Page, ParamClass, Patch, ValueKind, SINGLE_PARAMS,
    UNVERIFIED_ASSIGN_DESTINATION_NAMES, UNVERIFIED_DEFINABLE_DESTINATION_NAMES,
};

use crate::address::ParamAddress;

/// Page B destination parameters whose raw values have only unverified names (Q1).
fn destination_hint(address: ParamAddress, raw: u8) -> Option<&'static str> {
    if address.page != Page::PageB {
        return None;
    }
    let n = address.number;
    let is_assign = ASSIGN_LAYOUT
        .iter()
        .any(|l| l.targets.iter().any(|&(d, _)| d == n))
        || LFO_ASSIGN_LAYOUT.iter().any(|&(d, _)| d == n);
    let list = if is_assign {
        UNVERIFIED_ASSIGN_DESTINATION_NAMES
    } else if n == 62 || n == 63 {
        UNVERIFIED_DEFINABLE_DESTINATION_NAMES
    } else {
        return None;
    };
    list.get(raw as usize).copied()
}

/// Name, categories, sound version, every documented sound parameter (grouped by section)
/// and the non-zero undocumented bytes.
pub fn patch_summary(patch: &Patch) -> String {
    let mut out = format!(
        "Name:          '{}'\nCategories:    {} / {}\nSound version: {}{}\n",
        patch.name(),
        patch.category1(),
        patch.category2(),
        patch.sound_version(),
        if patch.sound_version() == 6 {
            " (OS 2.00)"
        } else {
            ""
        }
    );
    let mut section = "";
    for def in SINGLE_PARAMS {
        if def.class != ParamClass::Sound || matches!(def.kind, ValueKind::NameChar) {
            continue;
        }
        if def.section != section {
            section = def.section;
            out.push_str(&format!("\n[{section}]\n"));
        }
        let address = ParamAddress {
            page: def.page,
            number: def.number,
        };
        let raw = patch.get(def.page, def.number);
        let hint = destination_hint(address, raw)
            .map_or(String::new(), |h| format!("  (unverified: {h})"));
        out.push_str(&format!(
            "  {:<5} {:<26} {}{hint}\n",
            address.to_string(),
            def.name,
            address.describe_value(raw)
        ));
    }
    let undocumented: Vec<String> = (0..256usize)
        .filter_map(|i| {
            let address = if i < 128 {
                ParamAddress::a(i as u8)
            } else {
                ParamAddress::b((i - 128) as u8)
            };
            let raw = patch.as_bytes()[i];
            (single_param(address.page, address.number).is_none() && raw != 0)
                .then(|| format!("{address}={raw}"))
        })
        .collect();
    out.push_str(&format!(
        "\nUndocumented non-zero bytes (preserved): {}\n",
        if undocumented.is_empty() {
            "none".to_owned()
        } else {
            undocumented.join(" ")
        }
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_lists_header_params_hints_and_undocumented_bytes() {
        let mut bytes = *Patch::init().as_bytes();
        bytes[128 + 65] = 1; // Assign1 Destination
        bytes[92] = 9; // undocumented A92
        let text = patch_summary(&Patch::from_bytes(&bytes));
        assert!(text.contains("Name:          'Init'"));
        assert!(text.contains("Sound version: 6 (OS 2.00)"));
        assert!(text.contains("A40   Cutoff"));
        assert!(text.contains("(unverified: PatchVol)"));
        assert!(text.contains("A92=9"));
        assert!(
            !text.contains("Modulation Wheel"),
            "performance controllers are not listed"
        );
    }
}

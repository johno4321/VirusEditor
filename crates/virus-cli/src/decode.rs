//! Human-readable decoding of incoming MIDI for `virus monitor`.

use virus_midi::MidiEvent;
use virus_syx::{parse_sysex, single_param, Page, ParamClass, Patch, SysExMessage};

use crate::address::ParamAddress;
use crate::messages::hex;
use crate::settings::{bank_label, Target};

/// One line describing `event`, or `None` for transport bookkeeping (`Sent`, port lists).
pub fn describe(event: &MidiEvent, target: &Target) -> Option<String> {
    Some(match event {
        MidiEvent::SysEx(frame) => describe_sysex(frame, target),
        MidiEvent::ControlChange {
            channel,
            controller,
            value,
        } => format!(
            "CC   ch{:<2} #{controller:<3} = {value:<3} {}",
            channel + 1,
            channel_mapping(*channel, Page::PageA, *controller, *value, target)
        ),
        MidiEvent::PolyPressure {
            channel,
            note,
            pressure,
        } => format!(
            "PolyP ch{:<2} #{note:<3} = {pressure:<3} {}",
            channel + 1,
            channel_mapping(*channel, Page::PageB, *note, *pressure, target)
        ),
        MidiEvent::ProgramChange { channel, program } => {
            format!("Program change ch{} → program {program}", channel + 1)
        }
        MidiEvent::ConnectionStatus { input, output } => format!(
            "Connected: in = {}, out = {}",
            input.as_deref().unwrap_or("-"),
            output.as_deref().unwrap_or("-")
        ),
        MidiEvent::Error(e) => format!("ERROR: {e}"),
        MidiEvent::Sent { .. }
        | MidiEvent::BatchFinished { .. }
        | MidiEvent::PortListUpdated { .. } => return None,
    })
}

/// What a CC (Page A) / Poly Pressure (Page B) would do on the configured channel (SPEC §1.4).
fn channel_mapping(channel: u8, page: Page, number: u8, value: u8, target: &Target) -> String {
    let address = ParamAddress { page, number };
    let mapped = match single_param(page, number) {
        Some(def) if def.class == ParamClass::Sound => {
            format!(
                "→ {address} {} = {}",
                def.name,
                address.describe_value(value)
            )
        }
        Some(def) if def.class == ParamClass::Performance => {
            format!(
                "→ {address} {} (performance controller, not stored)",
                def.name
            )
        }
        Some(def) => format!("→ {address} {} (internal, ignored)", def.name),
        None => format!("→ {address} (undocumented)"),
    };
    if channel == target.channel {
        mapped
    } else {
        format!(
            "(other channel; the editor listens on ch{}) {mapped}",
            target.channel + 1
        )
    }
}

fn describe_sysex(frame: &[u8], target: &Target) -> String {
    let message = match parse_sysex(frame) {
        Ok(message) => message,
        Err(e) => {
            let shown = &frame[..frame.len().min(24)];
            return format!(
                "SysEx {} bytes, not a Virus message ({e}): {}{}",
                frame.len(),
                hex(shown),
                if frame.len() > shown.len() {
                    " …"
                } else {
                    ""
                }
            );
        }
    };
    let device = message.device_id();
    let prefix = if target.device.accepts(device) {
        format!("SysEx dev 0x{device:02X}")
    } else {
        format!(
            "SysEx dev 0x{device:02X} (NOT our device {})",
            target.device
        )
    };
    let body = match message {
        SysExMessage::SingleDump {
            bank,
            program,
            data,
            checksum_ok,
            ..
        } => {
            let patch = Patch::from_bytes(&data);
            let place = if bank == 0 {
                format!("edit buffer part 0x{program:02X}")
            } else {
                format!("bank {} program {program}", bank_label(bank))
            };
            format!(
                "Single dump {place}: '{}' [{} / {}] checksum {}",
                patch.name(),
                patch.category1(),
                patch.category2(),
                if checksum_ok { "ok" } else { "WRONG" }
            )
        }
        SysExMessage::MultiDump {
            bank,
            program,
            data,
            checksum_ok,
            ..
        } => {
            let name: String = data[4..14]
                .iter()
                .map(|&b| {
                    if (32..=126).contains(&b) {
                        b as char
                    } else {
                        '?'
                    }
                })
                .collect();
            format!(
                "Multi dump bb=0x{bank:02X} mm={program}: '{}' checksum {}",
                name.trim_end(),
                if checksum_ok { "ok" } else { "WRONG" }
            )
        }
        SysExMessage::ParamChange {
            page: Page::PageC,
            part,
            param,
            value,
            ..
        } => {
            format!("Param C{param} = {value} (part 0x{part:02X}; multi/global)")
        }
        SysExMessage::ParamChange {
            page,
            part,
            param,
            value,
            ..
        } => {
            let address = ParamAddress {
                page,
                number: param,
            };
            let part_note = if part == target.part.raw() {
                String::new()
            } else {
                " — not our part".to_owned()
            };
            format!(
                "Param {address} {} = {} (part 0x{part:02X}{part_note})",
                address.name(),
                address.describe_value(value)
            )
        }
        SysExMessage::Other {
            command, payload, ..
        } => format!(
            "Undocumented command 0x{command:02X}, {} payload bytes{}",
            payload.len(),
            if command == 0x12 {
                " (probably a Global dump, SPEC §3.6)"
            } else {
                ""
            }
        ),
        request => format!("{request:?}"),
    };
    format!("{prefix} {body}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::DeviceId;
    use virus_syx::{encode_param_change, encode_program_dump};

    fn target() -> Target {
        Target::default()
    }

    #[test]
    fn cc_maps_to_page_a_names_on_the_configured_channel() {
        let line = describe(
            &MidiEvent::ControlChange {
                channel: 0,
                controller: 40,
                value: 95,
            },
            &target(),
        )
        .unwrap();
        assert!(line.contains("→ A40 Cutoff = 95"), "{line}");
        let line = describe(
            &MidiEvent::ControlChange {
                channel: 0,
                controller: 1,
                value: 3,
            },
            &target(),
        )
        .unwrap();
        assert!(line.contains("performance controller"), "{line}");
        let line = describe(
            &MidiEvent::ControlChange {
                channel: 4,
                controller: 10,
                value: 74,
            },
            &target(),
        )
        .unwrap();
        assert!(
            line.contains("other channel") && line.contains("Panorama = 74 (+10)"),
            "{line}"
        );
    }

    #[test]
    fn poly_pressure_maps_to_page_b() {
        let line = describe(
            &MidiEvent::PolyPressure {
                channel: 0,
                note: 64,
                pressure: 3,
            },
            &target(),
        )
        .unwrap();
        assert!(line.contains("B64 Assign1 Source = 3 (ModWheel)"), "{line}");
    }

    #[test]
    fn sysex_is_decoded_with_names_and_device_checks() {
        let line = describe(
            &MidiEvent::SysEx(encode_param_change(0, Page::PageA, 0x40, 40, 95)),
            &target(),
        )
        .unwrap();
        assert!(line.contains("Param A40 Cutoff = 95 (part 0x40)"), "{line}");
        let line = describe(
            &MidiEvent::SysEx(encode_param_change(5, Page::PageB, 0x02, 65, 7)),
            &target(),
        )
        .unwrap();
        assert!(
            line.contains("NOT our device") && line.contains("not our part"),
            "{line}"
        );
        let mut patch = Patch::init();
        patch.set_name("Probe");
        let dump = encode_program_dump(0x10, 2, 9, patch.as_bytes());
        let line = describe(
            &MidiEvent::SysEx(dump),
            &Target {
                device: DeviceId(3),
                ..target()
            },
        )
        .unwrap();
        assert!(
            line.contains("bank B program 9: 'Probe'") && line.contains("checksum ok"),
            "{line}"
        );
        let line = describe(&MidiEvent::SysEx(vec![0xF0, 0x41, 0x10, 0xF7]), &target()).unwrap();
        assert!(line.contains("not a Virus message"), "{line}");
        assert_eq!(describe(&MidiEvent::Sent { bytes: 3 }, &target()), None);
    }
}

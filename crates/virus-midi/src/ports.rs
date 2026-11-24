//! MIDI port enumeration and Access Virus port discovery.

use midir::{MidiInput, MidiOutput};

/// Names of all currently available MIDI input ports.
pub fn list_input_ports() -> Result<Vec<String>, String> {
    let midi_in = MidiInput::new("virus-in-scanner").map_err(|e| e.to_string())?;
    Ok(midi_in
        .ports()
        .iter()
        .enumerate()
        .map(|(i, p)| {
            midi_in
                .port_name(p)
                .unwrap_or_else(|_| format!("Input Port {i}"))
        })
        .collect())
}

/// Names of all currently available MIDI output ports.
pub fn list_output_ports() -> Result<Vec<String>, String> {
    let midi_out = MidiOutput::new("virus-out-scanner").map_err(|e| e.to_string())?;
    Ok(midi_out
        .ports()
        .iter()
        .enumerate()
        .map(|(i, p)| {
            midi_out
                .port_name(p)
                .unwrap_or_else(|_| format!("Output Port {i}"))
        })
        .collect())
}

/// Port names that look like an Access Virus connection (case-insensitive
/// "virus", "access" or "osr"), in their original order.
pub fn find_virus_ports(names: &[String]) -> Vec<String> {
    names
        .iter()
        .filter(|name| {
            let lower = name.to_lowercase();
            ["virus", "access", "osr"].iter().any(|k| lower.contains(k))
        })
        .cloned()
        .collect()
}

/// Ports that are never a Virus (Windows' built-in synth).
const NEVER_VIRUS: &[&str] = &["microsoft gs wavetable"];

/// Ports that could be a real device: everything except Windows' built-in synth.
pub fn real_ports(names: &[String]) -> Vec<String> {
    names
        .iter()
        .filter(|n| !NEVER_VIRUS.iter().any(|x| n.to_lowercase().contains(x)))
        .cloned()
        .collect()
}

/// The port to use when nothing was chosen: the first Virus-looking port, else the only
/// real port.
pub fn auto_pick_port(names: &[String]) -> Option<String> {
    if let Some(virus) = find_virus_ports(names).into_iter().next() {
        return Some(virus);
    }
    match real_ports(names).as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    }
}

/// A port name without the "<digits>- " prefix Windows adds when the USB socket changes
/// (e.g. "2- mioXC MIDI OUT"), lowercased for comparison.
pub fn base_port_name(name: &str) -> String {
    let lower = name.trim().to_lowercase();
    match lower.split_once("- ") {
        Some((digits, rest)) if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) => {
            rest.trim_start().to_owned()
        }
        _ => lower,
    }
}

/// The available port for the wanted name: the exact name, else a port with the same base
/// name (a renamed port).
pub fn find_port_or_renamed(wanted: &str, available: &[String]) -> Option<String> {
    if let Some(exact) = available.iter().find(|n| *n == wanted) {
        return Some(exact.clone());
    }
    let base = base_port_name(wanted);
    available.iter().find(|n| base_port_name(n) == base).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn base_name_drops_the_windows_prefix() {
        assert_eq!(base_port_name("2- mioXC MIDI OUT"), "mioxc midi out");
        assert_eq!(base_port_name("12- MIOXC MIDI OUT"), "mioxc midi out");
        assert_eq!(base_port_name("mioXC MIDI OUT"), "mioxc midi out");
        assert_eq!(base_port_name("A- Port"), "a- port");
    }

    #[test]
    fn renamed_port_is_found_by_base_name() {
        let available = names(&["Other", "2- mioXC MIDI OUT"]);
        assert_eq!(find_port_or_renamed("mioXC MIDI OUT", &available), Some("2- mioXC MIDI OUT".into()));
        assert_eq!(find_port_or_renamed("3- mioxc midi out", &available), Some("2- mioXC MIDI OUT".into()));
        assert_eq!(find_port_or_renamed("mioXC MIDI IN", &available), None);
        let both = names(&["2- mioXC MIDI OUT", "mioXC MIDI OUT"]);
        assert_eq!(find_port_or_renamed("mioXC MIDI OUT", &both), Some("mioXC MIDI OUT".into()));
    }

    #[test]
    fn auto_pick_prefers_virus_then_the_only_real_port() {
        assert_eq!(auto_pick_port(&names(&["GS Synth", "Virus A", "Virus B"])), Some("Virus A".into()));
        assert_eq!(auto_pick_port(&names(&["Microsoft GS Wavetable Synth", "mioXC MIDI OUT"])), Some("mioXC MIDI OUT".into()));
        assert_eq!(auto_pick_port(&names(&["mioXC MIDI OUT", "loopMIDI"])), None);
        assert_eq!(auto_pick_port(&names(&["Microsoft GS Wavetable Synth"])), None);
    }

    #[test]
    fn finds_virus_ports_case_insensitively() {
        let names: Vec<String> = [
            "Microsoft GS Wavetable Synth",
            "VIRUS Rack",
            "2- Access USB",
            "OSR1 MIDI",
            "loopMIDI Port",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            find_virus_ports(&names),
            vec!["VIRUS Rack", "2- Access USB", "OSR1 MIDI"]
        );
        assert!(find_virus_ports(&[]).is_empty());
    }
}

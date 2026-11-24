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

#[cfg(test)]
mod tests {
    use super::*;

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

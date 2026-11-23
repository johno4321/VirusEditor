//! Hardware MIDI port enumeration and discovery for Access Virus units.

use midir::{MidiInput, MidiOutput};

/// Port descriptor holding name and port index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortInfo {
    pub index: usize,
    pub name: String,
}

/// Enumerates all currently available MIDI Input port names.
pub fn list_input_ports() -> Result<Vec<PortInfo>, String> {
    let midi_in = MidiInput::new("virus-in-scanner").map_err(|e| e.to_string())?;
    let mut ports = Vec::new();
    for (i, p) in midi_in.ports().iter().enumerate() {
        let name = midi_in.port_name(p).unwrap_or_else(|_| format!("Input Port {}", i));
        ports.push(PortInfo { index: i, name });
    }
    Ok(ports)
}

/// Enumerates all currently available MIDI Output port names.
pub fn list_output_ports() -> Result<Vec<PortInfo>, String> {
    let midi_out = MidiOutput::new("virus-out-scanner").map_err(|e| e.to_string())?;
    let mut ports = Vec::new();
    for (i, p) in midi_out.ports().iter().enumerate() {
        let name = midi_out.port_name(p).unwrap_or_else(|_| format!("Output Port {}", i));
        ports.push(PortInfo { index: i, name });
    }
    Ok(ports)
}

/// Helper that inspects port names to find candidate Access Virus connections.
pub fn find_virus_port_indices(ports: &[PortInfo]) -> Vec<usize> {
    ports
        .iter()
        .filter(|p| {
            let lower = p.name.to_lowercase();
            lower.contains("virus") || lower.contains("access") || lower.contains("osr1")
        })
        .map(|p| p.index)
        .collect()
}

//! Low-latency MIDI I/O engine, SysEx chunk buffering, and port discovery for the Access Virus.

pub mod events;
pub mod ports;
pub mod stream;
pub mod engine;

pub use events::{MidiCommand, MidiEvent};
pub use ports::{find_virus_port_indices, list_input_ports, list_output_ports, PortInfo};
pub use stream::SysExStreamAssembler;
pub use engine::MidiService;

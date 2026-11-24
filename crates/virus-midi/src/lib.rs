//! Dumb, reliable MIDI transport for the Access Virus editor.
//!
//! The crate moves raw bytes: it parses incoming MIDI into channel events and SysEx
//! frames and sends outgoing messages, but does not interpret Virus parameters.

pub mod batch;
pub mod engine;
pub mod events;
pub mod notes;
pub mod parser;
pub mod ports;

pub use batch::BatchQueue;
pub use engine::MidiService;
pub use events::{MidiCommand, MidiEvent};
pub use notes::{NoteEvent, NoteTracker, Phrase, PhraseNote, PhrasePlayer, ReleaseTrigger};
pub use parser::{MidiInputParser, MAX_SYSEX_LEN, SYSEX_END, SYSEX_START};
pub use ports::{find_virus_ports, list_input_ports, list_output_ports};

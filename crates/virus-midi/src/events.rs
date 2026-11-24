//! Commands sent to the MIDI worker and events it emits.

/// Commands sent from the UI thread to the background MIDI worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MidiCommand {
    /// Re-enumerates ports and emits [`MidiEvent::PortListUpdated`].
    RefreshPorts,
    /// Connects the input port with this exact name, replacing any current input.
    ConnectInput(String),
    /// Connects the output port with this exact name, replacing any current output.
    ConnectOutput(String),
    DisconnectInput,
    DisconnectOutput,
    /// Disconnects both input and output.
    Disconnect,
    /// Sends one complete MIDI message (channel message or SysEx frame) immediately.
    Send(Vec<u8>),
    /// Queues messages to be sent one by one with `gap_ms` between them.
    /// A batch sent while another is pending is appended to it.
    SendBatch {
        messages: Vec<Vec<u8>>,
        gap_ms: u64,
    },
    /// Drops all messages of pending batches that have not been sent yet.
    CancelBatch,
}

/// Events emitted by the MIDI worker and input callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MidiEvent {
    PortListUpdated {
        inputs: Vec<String>,
        outputs: Vec<String>,
    },
    /// Names of the currently connected ports (`None` = not connected).
    ConnectionStatus {
        input: Option<String>,
        output: Option<String>,
    },
    /// `channel` is 0..=15.
    ControlChange {
        channel: u8,
        controller: u8,
        value: u8,
    },
    PolyPressure {
        channel: u8,
        note: u8,
        pressure: u8,
    },
    ProgramChange {
        channel: u8,
        program: u8,
    },
    /// A complete `F0 .. F7` frame, real-time bytes removed, otherwise unparsed.
    SysEx(Vec<u8>),
    /// A message of `bytes` length was handed to the output port successfully.
    Sent {
        bytes: usize,
    },
    Error(String),
}

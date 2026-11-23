//! Asynchronous MIDI commands and event types for thread-safe UI communication.

use virus_syx::domain::{Page, Patch};

/// High-level commands sent from the UI thread to the background MIDI service.
#[derive(Debug, Clone)]
pub enum MidiCommand {
    /// Connects to the MIDI Input port at the specified index.
    ConnectInput(usize),
    /// Connects to the MIDI Output port at the specified index.
    ConnectOutput(usize),
    /// Refreshes the list of available MIDI input and output ports.
    RefreshPorts,
    /// Disconnects active input and output ports.
    Disconnect,
    /// Live sound/global parameter tweak.
    SendParam {
        device_id: u8,
        page: Page,
        part: u8,
        param: u8,
        value: u8,
    },
    /// Sends a complete Single Program patch dump (256 bytes) to the synth.
    SendPatchDump {
        device_id: u8,
        bank: u8,
        prog: u8,
        patch: Box<Patch>,
    },
    /// Requests the synth to send a Single Program dump.
    RequestPatchDump {
        device_id: u8,
        bank: u8,
        prog: u8,
    },
    /// Requests a Single Bank dump (Banks A..D).
    RequestBankDump {
        device_id: u8,
        bank: u8,
    },
    /// Requests a Total RAM dump from the synth.
    RequestTotalDump {
        device_id: u8,
    },
    /// Standard MIDI Control Change (for Page A parameters).
    SendControlChange {
        channel: u8,
        controller: u8,
        value: u8,
    },
    /// Standard MIDI Polyphonic Pressure (for Page B parameters).
    SendPolyPressure {
        channel: u8,
        note: u8,
        pressure: u8,
    },
    /// Sends a raw arbitrary SysEx frame.
    SendRawSysEx(Vec<u8>),
}

/// Events emitted by the background MIDI thread to the UI thread.
#[derive(Debug, Clone)]
pub enum MidiEvent {
    /// List of available input and output port names.
    PortListUpdated {
        inputs: Vec<String>,
        outputs: Vec<String>,
    },
    /// Connection state changed.
    ConnectionStatus {
        input_connected: bool,
        input_name: Option<String>,
        output_connected: bool,
        output_name: Option<String>,
    },
    /// Received a parameter change from the synth.
    ParamReceived {
        page: Page,
        part: u8,
        param: u8,
        value: u8,
    },
    /// Received a full Single Program patch dump from the synth.
    PatchReceived {
        bank: u8,
        prog: u8,
        patch: Box<Patch>,
    },
    /// Raw SysEx message received (unrecognized or non-standard).
    RawSysExReceived(Vec<u8>),
    /// Informational warning or error from the MIDI subsystem.
    Error(String),
}

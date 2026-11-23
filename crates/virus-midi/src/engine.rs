//! Background MIDI worker engine running isolated from the UI thread.

use crate::events::{MidiCommand, MidiEvent};
use crate::ports::{list_input_ports, list_output_ports};
use crate::stream::SysExStreamAssembler;
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use virus_syx::domain::{
    encode_dump_request, encode_param_change, encode_program_dump, encode_single_bank_request,
    encode_total_request, parse_sysex, Patch, SysExMessage,
};

/// High-level MIDI management service providing thread-safe channels for UI communication.
pub struct MidiService {
    tx_cmd: Sender<MidiCommand>,
    rx_evt: Receiver<MidiEvent>,
}

impl MidiService {
    /// Spawns the background MIDI engine thread and returns the active service handle.
    pub fn spawn() -> Result<Self, String> {
        let (tx_cmd, rx_cmd) = unbounded::<MidiCommand>();
        let (tx_evt, rx_evt) = unbounded::<MidiEvent>();

        thread::Builder::new()
            .name("virus-midi-worker".into())
            .spawn(move || {
                run_midi_worker(rx_cmd, tx_evt);
            })
            .map_err(|e| format!("Failed to spawn MIDI thread: {}", e))?;

        Ok(Self { tx_cmd, rx_evt })
    }

    /// Sends a command to the background MIDI thread.
    pub fn send_command(&self, cmd: MidiCommand) -> Result<(), String> {
        self.tx_cmd.send(cmd).map_err(|e| e.to_string())
    }

    /// Non-blocking check for incoming events from the MIDI subsystem.
    pub fn try_recv_event(&self) -> Option<MidiEvent> {
        self.rx_evt.try_recv().ok()
    }

    /// Returns a reference to the event receiver (for use with event-driven loops).
    pub fn event_receiver(&self) -> &Receiver<MidiEvent> {
        &self.rx_evt
    }
}

fn run_midi_worker(rx_cmd: Receiver<MidiCommand>, tx_evt: Sender<MidiEvent>) {
    let mut in_conn: Option<MidiInputConnection<()>> = None;
    let mut out_conn: Option<MidiOutputConnection> = None;
    let mut cur_in_name: Option<String> = None;
    let mut cur_out_name: Option<String> = None;

    // Send initial port list
    notify_port_list(&tx_evt);

    while let Ok(cmd) = rx_cmd.recv() {
        match cmd {
            MidiCommand::RefreshPorts => {
                notify_port_list(&tx_evt);
            }
            MidiCommand::ConnectInput(idx) => {
                // Drop existing connection
                in_conn = None;
                cur_in_name = None;

                match MidiInput::new("virus-midi-in") {
                    Ok(midi_in) => {
                        let ports = midi_in.ports();
                        if let Some(port) = ports.get(idx) {
                            let port_name = midi_in.port_name(port).unwrap_or_else(|_| "Unknown Input".into());
                            let tx_cb = tx_evt.clone();
                            let assembler = Arc::new(Mutex::new(SysExStreamAssembler::new()));

                            let conn_res = midi_in.connect(
                                port,
                                "virus-input-callback",
                                move |_timestamp, data, _state| {
                                    handle_incoming_midi_bytes(data, &assembler, &tx_cb);
                                },
                                (),
                            );

                            match conn_res {
                                Ok(conn) => {
                                    in_conn = Some(conn);
                                    cur_in_name = Some(port_name);
                                }
                                Err(e) => {
                                    let _ = tx_evt.send(MidiEvent::Error(format!("Input connect error: {}", e)));
                                }
                            }
                        } else {
                            let _ = tx_evt.send(MidiEvent::Error("Invalid input port index".into()));
                        }
                    }
                    Err(e) => {
                        let _ = tx_evt.send(MidiEvent::Error(format!("MidiInput init error: {}", e)));
                    }
                }

                send_connection_status(&tx_evt, &cur_in_name, &cur_out_name);
            }
            MidiCommand::ConnectOutput(idx) => {
                out_conn = None;
                cur_out_name = None;

                match MidiOutput::new("virus-midi-out") {
                    Ok(midi_out) => {
                        let ports = midi_out.ports();
                        if let Some(port) = ports.get(idx) {
                            let port_name = midi_out.port_name(port).unwrap_or_else(|_| "Unknown Output".into());
                            match midi_out.connect(port, "virus-output-port") {
                                Ok(conn) => {
                                    out_conn = Some(conn);
                                    cur_out_name = Some(port_name);
                                }
                                Err(e) => {
                                    let _ = tx_evt.send(MidiEvent::Error(format!("Output connect error: {}", e)));
                                }
                            }
                        } else {
                            let _ = tx_evt.send(MidiEvent::Error("Invalid output port index".into()));
                        }
                    }
                    Err(e) => {
                        let _ = tx_evt.send(MidiEvent::Error(format!("MidiOutput init error: {}", e)));
                    }
                }

                send_connection_status(&tx_evt, &cur_in_name, &cur_out_name);
            }
            MidiCommand::Disconnect => {
                in_conn = None;
                out_conn = None;
                cur_in_name = None;
                cur_out_name = None;
                send_connection_status(&tx_evt, &cur_in_name, &cur_out_name);
            }
            MidiCommand::SendParam { device_id, page, part, param, value } => {
                if let Some(ref mut out) = out_conn {
                    let msg = encode_param_change(device_id, page, part, param, value);
                    let _ = out.send(&msg);
                }
            }
            MidiCommand::SendPatchDump { device_id, bank, prog, patch } => {
                if let Some(ref mut out) = out_conn {
                    let raw = patch.to_bytes();
                    let msg = encode_program_dump(device_id, bank, prog, &raw);
                    let _ = out.send(&msg);
                }
            }
            MidiCommand::RequestPatchDump { device_id, bank, prog } => {
                if let Some(ref mut out) = out_conn {
                    let msg = encode_dump_request(device_id, bank, prog);
                    let _ = out.send(&msg);
                }
            }
            MidiCommand::RequestBankDump { device_id, bank } => {
                if let Some(ref mut out) = out_conn {
                    let msg = encode_single_bank_request(device_id, bank);
                    let _ = out.send(&msg);
                }
            }
            MidiCommand::RequestTotalDump { device_id } => {
                if let Some(ref mut out) = out_conn {
                    let msg = encode_total_request(device_id);
                    let _ = out.send(&msg);
                }
            }
            MidiCommand::SendControlChange { channel, controller, value } => {
                if let Some(ref mut out) = out_conn {
                    let status = 0xB0 | (channel & 0x0F);
                    let msg = [status, controller & 0x7F, value & 0x7F];
                    let _ = out.send(&msg);
                }
            }
            MidiCommand::SendPolyPressure { channel, note, pressure } => {
                if let Some(ref mut out) = out_conn {
                    let status = 0xA0 | (channel & 0x0F);
                    let msg = [status, note & 0x7F, pressure & 0x7F];
                    let _ = out.send(&msg);
                }
            }
            MidiCommand::SendRawSysEx(raw) => {
                if let Some(ref mut out) = out_conn {
                    let _ = out.send(&raw);
                }
            }
        }
    }
}

fn handle_incoming_midi_bytes(
    data: &[u8],
    assembler_arc: &Arc<Mutex<SysExStreamAssembler>>,
    tx: &Sender<MidiEvent>,
) {
    // 1. Check for standard 3-byte Channel messages (CC or PolyPressure)
    if data.len() == 3 {
        let status = data[0] & 0xF0;
        if status == 0xB0 {
            // Control Change (Page A parameter)
            let param = data[1];
            let value = data[2];
            let _ = tx.send(MidiEvent::ParamReceived {
                page: virus_syx::domain::Page::PageA,
                part: 0x40,
                param,
                value,
            });
            return;
        } else if status == 0xA0 {
            // PolyPressure (Page B parameter)
            let param = data[1];
            let value = data[2];
            let _ = tx.send(MidiEvent::ParamReceived {
                page: virus_syx::domain::Page::PageB,
                part: 0x40,
                param,
                value,
            });
            return;
        }
    }

    // 2. Feed into SysEx stream assembler
    if let Ok(mut asm) = assembler_arc.lock() {
        let completed = asm.feed(data);
        for frame in completed {
            match parse_sysex(&frame) {
                Ok(SysExMessage::SingleProgramDump { bank, prog, data, .. }) => {
                    let patch = Patch::from_bytes(&data);
                    let _ = tx.send(MidiEvent::PatchReceived {
                        bank,
                        prog,
                        patch: Box::new(patch),
                    });
                }
                Ok(SysExMessage::ParamChange { page, part, param, value, .. }) => {
                    let _ = tx.send(MidiEvent::ParamReceived { page, part, param, value });
                }
                Ok(_) => {
                    let _ = tx.send(MidiEvent::RawSysExReceived(frame));
                }
                Err(_) => {
                    let _ = tx.send(MidiEvent::RawSysExReceived(frame));
                }
            }
        }
    }
}

fn notify_port_list(tx: &Sender<MidiEvent>) {
    let in_ports = list_input_ports().unwrap_or_default().into_iter().map(|p| p.name).collect();
    let out_ports = list_output_ports().unwrap_or_default().into_iter().map(|p| p.name).collect();
    let _ = tx.send(MidiEvent::PortListUpdated { inputs: in_ports, outputs: out_ports });
}

fn send_connection_status(
    tx: &Sender<MidiEvent>,
    cur_in: &Option<String>,
    cur_out: &Option<String>,
) {
    let _ = tx.send(MidiEvent::ConnectionStatus {
        input_connected: cur_in.is_some(),
        input_name: cur_in.clone(),
        output_connected: cur_out.is_some(),
        output_name: cur_out.clone(),
    });
}

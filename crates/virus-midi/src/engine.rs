//! Background MIDI worker thread. The midir-specific code is kept here and kept thin.

use crate::batch::BatchQueue;
use crate::events::{MidiCommand, MidiEvent};
use crate::parser::MidiInputParser;
use crate::ports::{list_input_ports, list_output_ports};
use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

type Notifier = Box<dyn Fn() + Send + Sync>;

/// Delivers events to the UI channel and wakes the UI via the optional notifier.
#[derive(Clone)]
struct EventSink {
    tx: Sender<MidiEvent>,
    notifier: Arc<RwLock<Option<Notifier>>>,
}

impl EventSink {
    fn emit(&self, event: MidiEvent) {
        if self.tx.send(event).is_err() {
            return;
        }
        if let Ok(guard) = self.notifier.read() {
            if let Some(notify) = guard.as_ref() {
                notify();
            }
        }
    }

    fn error(&self, message: impl Into<String>) {
        self.emit(MidiEvent::Error(message.into()));
    }
}

/// Handle to the background MIDI worker. Dropping it stops the worker.
pub struct MidiService {
    tx_cmd: Sender<MidiCommand>,
    rx_evt: Receiver<MidiEvent>,
    notifier: Arc<RwLock<Option<Notifier>>>,
}

impl MidiService {
    /// Spawns the worker thread. It immediately emits a `PortListUpdated` event.
    pub fn spawn() -> Result<Self, String> {
        let (tx_cmd, rx_cmd) = unbounded();
        let (tx_evt, rx_evt) = unbounded();
        let notifier = Arc::new(RwLock::new(None));
        let sink = EventSink {
            tx: tx_evt,
            notifier: Arc::clone(&notifier),
        };

        thread::Builder::new()
            .name("virus-midi-worker".into())
            .spawn(move || Worker::new(sink).run(&rx_cmd))
            .map_err(|e| format!("Failed to spawn MIDI thread: {e}"))?;

        Ok(Self {
            tx_cmd,
            rx_evt,
            notifier,
        })
    }

    pub fn send_command(&self, cmd: MidiCommand) -> Result<(), String> {
        self.tx_cmd.send(cmd).map_err(|e| e.to_string())
    }

    /// Non-blocking poll for the next event.
    pub fn try_recv_event(&self) -> Option<MidiEvent> {
        self.rx_evt.try_recv().ok()
    }

    pub fn event_receiver(&self) -> &Receiver<MidiEvent> {
        &self.rx_evt
    }

    /// Registers a callback invoked (on the MIDI threads) after every emitted event,
    /// e.g. to call `egui::Context::request_repaint`. Replaces any previous notifier.
    /// The callback must not call back into `set_event_notifier`.
    pub fn set_event_notifier(&self, notifier: Box<dyn Fn() + Send + Sync>) {
        if let Ok(mut guard) = self.notifier.write() {
            *guard = Some(notifier);
        }
    }
}

struct Worker {
    sink: EventSink,
    input: Option<(MidiInputConnection<MidiInputParser>, String)>,
    output: Option<(MidiOutputConnection, String)>,
    batch: BatchQueue,
}

impl Worker {
    fn new(sink: EventSink) -> Self {
        Self {
            sink,
            input: None,
            output: None,
            batch: BatchQueue::new(),
        }
    }

    fn run(mut self, rx_cmd: &Receiver<MidiCommand>) {
        self.refresh_ports();
        loop {
            let cmd = match self.batch.time_until_next(Instant::now()) {
                Some(wait) => match rx_cmd.recv_timeout(wait) {
                    Ok(cmd) => Some(cmd),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                },
                None => match rx_cmd.recv() {
                    Ok(cmd) => Some(cmd),
                    Err(_) => break,
                },
            };
            if let Some(cmd) = cmd {
                self.handle(cmd);
            }
            if let Some(message) = self.batch.pop_due(Instant::now()) {
                if !self.send(&message) {
                    let dropped = self.batch.clear();
                    if dropped > 0 {
                        self.sink
                            .error(format!("Batch aborted: {dropped} message(s) not sent"));
                    }
                }
            }
        }
    }

    fn handle(&mut self, cmd: MidiCommand) {
        match cmd {
            MidiCommand::RefreshPorts => self.refresh_ports(),
            MidiCommand::ConnectInput(name) => {
                // Close first: some backends cannot open the same port twice.
                self.input = None;
                match connect_input(&name, self.sink.clone()) {
                    Ok(conn) => self.input = Some((conn, name)),
                    Err(e) => self.sink.error(e),
                }
                self.emit_status();
            }
            MidiCommand::ConnectOutput(name) => {
                self.output = None;
                match connect_output(&name) {
                    Ok(conn) => self.output = Some((conn, name)),
                    Err(e) => self.sink.error(e),
                }
                self.emit_status();
            }
            MidiCommand::DisconnectInput => {
                self.input = None;
                self.emit_status();
            }
            MidiCommand::DisconnectOutput => {
                self.output = None;
                self.emit_status();
            }
            MidiCommand::Disconnect => {
                self.input = None;
                self.output = None;
                self.emit_status();
            }
            MidiCommand::Send(message) => {
                self.send(&message);
            }
            MidiCommand::SendBatch { messages, gap_ms } => {
                self.batch.push(messages, Duration::from_millis(gap_ms));
            }
            MidiCommand::CancelBatch => {
                self.batch.clear();
            }
        }
    }

    /// Sends one message, reporting the outcome as `Sent` or `Error`. Returns success.
    fn send(&mut self, message: &[u8]) -> bool {
        let result = match self.output.as_mut() {
            None => Err("Cannot send: no MIDI output connected".to_string()),
            Some(_) if message.is_empty() => Err("Cannot send an empty MIDI message".to_string()),
            Some((conn, name)) => conn
                .send(message)
                .map_err(|e| format!("Send to '{name}' failed: {e}")),
        };
        match result {
            Ok(()) => {
                self.sink.emit(MidiEvent::Sent {
                    bytes: message.len(),
                });
                true
            }
            Err(e) => {
                self.sink.error(e);
                false
            }
        }
    }

    fn refresh_ports(&self) {
        match (list_input_ports(), list_output_ports()) {
            (Ok(inputs), Ok(outputs)) => self
                .sink
                .emit(MidiEvent::PortListUpdated { inputs, outputs }),
            (Err(e), _) | (_, Err(e)) => self.sink.error(format!("Port enumeration failed: {e}")),
        }
    }

    fn emit_status(&self) {
        self.sink.emit(MidiEvent::ConnectionStatus {
            input: self.input.as_ref().map(|(_, n)| n.clone()),
            output: self.output.as_ref().map(|(_, n)| n.clone()),
        });
    }
}

fn connect_input(
    name: &str,
    sink: EventSink,
) -> Result<MidiInputConnection<MidiInputParser>, String> {
    let mut midi_in =
        MidiInput::new("virus-midi-in").map_err(|e| format!("MIDI input init failed: {e}"))?;
    // Clock and active sensing would only wake the callback for nothing; SysEx must pass.
    midi_in.ignore(Ignore::TimeAndActiveSense);
    let port = midi_in
        .ports()
        .into_iter()
        .find(|p| midi_in.port_name(p).is_ok_and(|n| n == name))
        .ok_or_else(|| format!("MIDI input port '{name}' not found"))?;
    midi_in
        .connect(
            &port,
            "virus-input",
            move |_timestamp, data, parser: &mut MidiInputParser| {
                for event in parser.feed(data) {
                    sink.emit(event);
                }
            },
            MidiInputParser::new(),
        )
        .map_err(|e| format!("Connecting input '{name}' failed: {e}"))
}

fn connect_output(name: &str) -> Result<MidiOutputConnection, String> {
    let midi_out =
        MidiOutput::new("virus-midi-out").map_err(|e| format!("MIDI output init failed: {e}"))?;
    let port = midi_out
        .ports()
        .into_iter()
        .find(|p| midi_out.port_name(p).is_ok_and(|n| n == name))
        .ok_or_else(|| format!("MIDI output port '{name}' not found"))?;
    midi_out
        .connect(&port, "virus-output")
        .map_err(|e| format!("Connecting output '{name}' failed: {e}"))
}

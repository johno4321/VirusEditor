//! The real [`Transport`]: a thin wrapper around `virus_midi::MidiService`, plus port selection
//! by name substring and optional hex logging of all traffic (`--verbose`).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use virus_midi::{find_virus_ports, list_input_ports, list_output_ports, MidiCommand, MidiEvent, MidiService};

use crate::messages::hex;
use crate::transport::Transport;

/// How long to wait for the worker to confirm a send or a connection.
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(3);

/// Ports that are never a Virus (Windows' built-in synth).
const NEVER_VIRUS: &[&str] = &["microsoft gs wavetable"];

/// Picks one port name. With a pattern: an exact (case-insensitive) match, else the only
/// name containing it. Without: the only Virus-looking port, else the only real port.
pub fn select_port(names: &[String], pattern: Option<&str>, kind: &str, flag: &str) -> Result<String, String> {
    let list = || if names.is_empty() { "none".to_owned() } else { names.join(" | ") };
    if let Some(pattern) = pattern {
        let lower = pattern.to_lowercase();
        if let Some(exact) = names.iter().find(|n| n.to_lowercase() == lower) {
            return Ok(exact.clone());
        }
        let matches: Vec<&String> = names.iter().filter(|n| n.to_lowercase().contains(&lower)).collect();
        return match matches.as_slice() {
            [only] => Ok((*only).clone()),
            [] => Err(format!("no MIDI {kind} port contains '{pattern}' (available: {})", list())),
            many => Err(format!(
                "'{pattern}' matches several {kind} ports: {}; be more specific with {flag}",
                many.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" | ")
            )),
        };
    }
    let virus = find_virus_ports(names);
    if let [only] = virus.as_slice() {
        return Ok(only.clone());
    }
    if virus.len() > 1 {
        return Err(format!("several Virus-looking {kind} ports: {}; choose one with {flag}", virus.join(" | ")));
    }
    let real: Vec<&String> = names
        .iter()
        .filter(|n| !NEVER_VIRUS.iter().any(|x| n.to_lowercase().contains(x)))
        .collect();
    match real.as_slice() {
        [only] => Ok((*only).clone()),
        _ => Err(format!("cannot pick a MIDI {kind} port automatically; use {flag} <name part> (available: {})", list())),
    }
}

/// Raw bytes of a channel event, for `--verbose`.
fn channel_bytes(event: &MidiEvent) -> Option<Vec<u8>> {
    match *event {
        MidiEvent::ControlChange { channel, controller, value } => Some(vec![0xB0 | channel, controller, value]),
        MidiEvent::PolyPressure { channel, note, pressure } => Some(vec![0xA0 | channel, note, pressure]),
        MidiEvent::ProgramChange { channel, program } => Some(vec![0xC0 | channel, program]),
        _ => None,
    }
}

pub struct MidiLink {
    service: MidiService,
    pending: VecDeque<MidiEvent>,
    verbose: bool,
    /// Start of the session, for the millisecond timestamps in the `--verbose` log.
    started: Instant,
    pub input: Option<String>,
    pub output: Option<String>,
}

impl MidiLink {
    /// Starts the MIDI worker and connects the selected ports.
    pub fn open(
        input: Option<&str>,
        output: Option<&str>,
        need_input: bool,
        need_output: bool,
        verbose: bool,
    ) -> Result<Self, String> {
        let input_name = if need_input {
            Some(select_port(&list_input_ports()?, input, "input", "--in")?)
        } else {
            None
        };
        let output_name = if need_output {
            Some(select_port(&list_output_ports()?, output, "output", "--out")?)
        } else {
            None
        };
        let mut link = Self { service: MidiService::spawn()?, pending: VecDeque::new(), verbose, started: Instant::now(), input: None, output: None };
        if let Some(name) = &input_name {
            link.service.send_command(MidiCommand::ConnectInput(name.clone()))?;
        }
        if let Some(name) = &output_name {
            link.service.send_command(MidiCommand::ConnectOutput(name.clone()))?;
        }
        let deadline = Instant::now() + CONFIRM_TIMEOUT;
        while link.input != input_name || link.output != output_name {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match link.next_event(remaining) {
                Some(MidiEvent::ConnectionStatus { input, output }) => {
                    link.input = input;
                    link.output = output;
                }
                Some(MidiEvent::Error(e)) => {
                    return Err(format!("{e} (is another program, e.g. the editor or a DAW, using the port?)"))
                }
                Some(_) => {}
                None => return Err("timed out connecting the MIDI ports".to_owned()),
            }
        }
        Ok(link)
    }

    /// Milliseconds since the link was opened, e.g. `12345ms`.
    fn stamp(&self) -> String {
        format!("{:>7}ms", self.started.elapsed().as_millis())
    }

    /// The next event from the worker, logged when verbose. `Sent` events are not logged.
    fn next_event(&mut self, timeout: Duration) -> Option<MidiEvent> {
        let event = self.service.event_receiver().recv_timeout(timeout).ok()?;
        if self.verbose {
            match &event {
                MidiEvent::SysEx(frame) => eprintln!("{} RX {}", self.stamp(), hex(frame)),
                MidiEvent::Error(e) => eprintln!("{} RX error: {e}", self.stamp()),
                other => {
                    if let Some(bytes) = channel_bytes(other) {
                        eprintln!("{} RX {}", self.stamp(), hex(&bytes));
                    }
                }
            }
        }
        Some(event)
    }

    /// Waits for `count` `Sent` confirmations, keeping other events for later.
    fn await_sent(&mut self, count: usize, per_message: Duration, progress: &mut dyn FnMut(usize, usize)) -> Result<(), String> {
        let mut sent = 0;
        while sent < count {
            match self.next_event(per_message) {
                Some(MidiEvent::Sent { .. }) => {
                    sent += 1;
                    progress(sent, count);
                }
                Some(MidiEvent::Error(e)) => return Err(e),
                Some(other) => self.pending.push_back(other),
                None => return Err(format!("the MIDI worker did not confirm sending ({sent} of {count} sent)")),
            }
        }
        Ok(())
    }
}

impl Transport for MidiLink {
    fn send(&mut self, message: &[u8]) -> Result<(), String> {
        if self.verbose {
            eprintln!("{} TX {}", self.stamp(), hex(message));
        }
        self.service.send_command(MidiCommand::Send(message.to_vec()))?;
        self.await_sent(1, CONFIRM_TIMEOUT, &mut |_, _| {})
    }

    fn send_paced(
        &mut self,
        messages: &[Vec<u8>],
        gap: Duration,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<(), String> {
        if self.verbose {
            for (i, message) in messages.iter().enumerate() {
                eprintln!("{} TX[{}/{}] {}", self.stamp(), i + 1, messages.len(), hex(message));
            }
        }
        self.service.send_command(MidiCommand::SendBatch { messages: messages.to_vec(), gap_ms: gap.as_millis() as u64 })?;
        let result = self.await_sent(messages.len(), gap + CONFIRM_TIMEOUT, progress);
        if result.is_err() {
            let _ = self.service.send_command(MidiCommand::CancelBatch);
        }
        result
    }

    fn recv(&mut self, timeout: Duration) -> Option<MidiEvent> {
        self.pending.pop_front().or_else(|| self.next_event(timeout))
    }

    fn discard_pending(&mut self) -> usize {
        let mut dropped = self.pending.len();
        self.pending.clear();
        while let Ok(event) = self.service.event_receiver().try_recv() {
            if self.verbose {
                if let MidiEvent::SysEx(frame) = &event {
                    eprintln!("{} RX (stale, dropped) {}", self.stamp(), hex(frame));
                }
            }
            dropped += 1;
        }
        dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn pattern_selects_exact_then_unique_substring() {
        let ports = names(&["UM-ONE", "UM-ONE 2", "loopMIDI Port"]);
        assert_eq!(select_port(&ports, Some("um-one"), "input", "--in"), Ok("UM-ONE".into()));
        assert_eq!(select_port(&ports, Some("loop"), "input", "--in"), Ok("loopMIDI Port".into()));
        assert!(select_port(&ports, Some("UM"), "input", "--in").unwrap_err().contains("several"));
        assert!(select_port(&ports, Some("virus"), "input", "--in").unwrap_err().contains("available"));
    }

    #[test]
    fn automatic_selection_prefers_virus_ports_then_the_only_real_port() {
        let ports = names(&["Microsoft GS Wavetable Synth", "VIRUS Rack", "UM-ONE"]);
        assert_eq!(select_port(&ports, None, "output", "--out"), Ok("VIRUS Rack".into()));
        let ports = names(&["Microsoft GS Wavetable Synth", "UM-ONE"]);
        assert_eq!(select_port(&ports, None, "output", "--out"), Ok("UM-ONE".into()));
        let ports = names(&["UM-ONE", "loopMIDI Port"]);
        assert!(select_port(&ports, None, "output", "--out").unwrap_err().contains("--out"));
        assert!(select_port(&[], None, "input", "--in").unwrap_err().contains("none"));
    }
}

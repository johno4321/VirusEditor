//! MIDI port enumeration and Access Virus port discovery.

use std::time::{Duration, Instant};

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

/// When the worker re-enumerates ports, and whether the lists changed since last time (R10).
/// Polling is off until an interval is set.
#[derive(Debug, Default)]
pub struct PortWatch {
    interval: Option<Duration>,
    next_poll: Option<Instant>,
    last: Option<(Vec<String>, Vec<String>)>,
}

impl PortWatch {
    /// Polls every `interval` from `now` on (`None` or zero = never).
    pub fn set_interval(&mut self, interval: Option<Duration>, now: Instant) {
        self.interval = interval.filter(|i| !i.is_zero());
        self.next_poll = self.interval.map(|i| now + i);
    }

    /// Time until the next poll is due, if polling is on.
    pub fn time_until_poll(&self, now: Instant) -> Option<Duration> {
        self.next_poll.map(|t| t.saturating_duration_since(now))
    }

    pub fn is_due(&self, now: Instant) -> bool {
        self.next_poll.is_some_and(|t| now >= t)
    }

    /// A poll happened at `now` (successful or not): the next one is due an interval later.
    pub fn polled(&mut self, now: Instant) {
        self.next_poll = self.interval.map(|i| now + i);
    }

    /// Records the current lists and returns whether they differ from the last ones recorded.
    pub fn changed(&mut self, inputs: &[String], outputs: &[String]) -> bool {
        let same = self.last.as_ref().is_some_and(|(i, o)| i == inputs && o == outputs);
        if !same {
            self.last = Some((inputs.to_vec(), outputs.to_vec()));
        }
        !same
    }

    /// Forgets the last lists, so the next poll reports them even if they are unchanged
    /// (e.g. after a send failure closed the output, so the app reconnects it).
    pub fn forget(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn port_watch_is_off_until_an_interval_is_set() {
        let t0 = Instant::now();
        let mut watch = PortWatch::default();
        assert_eq!(watch.time_until_poll(t0), None);
        assert!(!watch.is_due(t0 + Duration::from_secs(60)));
        watch.set_interval(Some(Duration::ZERO), t0);
        assert_eq!(watch.time_until_poll(t0), None);
    }

    #[test]
    fn port_watch_polls_every_interval() {
        let t0 = Instant::now();
        let every = Duration::from_secs(2);
        let mut watch = PortWatch::default();
        watch.set_interval(Some(every), t0);
        assert_eq!(watch.time_until_poll(t0), Some(every));
        assert!(!watch.is_due(t0 + every - Duration::from_millis(1)));
        assert!(watch.is_due(t0 + every));
        watch.polled(t0 + every);
        assert_eq!(watch.time_until_poll(t0 + every), Some(every));
        watch.set_interval(None, t0);
        assert_eq!(watch.time_until_poll(t0), None);
    }

    #[test]
    fn port_watch_reports_only_changed_lists() {
        let mut watch = PortWatch::default();
        let outs = names(&["mioXC MIDI OUT"]);
        assert!(watch.changed(&[], &outs));
        assert!(!watch.changed(&[], &outs));
        assert!(watch.changed(&[], &names(&["2- mioXC MIDI OUT"])));
        assert!(watch.changed(&names(&["mioXC MIDI IN"]), &names(&["2- mioXC MIDI OUT"])));
    }

    #[test]
    fn port_watch_reports_unchanged_lists_again_after_forget() {
        let mut watch = PortWatch::default();
        let outs = names(&["mioXC MIDI OUT"]);
        watch.changed(&[], &outs);
        watch.forget();
        assert!(watch.changed(&[], &outs));
        assert!(!watch.changed(&[], &outs));
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

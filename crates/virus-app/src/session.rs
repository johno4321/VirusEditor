//! Non-UI editor logic: MIDI settings, the edited patch, interpretation of incoming MIDI,
//! the SysEx messages for outgoing edits, MIDI port selection and activity LEDs.
//!
//! Nothing here touches egui or a real MIDI port, so it is unit-tested directly.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use virus_db::IncomingDump;
use crate::library::queue_received;
use virus_midi::{auto_pick_port, find_port_or_renamed, MidiCommand, MidiEvent};
use virus_syx::{
    encode_dump_request, encode_param_change, encode_single_bank_request, is_sound_param,
    parse_sysex, single_param, DumpTarget, Page, ParamClass, Patch, SingleBank, SysExMessage,
    DEVICE_ID_OMNI, PART_SINGLE_BUFFER,
};

/// Offset of the name (B112) within Page B.
const NAME_PARAM: u8 = 112;
const NAME_LEN: usize = 10;

/// Which edit buffer the editor talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EditPart {
    /// Single mode: part `0x40`.
    #[default]
    Single,
    /// Multi / Multi-Single mode: part 0..=15 (shown as 1..=16).
    Multi(u8),
}

impl EditPart {
    /// The SysEx part byte (`pp` / `ss`).
    pub fn raw(self) -> u8 {
        match self {
            Self::Single => PART_SINGLE_BUFFER,
            Self::Multi(part) => part & 0x0F,
        }
    }

    pub fn label(self) -> String {
        match self {
            Self::Single => "Single".to_owned(),
            Self::Multi(part) => format!("Multi part {}", part + 1),
        }
    }
}

/// How the app addresses the synth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiSettings {
    /// Synth MIDI channel 0..=15 (shown as 1..=16); CC/PolyPressure are accepted only here.
    pub channel: u8,
    /// SysEx device ID: 0x00..=0x0F = device 1..16, 0x10 = Omni. OS 1.50+ default is device 1.
    pub device_id: u8,
    pub part: EditPart,
}

impl Default for MidiSettings {
    fn default() -> Self {
        Self { channel: 0, device_id: 0x00, part: EditPart::Single }
    }
}

impl MidiSettings {
    /// A message from device `dd` is for us if either side is Omni or the IDs match.
    fn accepts_device(&self, dd: u8) -> bool {
        self.device_id == DEVICE_ID_OMNI || dd == DEVICE_ID_OMNI || dd == self.device_id
    }
}

/// Keeps live messages (edits, requests) out of a paced bank send (L2): while a batch is
/// being transmitted they wait, then go out in order. Requests (`30`–`37`) also wait for a
/// quiet line, because the Virus ignores requests that arrive too soon after it has sent or
/// applied something (SPEC §3.7, G4).
///
/// A batch runs from [`start`](Self::start) until the worker reports it finished
/// ([`finish`](Self::finish)); an estimate of its length is only a fallback in case that report
/// never comes. After it, live messages keep the batch's gap of silence.
#[derive(Debug, Default)]
pub struct PacedSend {
    /// While a batch runs: the latest time it can end.
    batch_until: Option<Instant>,
    /// Silence kept after the batch's last message (its gap plus a dump's wire time).
    batch_tail: Duration,
    /// After a batch: live messages wait until then.
    busy_until: Option<Instant>,
    queue: VecDeque<(Ticket, Vec<u8>)>,
    next_ticket: u64,
    /// Last MIDI sent or received.
    last_activity: Option<Instant>,
}

/// Identifies a message queued by [`PacedSend::submit_tracked`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ticket(u64);

/// What [`PacedSend::submit_tracked`] did with a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Submitted {
    /// It may go out now.
    Now(Vec<u8>),
    /// It waits in the queue.
    Queued(Ticket),
}

/// Silence the Virus needs before a request (measured live, SPEC §3.7).
pub const REQUEST_QUIET: Duration = Duration::from_millis(500);
/// Silence before a bank, arrangement or total request.
pub const BANK_REQUEST_QUIET: Duration = Duration::from_millis(2000);

/// Length of a single or multi dump frame; batches are made of these.
const DUMP_LEN: usize = 267;
/// Added to a batch's estimated length before the fallback lets live messages out.
const BATCH_END_SLACK: Duration = Duration::from_secs(2);

/// How long sending `count` dumps takes with `gap` of silence after each one has left the wire
/// (shown in the bank-send dialogs).
pub fn dump_batch_duration(count: usize, gap: Duration) -> Duration {
    (gap + virus_midi::wire_time(DUMP_LEN)) * u32::try_from(count).unwrap_or(u32::MAX)
}

/// How long the line must be quiet before `message` may go out (zero for non-requests).
pub fn required_quiet(message: &[u8]) -> Duration {
    let is_virus = message.len() > 6 && message[..5] == [0xF0, 0x00, 0x20, 0x33, 0x01];
    match message.get(6) {
        Some(0x32 | 0x33 | 0x34 | 0x36) if is_virus => BANK_REQUEST_QUIET,
        Some(0x30..=0x37) if is_virus => REQUEST_QUIET,
        _ => Duration::ZERO,
    }
}

impl PacedSend {
    /// A batch of `count` dumps with `gap` of silence after each starts now. Appending to a
    /// running batch extends it (the worker reports one end for both).
    pub fn start(&mut self, now: Instant, count: usize, gap: Duration) {
        // Per dump: the gap, the wire time, and the same again in case the OS send blocks
        // until the bytes are out.
        let per_dump = gap + virus_midi::wire_time(DUMP_LEN) * 2;
        let from = self.batch_until.filter(|&t| t > now).unwrap_or(now);
        self.batch_until = Some(from + per_dump * u32::try_from(count).unwrap_or(u32::MAX) + BATCH_END_SLACK);
        self.batch_tail = gap + virus_midi::wire_time(DUMP_LEN);
    }

    /// The worker reported the batch finished (sent, cancelled or aborted) at `now`; waiting
    /// messages go out after the gap following its last dump.
    pub fn finish(&mut self, now: Instant) {
        if self.batch_until.take().is_some() {
            self.busy_until = Some(now + self.batch_tail);
        }
    }

    /// Forgets the batch; waiting messages may go out at once.
    pub fn cancel(&mut self) {
        self.batch_until = None;
        self.busy_until = None;
    }

    /// A batch was started and has not been reported finished (nor outlived its estimate).
    pub fn batch_running(&self, now: Instant) -> bool {
        self.batch_until.is_some_and(|t| now < t)
    }

    /// MIDI was sent or received at `now`; requests wait for the line to go quiet again.
    pub fn note_activity(&mut self, now: Instant) {
        self.last_activity = Some(now);
    }

    /// When `message` may go out, if not now.
    fn ready_at(&self, message: &[u8], now: Instant) -> Option<Instant> {
        if let Some(until) = self.batch_until.filter(|&t| now < t) {
            return Some(until);
        }
        if let Some(until) = self.busy_until.filter(|&t| now < t) {
            return Some(until);
        }
        let quiet = required_quiet(message);
        let at = self.last_activity? + quiet;
        (!quiet.is_zero() && now < at).then_some(at)
    }

    /// Returns the message if it can go out immediately, otherwise queues it.
    pub fn submit(&mut self, message: Vec<u8>, now: Instant) -> Option<Vec<u8>> {
        match self.submit_tracked(message, now) {
            Submitted::Now(message) => Some(message),
            Submitted::Queued(_) => None,
        }
    }

    /// Like [`submit`](Self::submit), but a queued message gets a ticket, so the caller can
    /// tell when it went out ([`is_queued`](Self::is_queued)) or withdraw it ([`remove`](Self::remove)).
    pub fn submit_tracked(&mut self, message: Vec<u8>, now: Instant) -> Submitted {
        if self.ready_at(&message, now).is_some() || !self.queue.is_empty() {
            let ticket = Ticket(self.next_ticket);
            self.next_ticket += 1;
            self.queue.push_back((ticket, message));
            Submitted::Queued(ticket)
        } else {
            self.last_activity = Some(now);
            Submitted::Now(message)
        }
    }

    /// The message is still waiting (not yet returned by `drain_ready`, nor removed).
    pub fn is_queued(&self, ticket: Ticket) -> bool {
        self.queue.iter().any(|(t, _)| *t == ticket)
    }

    /// Withdraws a waiting message. Returns whether it was still queued.
    pub fn remove(&mut self, ticket: Ticket) -> bool {
        let before = self.queue.len();
        self.queue.retain(|(t, _)| *t != ticket);
        self.queue.len() != before
    }

    /// Queued messages that may be sent now, in order; stops at the first that must still wait.
    pub fn drain_ready(&mut self, now: Instant) -> Vec<Vec<u8>> {
        if !self.batch_running(now) {
            self.batch_until = None;
        }
        if self.busy_until.is_some_and(|t| now >= t) {
            self.busy_until = None;
        }
        let mut ready = Vec::new();
        while let Some((_, message)) = self.queue.front() {
            if self.ready_at(message, now).is_some() {
                break;
            }
            let (_, message) = self.queue.pop_front().expect("front exists");
            self.last_activity = Some(now);
            ready.push(message);
        }
        ready
    }

    /// How long until queued messages can be sent, if any are waiting.
    pub fn next_wakeup(&self, now: Instant) -> Option<Duration> {
        let (_, front) = self.queue.front()?;
        Some(self.ready_at(front, now).map_or(Duration::ZERO, |t| t.saturating_duration_since(now)))
    }
}

/// What an incoming MIDI event meant for the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incoming {
    /// An edit-buffer dump for the edited part replaced the editor patch.
    PatchLoaded { name: String, checksum_ok: bool },
    /// One parameter of the edited patch changed.
    ParamChanged { page: Page, number: u8, value: u8 },
    /// A single dump for a bank slot or another part, queued in [`Session::received`].
    DumpQueued { bank: u8, program: u8, checksum_ok: bool },
    /// Not for the editor (other channel, performance controller, other part, other device…).
    Ignored,
}

/// The edited sound and the rules for keeping it in sync with the synth.
#[derive(Debug, Default)]
pub struct Session {
    pub settings: MidiSettings,
    patch: Patch,
    /// Single dumps received from the synth that weren't for the edited buffer
    /// (e.g. the answer to a bank request). The librarian offers to import them.
    pub received: Vec<IncomingDump>,
}

impl Session {
    pub fn patch(&self) -> &Patch {
        &self.patch
    }

    pub fn load_patch(&mut self, patch: Patch) {
        self.patch = patch;
    }

    /// Interprets one event from the MIDI worker (SPEC §1.4, §5.5).
    pub fn handle_midi(&mut self, event: &MidiEvent) -> Incoming {
        match *event {
            MidiEvent::ControlChange { channel, controller, value } => {
                self.apply_channel_param(channel, Page::PageA, controller, value)
            }
            MidiEvent::PolyPressure { channel, note, pressure } => {
                self.apply_channel_param(channel, Page::PageB, note, pressure)
            }
            MidiEvent::SysEx(ref frame) => self.handle_sysex(frame),
            _ => Incoming::Ignored,
        }
    }

    /// CC (Page A) / PolyPressure (Page B): only on the synth's channel and only for sound
    /// parameters, never performance controllers such as mod wheel or volume (D6).
    fn apply_channel_param(&mut self, channel: u8, page: Page, number: u8, value: u8) -> Incoming {
        if channel != self.settings.channel || !is_sound_param(page, number) {
            return Incoming::Ignored;
        }
        // In Multi mode CC/PolyPressure address parts by channel (F3/F4 will map them); until then
        // they must not edit the Single patch.
        if self.settings.part != EditPart::Single {
            return Incoming::Ignored;
        }
        self.patch.set(page, number, value);
        Incoming::ParamChanged { page, number, value: value & 0x7F }
    }

    fn handle_sysex(&mut self, frame: &[u8]) -> Incoming {
        let Ok(message) = parse_sysex(frame) else {
            return Incoming::Ignored;
        };
        if !self.settings.accepts_device(message.device_id()) {
            return Incoming::Ignored;
        }
        match message {
            SysExMessage::SingleDump { bank, program, data, checksum_ok, .. } => {
                if bank == 0 && program == self.settings.part.raw() {
                    self.patch = Patch::from_bytes(&data);
                    Incoming::PatchLoaded { name: self.patch.name(), checksum_ok }
                } else {
                    queue_received(&mut self.received, IncomingDump { bank, program, data: *data, checksum_ok });
                    Incoming::DumpQueued { bank, program, checksum_ok }
                }
            }
            SysExMessage::ParamChange { page, part, param, value, .. } => {
                if part != self.settings.part.raw() || !accepts_sysex_param(page, param) {
                    return Incoming::Ignored;
                }
                self.patch.set(page, param, value);
                Incoming::ParamChanged { page, number: param, value: value & 0x7F }
            }
            _ => Incoming::Ignored,
        }
    }

    /// Changes one byte of the edited patch. Returns the SysEx parameter change to send (P8),
    /// or `None` if nothing changed or the parameter isn't editable (Page C, A0 Sound Version).
    pub fn edit(&mut self, page: Page, number: u8, value: u8) -> Option<Vec<u8>> {
        if page == Page::PageC || (page == Page::PageA && number == 0) {
            return None;
        }
        let before = self.patch.get(page, number);
        self.patch.set(page, number, value);
        let after = self.patch.get(page, number);
        (after != before).then(|| self.param_message(page, number, after))
    }

    /// Writes the name (ASCII 32..126, at most 10 characters, D7) and returns one parameter
    /// change per name byte that changed.
    pub fn set_name(&mut self, name: &str) -> Vec<Vec<u8>> {
        let before = self.patch.clone();
        self.patch.set_name(&sanitize_name(name));
        (0..NAME_LEN as u8)
            .map(|i| NAME_PARAM + i)
            .filter(|&n| self.patch.get(Page::PageB, n) != before.get(Page::PageB, n))
            .map(|n| self.param_message(Page::PageB, n, self.patch.get(Page::PageB, n)))
            .collect()
    }

    /// SysEx `7x pp nn vv` addressed to the edited part.
    pub fn param_message(&self, page: Page, number: u8, value: u8) -> Vec<u8> {
        encode_param_change(self.settings.device_id, page, self.settings.part.raw(), number, value)
    }

    /// Single request for the edited part's edit buffer (`30 00 pp`).
    pub fn fetch_request(&self) -> Vec<u8> {
        encode_dump_request(self.settings.device_id, 0, self.settings.part.raw())
    }

    /// Single dump of `patch` into the edited part's edit buffer (`10 00 pp`).
    pub fn edit_buffer_dump(&self, patch: &Patch) -> Vec<u8> {
        patch.to_sysex(self.settings.device_id, DumpTarget::EditBuffer { part: self.settings.part.raw() })
    }

    pub fn bank_request(&self, bank: SingleBank) -> Vec<u8> {
        encode_single_bank_request(self.settings.device_id, bank.to_raw())
    }
}

/// Incoming SysEx parameter changes may touch any stored byte of Page A/B (including
/// undocumented ones, which keeps the edit buffer in sync), but not performance
/// controllers, A0 or Page C.
fn accepts_sysex_param(page: Page, number: u8) -> bool {
    if page == Page::PageC {
        return false;
    }
    !matches!(
        single_param(page, number).map(|def| def.class),
        Some(ParamClass::Performance | ParamClass::Internal)
    )
}

/// Keeps printable ASCII (32..=126) only and at most 10 characters (SPEC §5.4).
pub fn sanitize_name(name: &str) -> String {
    name.chars().filter(|c| (' '..='~').contains(c)).take(NAME_LEN).collect()
}

/// How often the app asks the worker to re-enumerate ports (R10).
pub const PORT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// MIDI port lists and the user's selection, by port name (R1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ports {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub selected_input: Option<String>,
    pub selected_output: Option<String>,
    /// The port the user chose or the settings remembered. Unlike `selected_*` it is kept while
    /// the port is unplugged or renamed, so the app reconnects when it comes back.
    pub wanted_input: Option<String>,
    pub wanted_output: Option<String>,
    /// As reported by the worker's `ConnectionStatus`.
    pub connected_input: Option<String>,
    pub connected_output: Option<String>,
    /// A connect command was issued and its `ConnectionStatus` hasn't arrived yet.
    pub connecting_input: bool,
    pub connecting_output: bool,
    /// Connect to a Virus-looking port (or the only real port) until the user picks ports by hand.
    pub auto_connect: bool,
}

impl Default for Ports {
    fn default() -> Self {
        Self {
            inputs: Vec::new(),
            outputs: Vec::new(),
            selected_input: None,
            selected_output: None,
            wanted_input: None,
            wanted_output: None,
            connected_input: None,
            connected_output: None,
            connecting_input: false,
            connecting_output: false,
            auto_connect: true,
        }
    }
}

impl Ports {
    /// Updates the port lists and reconnects. The wanted port is used if present, or a port
    /// with the same base name (Windows renames ports when the USB socket changes); else
    /// auto-connect picks one, else the input/output is disconnected. A selected port that is
    /// listed but not connected (its connection failed or the worker closed a dead handle) is
    /// connected again.
    pub fn on_port_list(&mut self, inputs: Vec<String>, outputs: Vec<String>) -> Vec<MidiCommand> {
        let mut commands = Vec::new();
        commands.extend(reconcile(
            PortSide {
                wanted: &mut self.wanted_input,
                selected: &mut self.selected_input,
                connected: self.connected_input.as_deref(),
                connecting: &mut self.connecting_input,
            },
            &inputs,
            self.auto_connect,
            Side::Input,
        ));
        commands.extend(reconcile(
            PortSide {
                wanted: &mut self.wanted_output,
                selected: &mut self.selected_output,
                connected: self.connected_output.as_deref(),
                connecting: &mut self.connecting_output,
            },
            &outputs,
            self.auto_connect,
            Side::Output,
        ));
        self.inputs = inputs;
        self.outputs = outputs;
        commands
    }

    pub fn on_status(&mut self, input: Option<String>, output: Option<String>) {
        self.connected_input = input;
        self.connected_output = output;
        self.connecting_input = false;
        self.connecting_output = false;
    }

    /// The user picked an input port (`None` = disconnect). Turns auto-connect off.
    pub fn select_input(&mut self, name: Option<String>) -> MidiCommand {
        self.auto_connect = false;
        self.selected_input.clone_from(&name);
        self.wanted_input.clone_from(&name);
        self.connecting_input = name.is_some();
        name.map_or(MidiCommand::DisconnectInput, MidiCommand::ConnectInput)
    }

    /// The user picked an output port (`None` = disconnect). Turns auto-connect off.
    pub fn select_output(&mut self, name: Option<String>) -> MidiCommand {
        self.auto_connect = false;
        self.selected_output.clone_from(&name);
        self.wanted_output.clone_from(&name);
        self.connecting_output = name.is_some();
        name.map_or(MidiCommand::DisconnectOutput, MidiCommand::ConnectOutput)
    }

    pub fn output_connected(&self) -> bool {
        self.connected_output.is_some()
    }
}

#[derive(Clone, Copy)]
enum Side {
    Input,
    Output,
}

/// One side's state, as `reconcile` needs it.
struct PortSide<'a> {
    wanted: &'a mut Option<String>,
    selected: &'a mut Option<String>,
    connected: Option<&'a str>,
    connecting: &'a mut bool,
}

/// Decides which port `selected` should be and returns the command if that changes, or if the
/// selected port is listed but not connected (and no connect is under way). A wanted name that
/// cannot be found is kept; auto-connect remembers its pick only when nothing was wanted.
fn reconcile(port: PortSide<'_>, available: &[String], auto: bool, side: Side) -> Option<MidiCommand> {
    let PortSide { wanted, selected, connected, connecting } = port;
    let target = match wanted.as_deref().and_then(|w| find_port_or_renamed(w, available)) {
        Some(found) => Some(found),
        None if auto => {
            let pick = auto_pick_port(available);
            if wanted.is_none() {
                wanted.clone_from(&pick);
            }
            pick
        }
        None => None,
    };
    let reconnect = target.is_some() && target.as_deref() != connected && !*connecting;
    if target == *selected && !reconnect {
        return None;
    }
    selected.clone_from(&target);
    *connecting = target.is_some();
    Some(match (target, side) {
        (Some(name), Side::Input) => MidiCommand::ConnectInput(name),
        (Some(name), Side::Output) => MidiCommand::ConnectOutput(name),
        (None, Side::Input) => MidiCommand::DisconnectInput,
        (None, Side::Output) => MidiCommand::DisconnectOutput,
    })
}

/// Rx/Tx activity LEDs.
#[derive(Debug, Clone, Copy, Default)]
pub struct Activity {
    rx_until: Option<Instant>,
    tx_until: Option<Instant>,
}

impl Activity {
    pub const GLOW: Duration = Duration::from_millis(150);

    pub fn rx(&mut self, now: Instant) {
        self.rx_until = Some(now + Self::GLOW);
    }

    /// Call only for `MidiEvent::Sent`, so Tx never lights for a message that wasn't sent (R2).
    pub fn tx(&mut self, now: Instant) {
        self.tx_until = Some(now + Self::GLOW);
    }

    pub fn rx_lit(&self, now: Instant) -> bool {
        self.rx_until.is_some_and(|t| t > now)
    }

    pub fn tx_lit(&self, now: Instant) -> bool {
        self.tx_until.is_some_and(|t| t > now)
    }

    /// Time until the next LED goes out, if any is lit (used for a short repaint, U2).
    pub fn next_change(&self, now: Instant) -> Option<Duration> {
        [self.rx_until, self.tx_until]
            .into_iter()
            .flatten()
            .filter(|&t| t > now)
            .map(|t| t - now)
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use virus_syx::encode_program_dump;

    fn session() -> Session {
        let mut session = Session::default();
        session.settings.channel = 2;
        session
    }

    fn dump(device: u8, bank: u8, program: u8, patch: &Patch) -> MidiEvent {
        MidiEvent::SysEx(encode_program_dump(device, bank, program, patch.as_bytes()))
    }

    fn named(name: &str) -> Patch {
        let mut patch = Patch::init();
        patch.set_name(name);
        patch
    }

    #[test]
    fn cc_on_synth_channel_sets_page_a_sound_param() {
        let mut s = session();
        let event = MidiEvent::ControlChange { channel: 2, controller: 40, value: 33 };
        assert_eq!(
            s.handle_midi(&event),
            Incoming::ParamChanged { page: Page::PageA, number: 40, value: 33 }
        );
        assert_eq!(s.patch().get(Page::PageA, 40), 33);
    }

    #[test]
    fn cc_on_other_channel_is_ignored() {
        let mut s = session();
        let before = s.patch().clone();
        let event = MidiEvent::ControlChange { channel: 3, controller: 40, value: 33 };
        assert_eq!(s.handle_midi(&event), Incoming::Ignored);
        assert_eq!(s.patch(), &before);
    }

    #[test]
    fn performance_controllers_are_ignored() {
        let mut s = session();
        let before = s.patch().clone();
        for controller in [1, 7, 11, 64, 123, 0, 92] {
            let event = MidiEvent::ControlChange { channel: 2, controller, value: 99 };
            assert_eq!(s.handle_midi(&event), Incoming::Ignored, "CC{controller}");
        }
        assert_eq!(s.patch(), &before);
    }

    #[test]
    fn poly_pressure_sets_page_b_on_synth_channel_only() {
        let mut s = session();
        let on_channel = MidiEvent::PolyPressure { channel: 2, note: 84, pressure: 3 };
        assert_eq!(
            s.handle_midi(&on_channel),
            Incoming::ParamChanged { page: Page::PageB, number: 84, value: 3 }
        );
        let off_channel = MidiEvent::PolyPressure { channel: 0, note: 84, pressure: 5 };
        assert_eq!(s.handle_midi(&off_channel), Incoming::Ignored);
        assert_eq!(s.patch().get(Page::PageB, 84), 3);
    }

    #[test]
    fn incoming_param_does_not_revert_other_edits() {
        let mut s = session();
        s.edit(Page::PageA, 42, 100).expect("edit");
        let event = MidiEvent::ControlChange { channel: 2, controller: 40, value: 10 };
        s.handle_midi(&event);
        assert_eq!(s.patch().get(Page::PageA, 42), 100);
        assert_eq!(s.patch().get(Page::PageA, 40), 10);
    }

    #[test]
    fn sysex_param_change_applies_only_to_edited_part() {
        let mut s = session();
        let for_single = encode_param_change(0x00, Page::PageA, 0x40, 40, 11);
        assert_eq!(
            s.handle_midi(&MidiEvent::SysEx(for_single)),
            Incoming::ParamChanged { page: Page::PageA, number: 40, value: 11 }
        );
        let for_part3 = encode_param_change(0x00, Page::PageA, 0x02, 40, 99);
        assert_eq!(s.handle_midi(&MidiEvent::SysEx(for_part3.clone())), Incoming::Ignored);
        assert_eq!(s.patch().get(Page::PageA, 40), 11);

        s.settings.part = EditPart::Multi(2);
        assert!(matches!(s.handle_midi(&MidiEvent::SysEx(for_part3)), Incoming::ParamChanged { .. }));
        assert_eq!(s.patch().get(Page::PageA, 40), 99);
    }

    #[test]
    fn sysex_param_change_never_touches_performance_or_sound_version() {
        let mut s = session();
        let before = s.patch().clone();
        for (page, param) in [(Page::PageA, 0), (Page::PageA, 1), (Page::PageA, 7), (Page::PageC, 40)] {
            let msg = encode_param_change(0x00, page, 0x40, param, 5);
            assert_eq!(s.handle_midi(&MidiEvent::SysEx(msg)), Incoming::Ignored);
        }
        assert_eq!(s.patch(), &before);
    }

    #[test]
    fn sysex_param_change_preserves_undocumented_bytes_sync() {
        let mut s = session();
        let msg = encode_param_change(0x00, Page::PageA, 0x40, 92, 17);
        assert!(matches!(s.handle_midi(&MidiEvent::SysEx(msg)), Incoming::ParamChanged { .. }));
        assert_eq!(s.patch().get(Page::PageA, 92), 17);
    }

    #[test]
    fn sysex_from_other_device_is_ignored_unless_omni() {
        let mut s = session();
        let other = encode_param_change(0x05, Page::PageA, 0x40, 40, 1);
        assert_eq!(s.handle_midi(&MidiEvent::SysEx(other.clone())), Incoming::Ignored);
        let omni = encode_param_change(DEVICE_ID_OMNI, Page::PageA, 0x40, 40, 1);
        assert!(matches!(s.handle_midi(&MidiEvent::SysEx(omni)), Incoming::ParamChanged { .. }));
        s.settings.device_id = DEVICE_ID_OMNI;
        assert!(matches!(s.handle_midi(&MidiEvent::SysEx(other)), Incoming::ParamChanged { .. }));
    }

    #[test]
    fn edit_buffer_dump_for_edited_part_loads_patch_byte_exact() {
        let mut s = session();
        let mut bytes = *named("Smoog   OK").as_bytes();
        bytes[128 + 125] = 77; // undocumented byte
        let patch = Patch::from_bytes(&bytes);
        let incoming = s.handle_midi(&dump(0x00, 0, 0x40, &patch));
        assert_eq!(incoming, Incoming::PatchLoaded { name: "Smoog   OK".into(), checksum_ok: true });
        assert_eq!(s.patch().as_bytes(), &bytes);
        assert!(s.received.is_empty());
    }

    #[test]
    fn bank_and_other_part_dumps_are_queued_not_loaded() {
        let mut s = session();
        let before = s.patch().clone();
        let a5 = named("Bank A 5");
        assert_eq!(
            s.handle_midi(&dump(0x00, 1, 5, &a5)),
            Incoming::DumpQueued { bank: 1, program: 5, checksum_ok: true }
        );
        assert!(matches!(s.handle_midi(&dump(0x00, 0, 3, &a5)), Incoming::DumpQueued { .. }));
        assert_eq!(s.patch(), &before);
        assert_eq!(s.received.len(), 2);
        assert_eq!(s.received[0].data, *a5.as_bytes());
    }

    #[test]
    fn bad_checksum_dump_is_loaded_and_flagged() {
        let mut s = session();
        let MidiEvent::SysEx(mut frame) = dump(0x00, 0, 0x40, &named("Broken")) else { unreachable!() };
        frame[265] ^= 1;
        assert_eq!(
            s.handle_midi(&MidiEvent::SysEx(frame)),
            Incoming::PatchLoaded { name: "Broken".into(), checksum_ok: false }
        );
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Silence kept after a batch's last 267-byte dump with a 150 ms gap.
    fn tail_150() -> Duration {
        ms(150) + virus_midi::wire_time(267)
    }

    #[test]
    fn a_full_bank_at_the_default_gap_takes_about_49_seconds() {
        let full_bank = dump_batch_duration(128, ms(virus_midi::DUMP_GAP_MS));
        assert_eq!(full_bank, Duration::from_micros(128 * (300_000 + 85_440)));
        assert_eq!(full_bank.as_secs(), 49);
    }

    #[test]
    fn paced_send_lets_messages_out_while_no_batch_runs() {
        let t0 = Instant::now();
        let mut p = PacedSend::default();
        assert_eq!(p.submit(vec![1], t0), Some(vec![1]));
        assert!(!p.batch_running(t0));
    }

    #[test]
    fn paced_send_holds_live_messages_until_the_batch_is_reported_finished() {
        let t0 = Instant::now();
        let mut p = PacedSend::default();
        p.start(t0, 4, ms(150));
        assert!(p.batch_running(t0));
        assert_eq!(p.submit(vec![2], t0 + ms(100)), None);
        assert_eq!(p.submit(vec![3], t0 + ms(200)), None);
        // Longer than 4 × 150 ms: a batch now ends when the worker says so.
        assert!(p.drain_ready(t0 + ms(1000)).is_empty());
        assert!(p.batch_running(t0 + ms(1000)));
    }

    #[test]
    fn paced_send_keeps_the_gap_after_the_last_dump_once_finished() {
        let t0 = Instant::now();
        let mut p = PacedSend::default();
        p.start(t0, 4, ms(150));
        assert_eq!(p.submit(vec![2], t0), None);
        let end = t0 + ms(1000);
        p.finish(end);
        assert!(!p.batch_running(end));
        assert_eq!(p.next_wakeup(end), Some(tail_150()));
        assert!(p.drain_ready(end + tail_150() - Duration::from_micros(1)).is_empty());
        assert_eq!(p.drain_ready(end + tail_150()), vec![vec![2]]);
    }

    #[test]
    fn paced_send_falls_back_to_an_estimate_when_no_end_is_reported() {
        let t0 = Instant::now();
        let mut p = PacedSend::default();
        p.start(t0, 4, ms(150));
        assert_eq!(p.submit(vec![2], t0), None);
        // 4 × (gap + twice a dump's wire time) + 2 s slack.
        let estimate = (ms(150) + virus_midi::wire_time(267) * 2) * 4 + ms(2000);
        assert!(p.drain_ready(t0 + estimate - Duration::from_micros(1)).is_empty());
        assert_eq!(p.drain_ready(t0 + estimate), vec![vec![2]]);
        assert!(!p.batch_running(t0 + estimate));
    }

    #[test]
    fn paced_send_extends_a_running_batch_that_gets_appended() {
        let t0 = Instant::now();
        let mut p = PacedSend::default();
        p.start(t0, 1, ms(150));
        let one = ms(150) + virus_midi::wire_time(267) * 2;
        p.start(t0, 1, ms(150));
        assert!(p.batch_running(t0 + one * 2 + ms(2000) + ms(1)));
    }

    #[test]
    fn paced_send_cancel_releases_waiting_messages_at_once() {
        let t0 = Instant::now();
        let mut p = PacedSend::default();
        p.start(t0, 4, ms(150));
        assert_eq!(p.submit(vec![5], t0), None);
        p.cancel();
        assert!(!p.batch_running(t0));
        assert_eq!(p.drain_ready(t0), vec![vec![5]]);
    }

    #[test]
    fn paced_send_finish_without_a_batch_does_not_hold_messages() {
        let t0 = Instant::now();
        let mut p = PacedSend::default();
        p.finish(t0);
        assert_eq!(p.submit(vec![1], t0), Some(vec![1]));
    }

    #[test]
    fn paced_send_tracked_message_on_a_quiet_line_goes_out_now() {
        let t0 = Instant::now();
        let mut p = PacedSend::default();
        let request = encode_dump_request(0x00, 0, 0x40);
        assert_eq!(p.submit_tracked(request.clone(), t0), Submitted::Now(request));
    }

    #[test]
    fn paced_send_tells_when_a_tracked_message_went_out() {
        let t0 = Instant::now();
        let mut p = PacedSend::default();
        let request = encode_dump_request(0x00, 0, 0x40);
        p.note_activity(t0);
        let Submitted::Queued(ticket) = p.submit_tracked(request.clone(), t0) else { panic!("queued") };
        assert!(p.is_queued(ticket));
        assert!(p.drain_ready(t0 + ms(499)).is_empty());
        assert!(p.is_queued(ticket));
        assert_eq!(p.drain_ready(t0 + ms(500)), vec![request]);
        assert!(!p.is_queued(ticket));
    }

    #[test]
    fn paced_send_removes_a_queued_message_and_keeps_the_others() {
        let t0 = Instant::now();
        let mut p = PacedSend::default();
        let request = encode_dump_request(0x00, 0, 0x40);
        let change = encode_param_change(0x00, Page::PageA, 0x40, 40, 11);
        p.note_activity(t0);
        let Submitted::Queued(ticket) = p.submit_tracked(request, t0) else { panic!("queued") };
        assert_eq!(p.submit(change.clone(), t0), None);
        assert!(p.remove(ticket));
        assert!(!p.remove(ticket));
        assert!(!p.is_queued(ticket));
        // The request no longer holds the change back for the quiet line.
        assert_eq!(p.drain_ready(t0), vec![change]);
    }

    #[test]
    fn paced_send_tickets_are_distinct() {
        let t0 = Instant::now();
        let mut p = PacedSend::default();
        p.note_activity(t0);
        let request = encode_dump_request(0x00, 0, 0x40);
        let Submitted::Queued(first) = p.submit_tracked(request.clone(), t0) else { panic!("queued") };
        let Submitted::Queued(second) = p.submit_tracked(request, t0) else { panic!("queued") };
        assert_ne!(first, second);
        p.remove(first);
        assert!(p.is_queued(second));
    }

    #[test]
    fn paced_send_lets_a_request_out_on_a_quiet_line() {
        let t0 = Instant::now();
        let mut p = PacedSend::default();
        let request = encode_dump_request(0x00, 0, 0x40);
        assert_eq!(p.submit(request.clone(), t0), Some(request));
    }

    #[test]
    fn paced_send_holds_a_request_for_500_ms_after_activity() {
        let t0 = Instant::now();
        let mut p = PacedSend::default();
        let request = encode_dump_request(0x00, 0, 0x40);
        p.note_activity(t0);
        assert_eq!(p.submit(request.clone(), t0 + ms(100)), None);
        assert_eq!(p.next_wakeup(t0 + ms(100)), Some(ms(400)));
        assert!(p.drain_ready(t0 + ms(499)).is_empty());
        assert_eq!(p.drain_ready(t0 + ms(500)), vec![request]);
        assert_eq!(p.next_wakeup(t0 + ms(500)), None);
    }

    #[test]
    fn paced_send_holds_a_bank_request_for_two_seconds_after_activity() {
        let t0 = Instant::now();
        let mut p = PacedSend::default();
        let request = encode_single_bank_request(0x00, 1);
        p.note_activity(t0);
        assert_eq!(p.submit(request.clone(), t0), None);
        assert!(p.drain_ready(t0 + ms(500)).is_empty());
        assert!(p.drain_ready(t0 + ms(1999)).is_empty());
        assert_eq!(p.drain_ready(t0 + ms(2000)), vec![request]);
    }

    #[test]
    fn paced_send_does_not_delay_param_changes_after_activity() {
        let t0 = Instant::now();
        let mut p = PacedSend::default();
        let change = encode_param_change(0x00, Page::PageA, 0x40, 40, 11);
        p.note_activity(t0);
        assert_eq!(p.submit(change.clone(), t0), Some(change));
    }

    #[test]
    fn paced_send_keeps_a_param_change_behind_a_waiting_request() {
        let t0 = Instant::now();
        let mut p = PacedSend::default();
        let request = encode_dump_request(0x00, 0, 0x40);
        let change = encode_param_change(0x00, Page::PageA, 0x40, 40, 11);
        p.note_activity(t0);
        assert_eq!(p.submit(request.clone(), t0), None);
        assert_eq!(p.submit(change.clone(), t0 + ms(10)), None);
        assert!(p.drain_ready(t0 + ms(400)).is_empty());
        assert_eq!(p.drain_ready(t0 + ms(500)), vec![request, change]);
    }

    #[test]
    fn paced_send_spaces_back_to_back_requests() {
        let t0 = Instant::now();
        let mut p = PacedSend::default();
        let first = encode_dump_request(0x00, 0, 0x40);
        let second = encode_dump_request(0x00, 1, 5);
        assert_eq!(p.submit(first.clone(), t0), Some(first));
        assert_eq!(p.submit(second.clone(), t0), None);
        assert_eq!(p.drain_ready(t0 + ms(500)), vec![second]);
    }

    #[test]
    fn paced_send_incoming_stream_pushes_a_waiting_request_back() {
        let t0 = Instant::now();
        let mut p = PacedSend::default();
        let request = encode_dump_request(0x00, 0, 0x40);
        p.note_activity(t0);
        assert_eq!(p.submit(request.clone(), t0), None);
        p.note_activity(t0 + ms(300));
        assert!(p.drain_ready(t0 + ms(500)).is_empty());
        p.note_activity(t0 + ms(700));
        assert!(p.drain_ready(t0 + ms(1000)).is_empty());
        assert_eq!(p.next_wakeup(t0 + ms(1000)), Some(ms(200)));
        assert_eq!(p.drain_ready(t0 + ms(1200)), vec![request]);
    }

    #[test]
    fn cc_and_poly_pressure_are_ignored_in_multi_mode() {
        let mut s = session();
        s.settings.part = EditPart::Multi(0);
        let before = s.patch().clone();
        let cc = MidiEvent::ControlChange { channel: 2, controller: 40, value: 99 };
        assert_eq!(s.handle_midi(&cc), Incoming::Ignored);
        let pp = MidiEvent::PolyPressure { channel: 2, note: 40, pressure: 99 };
        assert_eq!(s.handle_midi(&pp), Incoming::Ignored);
        assert_eq!(s.patch(), &before);
        s.settings.part = EditPart::Single;
        assert!(matches!(s.handle_midi(&cc), Incoming::ParamChanged { .. }));
    }

    #[test]
    fn repeated_dumps_for_one_slot_are_replaced_not_stacked() {
        let mut s = session();
        s.handle_midi(&dump(0x00, 1, 5, &named("old")));
        s.handle_midi(&dump(0x00, 1, 5, &named("new")));
        assert_eq!(s.received.len(), 1);
        assert_eq!(s.received[0].data, *named("new").as_bytes());
    }

    #[test]
    fn edit_sends_sysex_param_change_to_single_part() {
        let mut s = session();
        let msg = s.edit(Page::PageA, 40, 95).expect("changed");
        assert_eq!(msg, encode_param_change(0x00, Page::PageA, 0x40, 40, 95));
        assert_eq!(msg[6], 0x70);
        let msg = s.edit(Page::PageB, 84, 2).expect("changed");
        assert_eq!(msg[6..10], [0x71, 0x40, 84, 2]);
    }

    #[test]
    fn edit_addresses_selected_multi_part_and_device() {
        let mut s = session();
        s.settings.part = EditPart::Multi(5);
        s.settings.device_id = 0x03;
        let msg = s.edit(Page::PageA, 40, 95).expect("changed");
        assert_eq!(msg, encode_param_change(0x03, Page::PageA, 0x05, 40, 95));
    }

    #[test]
    fn unchanged_or_protected_edits_send_nothing() {
        let mut s = session();
        let current = s.patch().get(Page::PageA, 40);
        assert_eq!(s.edit(Page::PageA, 40, current), None);
        assert_eq!(s.edit(Page::PageA, 0, 1), None);
        assert_eq!(s.edit(Page::PageC, 10, 1), None);
        assert_eq!(s.patch().sound_version(), 6);
    }

    #[test]
    fn set_name_sends_only_changed_ascii_bytes() {
        let mut s = session();
        s.load_patch(named("Bass"));
        let messages = s.set_name("Bast");
        assert_eq!(messages, vec![encode_param_change(0x00, Page::PageB, 0x40, 115, b't')]);
        let messages = s.set_name("Bäst Lead Extra");
        assert_eq!(s.patch().name(), "Bst Lead E");
        assert!(messages.iter().all(|m| m[9] <= 126));
    }

    #[test]
    fn sanitize_name_keeps_printable_ascii_and_ten_chars() {
        assert_eq!(sanitize_name("Ünï\tcode✓ Lead Extra"), "ncode Lead");
        assert_eq!(sanitize_name("Smoog   OK"), "Smoog   OK");
    }

    #[test]
    fn fetch_and_send_address_edit_buffer_of_part() {
        let mut s = session();
        assert_eq!(s.fetch_request()[6..9], [0x30, 0x00, 0x40]);
        s.settings.part = EditPart::Multi(15);
        assert_eq!(s.fetch_request()[6..9], [0x30, 0x00, 0x0F]);
        let dump = s.edit_buffer_dump(&Patch::init());
        assert_eq!(dump.len(), 267);
        assert_eq!(dump[6..9], [0x10, 0x00, 0x0F]);
        assert_eq!(s.bank_request(SingleBank::B)[6..8], [0x32, 0x02]);
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn ports_auto_connect_to_virus_ports_by_name() {
        let mut ports = Ports::default();
        let commands = ports.on_port_list(names(&["loopMIDI", "Virus In"]), names(&["GS Synth", "Virus Out"]));
        assert_eq!(
            commands,
            vec![MidiCommand::ConnectInput("Virus In".into()), MidiCommand::ConnectOutput("Virus Out".into())]
        );
        assert_eq!(ports.selected_input.as_deref(), Some("Virus In"));
        assert!(ports.on_port_list(ports.inputs.clone(), ports.outputs.clone()).is_empty());
    }

    #[test]
    fn ports_clear_stale_selection_after_refresh() {
        let mut ports = Ports::default();
        assert_eq!(ports.select_input(Some("USB MIDI".into())), MidiCommand::ConnectInput("USB MIDI".into()));
        let commands = ports.on_port_list(names(&["Virus In"]), vec![]);
        assert_eq!(commands, vec![MidiCommand::DisconnectInput]);
        assert_eq!(ports.selected_input, None);
    }

    #[test]
    fn manual_selection_disables_auto_connect() {
        let mut ports = Ports::default();
        assert_eq!(ports.select_output(None), MidiCommand::DisconnectOutput);
        assert!(ports.on_port_list(names(&["Virus In"]), names(&["Virus Out"])).is_empty());
    }

    #[test]
    fn renamed_port_is_reconnected_and_the_wanted_name_is_kept() {
        let mut ports = Ports::default();
        ports.select_output(Some("mioXC MIDI OUT".into()));
        let commands = ports.on_port_list(vec![], names(&["2- mioXC MIDI OUT"]));
        assert_eq!(commands, vec![MidiCommand::ConnectOutput("2- mioXC MIDI OUT".into())]);
        assert_eq!(ports.selected_output.as_deref(), Some("2- mioXC MIDI OUT"));
        assert_eq!(ports.wanted_output.as_deref(), Some("mioXC MIDI OUT"));
        assert!(ports.on_port_list(vec![], names(&["2- mioXC MIDI OUT"])).is_empty());
    }

    #[test]
    fn port_returns_after_being_gone() {
        let mut ports = Ports::default();
        ports.select_output(Some("mioXC MIDI OUT".into()));
        ports.on_port_list(vec![], names(&["mioXC MIDI OUT"]));
        ports.on_status(None, Some("mioXC MIDI OUT".into()));
        assert_eq!(ports.on_port_list(vec![], vec![]), vec![MidiCommand::DisconnectOutput]);
        assert_eq!(ports.selected_output, None);
        assert_eq!(ports.wanted_output.as_deref(), Some("mioXC MIDI OUT"));
        let back = ports.on_port_list(vec![], names(&["3- mioXC MIDI OUT"]));
        assert_eq!(back, vec![MidiCommand::ConnectOutput("3- mioXC MIDI OUT".into())]);
    }

    #[test]
    fn port_that_returns_under_the_same_name_after_a_dead_handle_is_reconnected() {
        let mut ports = Ports::default();
        ports.select_output(Some("mioXC MIDI OUT".into()));
        ports.on_status(None, Some("mioXC MIDI OUT".into()));
        // A send failed: the worker closed the output and re-reports the unchanged list.
        ports.on_status(None, None);
        let commands = ports.on_port_list(vec![], names(&["mioXC MIDI OUT"]));
        assert_eq!(commands, vec![MidiCommand::ConnectOutput("mioXC MIDI OUT".into())]);
        assert_eq!(ports.selected_output.as_deref(), Some("mioXC MIDI OUT"));
    }

    #[test]
    fn dead_port_that_returns_renamed_is_reconnected_under_the_new_name() {
        let mut ports = Ports::default();
        ports.select_output(Some("mioXC MIDI OUT".into()));
        ports.on_status(None, Some("mioXC MIDI OUT".into()));
        ports.on_status(None, None);
        let commands = ports.on_port_list(vec![], names(&["2- mioXC MIDI OUT"]));
        assert_eq!(commands, vec![MidiCommand::ConnectOutput("2- mioXC MIDI OUT".into())]);
        assert_eq!(ports.wanted_output.as_deref(), Some("mioXC MIDI OUT"));
    }

    #[test]
    fn unchanged_list_sends_nothing_while_connected() {
        let mut ports = Ports::default();
        ports.select_output(Some("mioXC MIDI OUT".into()));
        ports.on_status(None, Some("mioXC MIDI OUT".into()));
        assert!(ports.on_port_list(vec![], names(&["mioXC MIDI OUT"])).is_empty());
        assert!(ports.on_port_list(vec![], names(&["mioXC MIDI OUT"])).is_empty());
    }

    #[test]
    fn no_second_connect_while_the_first_is_under_way() {
        let mut ports = Ports::default();
        let first = ports.on_port_list(vec![], names(&["Virus Out"]));
        assert_eq!(first, vec![MidiCommand::ConnectOutput("Virus Out".into())]);
        assert!(ports.on_port_list(vec![], names(&["Virus Out"])).is_empty());
    }

    #[test]
    fn failed_connect_is_retried_once_per_port_list() {
        let mut ports = Ports::default();
        ports.select_output(Some("mioXC MIDI OUT".into()));
        ports.on_status(None, None); // e.g. the port is open in another program
        let retry = ports.on_port_list(vec![], names(&["mioXC MIDI OUT"]));
        assert_eq!(retry, vec![MidiCommand::ConnectOutput("mioXC MIDI OUT".into())]);
        assert!(ports.on_port_list(vec![], names(&["mioXC MIDI OUT"])).is_empty());
    }

    #[test]
    fn user_chosen_port_is_kept_when_auto_connect_would_pick_another() {
        let mut ports = Ports { wanted_output: Some("UM-ONE".into()), ..Ports::default() };
        let commands = ports.on_port_list(vec![], names(&["UM-ONE", "Virus Out"]));
        assert_eq!(commands, vec![MidiCommand::ConnectOutput("UM-ONE".into())]);
        assert_eq!(ports.wanted_output.as_deref(), Some("UM-ONE"));
        // While it is missing, auto-connect may stand in, but the wish is not overwritten.
        let commands = ports.on_port_list(vec![], names(&["Virus Out"]));
        assert_eq!(commands, vec![MidiCommand::ConnectOutput("Virus Out".into())]);
        assert_eq!(ports.wanted_output.as_deref(), Some("UM-ONE"));
        let commands = ports.on_port_list(vec![], names(&["UM-ONE", "Virus Out"]));
        assert_eq!(commands, vec![MidiCommand::ConnectOutput("UM-ONE".into())]);
    }

    #[test]
    fn auto_connect_falls_back_to_the_only_real_port() {
        let mut ports = Ports::default();
        let commands = ports.on_port_list(
            names(&["mioXC MIDI IN"]),
            names(&["Microsoft GS Wavetable Synth", "mioXC MIDI OUT"]),
        );
        assert_eq!(
            commands,
            vec![MidiCommand::ConnectInput("mioXC MIDI IN".into()), MidiCommand::ConnectOutput("mioXC MIDI OUT".into())]
        );
        let mut ports = Ports::default();
        assert!(ports.on_port_list(vec![], names(&["mioXC MIDI OUT", "loopMIDI"])).is_empty());
    }

    #[test]
    fn leds_glow_briefly() {
        let start = Instant::now();
        let mut leds = Activity::default();
        assert!(!leds.tx_lit(start));
        assert_eq!(leds.next_change(start), None);
        leds.tx(start);
        assert!(leds.tx_lit(start));
        assert!(!leds.rx_lit(start));
        assert_eq!(leds.next_change(start), Some(Activity::GLOW));
        assert!(!leds.tx_lit(start + Activity::GLOW));
        assert_eq!(leds.next_change(start + Activity::GLOW), None);
    }
}

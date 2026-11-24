//! Non-UI editor logic: MIDI settings, the edited patch, interpretation of incoming MIDI,
//! the SysEx messages for outgoing edits, MIDI port selection and activity LEDs.
//!
//! Nothing here touches egui or a real MIDI port, so it is unit-tested directly.

use std::time::{Duration, Instant};

use virus_db::IncomingDump;
use virus_midi::{find_virus_ports, MidiCommand, MidiEvent};
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
                    self.received.push(IncomingDump { bank, program, data: *data, checksum_ok });
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

/// MIDI port lists and the user's selection, by port name (R1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ports {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub selected_input: Option<String>,
    pub selected_output: Option<String>,
    /// As reported by the worker's `ConnectionStatus`.
    pub connected_input: Option<String>,
    pub connected_output: Option<String>,
    /// Connect to Virus-looking ports automatically until the user picks ports by hand.
    pub auto_connect: bool,
}

impl Default for Ports {
    fn default() -> Self {
        Self {
            inputs: Vec::new(),
            outputs: Vec::new(),
            selected_input: None,
            selected_output: None,
            connected_input: None,
            connected_output: None,
            auto_connect: true,
        }
    }
}

impl Ports {
    /// Updates the port lists. Selections whose port vanished are cleared and disconnected;
    /// empty selections are auto-connected to the first Virus-looking port.
    pub fn on_port_list(&mut self, inputs: Vec<String>, outputs: Vec<String>) -> Vec<MidiCommand> {
        let mut commands = Vec::new();
        if let Some(cmd) = reconcile(&mut self.selected_input, &inputs, self.auto_connect, Side::Input) {
            commands.push(cmd);
        }
        if let Some(cmd) = reconcile(&mut self.selected_output, &outputs, self.auto_connect, Side::Output)
        {
            commands.push(cmd);
        }
        self.inputs = inputs;
        self.outputs = outputs;
        commands
    }

    pub fn on_status(&mut self, input: Option<String>, output: Option<String>) {
        self.connected_input = input;
        self.connected_output = output;
    }

    /// The user picked an input port (`None` = disconnect). Turns auto-connect off.
    pub fn select_input(&mut self, name: Option<String>) -> MidiCommand {
        self.auto_connect = false;
        self.selected_input.clone_from(&name);
        name.map_or(MidiCommand::DisconnectInput, MidiCommand::ConnectInput)
    }

    /// The user picked an output port (`None` = disconnect). Turns auto-connect off.
    pub fn select_output(&mut self, name: Option<String>) -> MidiCommand {
        self.auto_connect = false;
        self.selected_output.clone_from(&name);
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

fn reconcile(selected: &mut Option<String>, available: &[String], auto: bool, side: Side) -> Option<MidiCommand> {
    match selected {
        Some(name) if !available.contains(name) => {
            *selected = None;
            Some(match side {
                Side::Input => MidiCommand::DisconnectInput,
                Side::Output => MidiCommand::DisconnectOutput,
            })
        }
        Some(_) => None,
        None if auto => {
            let name = find_virus_ports(available).into_iter().next()?;
            *selected = Some(name.clone());
            Some(match side {
                Side::Input => MidiCommand::ConnectInput(name),
                Side::Output => MidiCommand::ConnectOutput(name),
            })
        }
        None => None,
    }
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

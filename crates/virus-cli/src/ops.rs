//! Synth operations built on a [`Transport`]: fetch, send, set, bank transfer. They print
//! nothing; progress is reported through callbacks so they run against the fake Virus in tests.

use std::time::Duration;

use virus_db::IncomingDump;

use crate::address::ParamAddress;
use crate::messages;
use crate::settings::{bank_label, Target};
use crate::transport::{
    default_quiet_limit, exchange, wait_quiet, BankCollector, CollectError, EditBufferCollector,
    FirstVirusMessage, ReceivedDump, Transport,
};

/// A transport plus how to address the synth and how long to wait.
pub struct Synth<T> {
    pub transport: T,
    pub target: Target,
    /// Idle timeout for answers.
    pub timeout: Duration,
    /// Pause after parameter changes / dumps before reading back, so the synth has applied them.
    pub settle: Duration,
    /// Quiet line required before a request (see [`crate::transport::wait_quiet`]).
    pub quiet: Duration,
    /// Quiet line required before a bank request; the Virus needs longer here.
    pub bank_quiet: Duration,
    /// Give up waiting for a quiet line after this long (`None`: [`default_quiet_limit`]).
    pub quiet_limit: Option<Duration>,
    /// Called with the number of stale messages dropped while waiting for a quiet line (only
    /// when some were dropped); `--verbose` prints it.
    pub on_stale: Option<Box<dyn FnMut(usize)>>,
}

impl<T: Transport> Synth<T> {
    pub fn new(transport: T, target: Target, timeout: Duration, settle: Duration) -> Self {
        Self {
            transport,
            target,
            timeout,
            settle,
            quiet: Duration::ZERO,
            bank_quiet: Duration::ZERO,
            quiet_limit: None,
            on_stale: None,
        }
    }

    /// Sets the quiet gaps before requests (real hardware; the fake needs none).
    pub fn with_quiet(mut self, quiet: Duration, bank_quiet: Duration) -> Self {
        self.quiet = quiet;
        self.bank_quiet = bank_quiet;
        self
    }

    fn pause(&self) {
        if !self.settle.is_zero() {
            std::thread::sleep(self.settle);
        }
    }

    /// Waits for a quiet line and reports what was dropped. [`exchange`] then finds nothing left.
    /// Fails when the input never goes quiet (see [`wait_quiet`]); nothing has been sent then.
    fn quiet_line(&mut self, quiet: Duration) -> Result<(), CollectError> {
        let limit = self
            .quiet_limit
            .unwrap_or_else(|| default_quiet_limit(quiet));
        let dropped = wait_quiet(&mut self.transport, quiet, limit)?;
        if let Some(report) = self.on_stale.as_mut().filter(|_| dropped > 0) {
            report(dropped);
        }
        Ok(())
    }

    /// Requests the target part's edit buffer (`30 00 pp`) and waits for its dump.
    pub fn fetch(&mut self) -> Result<ReceivedDump, CollectError> {
        let request = messages::fetch_request(&self.target);
        let mut collector = EditBufferCollector::new(self.target.device, self.target.part.raw());
        self.quiet_line(self.quiet)?;
        exchange(
            &mut self.transport,
            Some(&request),
            &mut collector,
            Duration::ZERO,
            self.timeout,
            |_| {},
        )?;
        Ok(collector.into_result().expect("collector is done"))
    }

    /// Sends `data` to the target part's edit buffer (`10 00 pp`).
    pub fn send_edit_buffer(&mut self, data: &[u8; 256]) -> Result<(), String> {
        self.transport
            .send(&messages::edit_buffer_dump(&self.target, data))?;
        self.pause();
        Ok(())
    }

    /// Sends a dump from a file to the target part's edit buffer. A dump whose checksum was
    /// wrong in the file (`checksum_ok == false`, sent because of `--allow-bad-checksum`) keeps
    /// a wrong checksum, so the Virus can still detect the damage.
    pub fn send_edit_buffer_as_stored(
        &mut self,
        data: &[u8; 256],
        checksum_ok: bool,
    ) -> Result<(), String> {
        self.transport.send(&messages::edit_buffer_dump_as_stored(
            &self.target,
            data,
            checksum_ok,
        ))?;
        self.pause();
        Ok(())
    }

    /// Sends one parameter change to the target part.
    pub fn set_param(&mut self, address: ParamAddress, value: u8) -> Result<(), String> {
        self.transport
            .send(&messages::param_change(&self.target, address, value))?;
        self.pause();
        Ok(())
    }

    /// Sends a raw message (`send-hex`) once the line is quiet, like a request.
    pub fn send_raw(&mut self, message: &[u8]) -> Result<(), String> {
        self.quiet_line(self.quiet).map_err(|e| e.to_string())?;
        self.transport.send(message)
    }

    /// Passes every event that arrives within `window` to `on_event`; returns how many arrived.
    pub fn listen(
        &mut self,
        window: Duration,
        on_event: &mut dyn FnMut(&virus_midi::MidiEvent),
    ) -> usize {
        let deadline = std::time::Instant::now() + window;
        let mut count = 0;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return count;
            }
            let Some(event) = self.transport.recv(left) else {
                return count;
            };
            on_event(&event);
            count += 1;
        }
    }

    /// Requests a bank (`32 bb`). Returns the collector even when incomplete, with the error.
    pub fn get_bank(
        &mut self,
        bank: u8,
        mut progress: impl FnMut(usize),
    ) -> (BankCollector, Option<CollectError>) {
        let request = messages::bank_request(&self.target, bank);
        let mut collector = BankCollector::new(self.target.device, bank);
        if let Err(error) = self.quiet_line(self.bank_quiet) {
            return (collector, Some(error));
        }
        let result = exchange(
            &mut self.transport,
            Some(&request),
            &mut collector,
            Duration::ZERO,
            self.timeout,
            |c| progress(c.count()),
        );
        (collector, result.err())
    }

    /// Sends single dumps for `programs` into RAM bank A or B, paced by `gap`. Programs listed
    /// in `bad_checksums` had a wrong checksum in the file and keep a wrong one (as stored).
    pub fn put_bank(
        &mut self,
        bank: u8,
        programs: &[(u8, [u8; 256])],
        bad_checksums: &[u8],
        gap: Duration,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<(), String> {
        if !matches!(bank, 1 | 2) {
            return Err(format!(
                "refusing to send to bank {}: only RAM banks A and B are writable (C/D are ROM)",
                bank_label(bank)
            ));
        }
        let frames = messages::bank_dumps(&self.target, bank, programs, bad_checksums);
        self.transport.send_paced(&frames, gap, progress)
    }

    /// Global request (`35`): the first Virus answer, raw (format undocumented).
    pub fn global(&mut self) -> Result<Vec<u8>, CollectError> {
        let request = messages::global_request(&self.target);
        let mut collector = FirstVirusMessage::new(self.target.device);
        self.quiet_line(self.quiet)?;
        exchange(
            &mut self.transport,
            Some(&request),
            &mut collector,
            Duration::ZERO,
            self.timeout,
            |_| {},
        )?;
        Ok(collector.into_result().expect("collector is done"))
    }
}

/// Offsets where two patches differ, as `A40` / `B36` addresses.
pub fn diff(a: &[u8; 256], b: &[u8; 256]) -> Vec<ParamAddress> {
    (0..256)
        .filter(|&i| a[i] != b[i])
        .map(|i| {
            if i < 128 {
                ParamAddress::a(i as u8)
            } else {
                ParamAddress::b((i - 128) as u8)
            }
        })
        .collect()
}

/// Comma-separated addresses, at most `limit` shown.
pub fn list_addresses(addresses: &[ParamAddress], limit: usize) -> String {
    let mut text = addresses
        .iter()
        .take(limit)
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    if addresses.len() > limit {
        text.push_str(&format!(", … ({} total)", addresses.len()));
    }
    text
}

/// Programs whose received data differs from what was sent.
pub fn verify_bank(sent: &[(u8, [u8; 256])], received: &BankCollector) -> Vec<u8> {
    let dumps = received.dumps();
    sent.iter()
        .filter(|(program, data)| {
            !dumps
                .iter()
                .any(|d| d.program == *program && d.data == *data)
        })
        .map(|(program, _)| *program)
        .collect()
}

/// Received dumps in the form `virus-db` imports.
pub fn to_incoming(dumps: &[&ReceivedDump]) -> Vec<IncomingDump> {
    dumps
        .iter()
        .map(|d| IncomingDump {
            bank: d.bank,
            program: d.program,
            data: d.data,
            checksum_ok: d.checksum_ok,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{pattern, synth, FakeVirus};
    use crate::settings::{DeviceId, Part};
    use virus_syx::Page;

    #[test]
    fn fetch_set_and_send_edit_buffer() {
        let mut s = synth(FakeVirus::new(0));
        let original = s.fetch().unwrap();
        assert!(original.checksum_ok);
        s.set_param(ParamAddress::a(40), 12).unwrap();
        let changed = s.fetch().unwrap();
        assert_eq!(
            diff(&original.data, &changed.data),
            vec![ParamAddress::a(40)]
        );
        s.send_edit_buffer(&original.data).unwrap();
        assert_eq!(s.fetch().unwrap().data, original.data);
    }

    #[test]
    fn fetch_addresses_the_configured_multi_part_and_device() {
        let mut s = synth(FakeVirus::new(3));
        assert!(matches!(s.fetch(), Err(CollectError::Timeout { .. })));
        s.target = Target {
            device: DeviceId(3),
            part: Part::Multi(4),
            channel: 0,
        };
        assert_eq!(s.fetch().unwrap().data, pattern(4));
    }

    #[test]
    fn get_bank_collects_all_programs_and_put_bank_refuses_rom() {
        let mut s = synth(FakeVirus::new(0));
        let mut seen = 0;
        let (bank, error) = s.get_bank(3, |n| seen = n);
        assert!(error.is_none());
        assert_eq!((bank.count(), seen), (128, 128));
        assert_eq!(bank.dumps()[5].data, pattern(3 * 31 + 5));

        let programs: Vec<(u8, [u8; 256])> = (0..128u8).map(|p| (p, pattern(p))).collect();
        let err = s
            .put_bank(3, &programs, &[], Duration::ZERO, &mut |_, _| {})
            .unwrap_err();
        assert!(err.contains("ROM"));
        assert!(s.transport.received.iter().all(|m| m[6] != 0x10));

        let mut sent = 0;
        s.put_bank(2, &programs, &[], Duration::ZERO, &mut |n, _| sent = n)
            .unwrap();
        assert_eq!(sent, 128);
        let (bank_b, error) = s.get_bank(2, |_| {});
        assert!(error.is_none());
        assert!(verify_bank(&programs, &bank_b).is_empty());
        let mut altered = programs.clone();
        altered[7].1[Page::PageB.dump_offset().unwrap() + 1] ^= 1;
        assert_eq!(verify_bank(&altered, &bank_b), vec![7]);
    }

    #[test]
    fn incomplete_bank_returns_partial_result_and_timeout() {
        let mut s = synth(FakeVirus::new(0));
        let (bank, error) = s.get_bank(5, |_| {});
        assert_eq!(bank.count(), 0);
        assert!(matches!(error, Some(CollectError::Timeout { .. })));
    }

    #[test]
    fn global_request_returns_raw_answer() {
        let mut s = synth(FakeVirus::new(0));
        assert_eq!(s.global().unwrap()[6], 0x12);
    }

    #[test]
    fn page_c_set_goes_to_the_target_part() {
        let mut s = synth(FakeVirus::new(0));
        s.set_param(ParamAddress::c(32), 1).unwrap();
        s.target = Target {
            part: Part::Multi(4),
            ..s.target
        };
        s.set_param(ParamAddress::c(33), 17).unwrap();
        let sent: Vec<&[u8]> = s.transport.received.iter().map(|m| &m[6..10]).collect();
        assert_eq!(sent, [&[0x72, 0x40, 32, 1][..], &[0x72, 0x04, 33, 17][..]]);
    }

    #[test]
    fn raw_send_waits_for_quiet_and_answers_can_be_listened_to() {
        let mut s = synth(FakeVirus::new(0));
        let stale =
            virus_midi::MidiEvent::SysEx(virus_syx::encode_program_dump(0, 1, 9, &pattern(9)));
        s.transport.push_stale(stale);
        let request = crate::messages::parse_hex_sysex("F0 00 20 33 01 00 30 00 40 F7").unwrap();
        s.send_raw(&request).unwrap();
        assert_eq!(s.transport.received, vec![request]);
        let mut answers = Vec::new();
        let n = s.listen(Duration::from_millis(20), &mut |e| answers.push(e.clone()));
        assert_eq!(n, 1, "only the answer, not the stale dump");
        match &answers[0] {
            virus_midi::MidiEvent::SysEx(frame) => assert_eq!(frame[6..9], [0x10, 0x00, 0x40]),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn stale_messages_dropped_before_a_request_are_reported() {
        let reports = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut s = synth(FakeVirus::new(0));
        let sink = reports.clone();
        s.on_stale = Some(Box::new(move |n| sink.borrow_mut().push(n)));

        s.fetch().unwrap();
        assert!(
            reports.borrow().is_empty(),
            "nothing is reported when the line was already quiet"
        );

        for program in 70..73u8 {
            s.transport.push_stale(virus_midi::MidiEvent::SysEx(
                virus_syx::encode_program_dump(0, 1, program, &pattern(program)),
            ));
        }
        let (bank, error) = s.get_bank(1, |_| {});
        assert!(error.is_none());
        assert_eq!(
            bank.started_mid_stream(),
            None,
            "the stale tail was not mistaken for the answer"
        );
        assert_eq!(*reports.borrow(), vec![3]);
    }

    #[test]
    fn address_lists_are_limited() {
        let all: Vec<ParamAddress> = (0..5).map(ParamAddress::a).collect();
        assert_eq!(list_addresses(&all, 2), "A0, A1, … (5 total)");
    }

    /// The checksum verdict of a single dump the fake received.
    fn sent_checksum_ok(frame: &[u8]) -> bool {
        match virus_syx::parse_sysex(frame).unwrap() {
            virus_syx::SysExMessage::SingleDump { checksum_ok, .. } => checksum_ok,
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn a_dump_with_a_bad_checksum_in_the_file_is_sent_with_a_bad_checksum() {
        let mut s = synth(FakeVirus::new(0));
        s.send_edit_buffer_as_stored(&pattern(9), false).unwrap();
        let frame = s.transport.received.last().unwrap();
        assert_eq!(
            frame[6..9],
            [0x10, 0x00, 0x40],
            "addressed to the single edit buffer"
        );
        assert!(
            !sent_checksum_ok(frame),
            "the Virus must still be able to see the damage"
        );
        assert_eq!(
            s.transport.edit_buffers[&0x40],
            pattern(9),
            "the data is sent unchanged"
        );
    }

    #[test]
    fn a_dump_with_a_good_checksum_in_the_file_is_sent_with_a_good_checksum() {
        let mut s = synth(FakeVirus::new(0));
        s.send_edit_buffer_as_stored(&pattern(9), true).unwrap();
        assert!(sent_checksum_ok(s.transport.received.last().unwrap()));
    }

    #[test]
    fn put_bank_keeps_wrong_checksums_only_for_the_programs_flagged_in_the_file() {
        let mut s = synth(FakeVirus::new(0));
        let programs: Vec<(u8, [u8; 256])> = (0..128u8).map(|p| (p, pattern(p))).collect();
        s.put_bank(1, &programs, &[3, 70], Duration::ZERO, &mut |_, _| {})
            .unwrap();
        let bad: Vec<u8> = s
            .transport
            .received
            .iter()
            .filter(|f| !sent_checksum_ok(f))
            .map(|f| f[8])
            .collect();
        assert_eq!(bad, [3, 70]);
        assert_eq!(s.transport.received.len(), 128);
    }

    /// A synth whose input never goes quiet, with a short quiet-wait limit.
    fn chattering_synth() -> Synth<FakeVirus> {
        let mut fake = FakeVirus::new(0);
        fake.chatter = true;
        let mut s = synth(fake).with_quiet(Duration::from_millis(5), Duration::from_millis(5));
        s.quiet_limit = Some(Duration::from_millis(30));
        s
    }

    #[test]
    fn fetch_gives_up_when_the_input_never_goes_quiet_and_sends_nothing() {
        let mut s = chattering_synth();
        let err = s.fetch().unwrap_err();
        match &err {
            CollectError::NeverQuiet { dropped, waited } => {
                assert!(*dropped > 0);
                assert!(*waited >= Duration::from_millis(30));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(err.to_string().contains("never went quiet"), "{err}");
        assert!(s.transport.received.is_empty(), "no request on a busy line");
    }

    #[test]
    fn get_bank_gives_up_when_the_input_never_goes_quiet_and_sends_nothing() {
        let mut s = chattering_synth();
        let (bank, error) = s.get_bank(1, |_| {});
        assert_eq!(bank.count(), 0);
        assert!(
            matches!(error, Some(CollectError::NeverQuiet { .. })),
            "{error:?}"
        );
        assert!(s.transport.received.is_empty(), "no request on a busy line");
    }

    #[test]
    fn send_raw_and_global_give_up_when_the_input_never_goes_quiet() {
        let mut s = chattering_synth();
        assert!(s
            .send_raw(&[0xF0, 0x00, 0x20, 0x33, 0x01, 0x00, 0x35, 0xF7])
            .unwrap_err()
            .contains("never went quiet"));
        assert!(matches!(s.global(), Err(CollectError::NeverQuiet { .. })));
        assert!(
            s.transport.received.is_empty(),
            "nothing is sent on a busy line"
        );
    }
}

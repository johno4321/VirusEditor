//! Synth operations built on a [`Transport`]: fetch, send, set, bank transfer. They print
//! nothing; progress is reported through callbacks so they run against the fake Virus in tests.

use std::time::Duration;

use virus_db::IncomingDump;

use crate::address::ParamAddress;
use crate::messages;
use crate::settings::{bank_label, Target};
use crate::transport::{
    exchange, wait_quiet, BankCollector, CollectError, EditBufferCollector, FirstVirusMessage, ReceivedDump,
    Transport,
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
    /// Called with the number of stale messages dropped while waiting for a quiet line (only
    /// when some were dropped); `--verbose` prints it.
    pub on_stale: Option<Box<dyn FnMut(usize)>>,
}

impl<T: Transport> Synth<T> {
    pub fn new(transport: T, target: Target, timeout: Duration, settle: Duration) -> Self {
        Self { transport, target, timeout, settle, quiet: Duration::ZERO, bank_quiet: Duration::ZERO, on_stale: None }
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
    fn quiet_line(&mut self, quiet: Duration) {
        let dropped = wait_quiet(&mut self.transport, quiet);
        if let Some(report) = self.on_stale.as_mut().filter(|_| dropped > 0) {
            report(dropped);
        }
    }

    /// Requests the target part's edit buffer (`30 00 pp`) and waits for its dump.
    pub fn fetch(&mut self) -> Result<ReceivedDump, CollectError> {
        let request = messages::fetch_request(&self.target);
        let mut collector = EditBufferCollector::new(self.target.device, self.target.part.raw());
        self.quiet_line(self.quiet);
        exchange(&mut self.transport, Some(&request), &mut collector, Duration::ZERO, self.timeout, |_| {})?;
        Ok(collector.into_result().expect("collector is done"))
    }

    /// Sends `data` to the target part's edit buffer (`10 00 pp`).
    pub fn send_edit_buffer(&mut self, data: &[u8; 256]) -> Result<(), String> {
        self.transport.send(&messages::edit_buffer_dump(&self.target, data))?;
        self.pause();
        Ok(())
    }

    /// Sends one parameter change to the target part.
    pub fn set_param(&mut self, address: ParamAddress, value: u8) -> Result<(), String> {
        self.transport.send(&messages::param_change(&self.target, address, value))?;
        self.pause();
        Ok(())
    }

    /// Requests a bank (`32 bb`). Returns the collector even when incomplete, with the error.
    pub fn get_bank(
        &mut self,
        bank: u8,
        mut progress: impl FnMut(usize),
    ) -> (BankCollector, Option<CollectError>) {
        let request = messages::bank_request(&self.target, bank);
        let mut collector = BankCollector::new(self.target.device, bank);
        self.quiet_line(self.bank_quiet);
        let result = exchange(&mut self.transport, Some(&request), &mut collector, Duration::ZERO, self.timeout, |c| {
            progress(c.count())
        });
        (collector, result.err())
    }

    /// Sends single dumps for `programs` into RAM bank A or B, paced by `gap`.
    pub fn put_bank(
        &mut self,
        bank: u8,
        programs: &[(u8, [u8; 256])],
        gap: Duration,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<(), String> {
        if !matches!(bank, 1 | 2) {
            return Err(format!(
                "refusing to send to bank {}: only RAM banks A and B are writable (C/D are ROM)",
                bank_label(bank)
            ));
        }
        let frames = messages::bank_dumps(&self.target, bank, programs);
        self.transport.send_paced(&frames, gap, progress)
    }

    /// Global request (`35`): the first Virus answer, raw (format undocumented).
    pub fn global(&mut self) -> Result<Vec<u8>, CollectError> {
        let request = messages::global_request(&self.target);
        let mut collector = FirstVirusMessage::new(self.target.device);
        self.quiet_line(self.quiet);
        exchange(&mut self.transport, Some(&request), &mut collector, Duration::ZERO, self.timeout, |_| {})?;
        Ok(collector.into_result().expect("collector is done"))
    }
}

/// Offsets where two patches differ, as `A40` / `B36` addresses.
pub fn diff(a: &[u8; 256], b: &[u8; 256]) -> Vec<ParamAddress> {
    (0..256)
        .filter(|&i| a[i] != b[i])
        .map(|i| if i < 128 { ParamAddress::a(i as u8) } else { ParamAddress::b((i - 128) as u8) })
        .collect()
}

/// Comma-separated addresses, at most `limit` shown.
pub fn list_addresses(addresses: &[ParamAddress], limit: usize) -> String {
    let mut text = addresses.iter().take(limit).map(ToString::to_string).collect::<Vec<_>>().join(", ");
    if addresses.len() > limit {
        text.push_str(&format!(", … ({} total)", addresses.len()));
    }
    text
}

/// Programs whose received data differs from what was sent.
pub fn verify_bank(sent: &[(u8, [u8; 256])], received: &BankCollector) -> Vec<u8> {
    let dumps = received.dumps();
    sent.iter()
        .filter(|(program, data)| !dumps.iter().any(|d| d.program == *program && d.data == *data))
        .map(|(program, _)| *program)
        .collect()
}

/// Received dumps in the form `virus-db` imports.
pub fn to_incoming(dumps: &[&ReceivedDump]) -> Vec<IncomingDump> {
    dumps
        .iter()
        .map(|d| IncomingDump { bank: d.bank, program: d.program, data: d.data, checksum_ok: d.checksum_ok })
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
        assert_eq!(diff(&original.data, &changed.data), vec![ParamAddress::a(40)]);
        s.send_edit_buffer(&original.data).unwrap();
        assert_eq!(s.fetch().unwrap().data, original.data);
    }

    #[test]
    fn fetch_addresses_the_configured_multi_part_and_device() {
        let mut s = synth(FakeVirus::new(3));
        assert!(matches!(s.fetch(), Err(CollectError::Timeout { .. })));
        s.target = Target { device: DeviceId(3), part: Part::Multi(4), channel: 0 };
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
        let err = s.put_bank(3, &programs, Duration::ZERO, &mut |_, _| {}).unwrap_err();
        assert!(err.contains("ROM"));
        assert!(s.transport.received.iter().all(|m| m[6] != 0x10));

        let mut sent = 0;
        s.put_bank(2, &programs, Duration::ZERO, &mut |n, _| sent = n).unwrap();
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
    fn stale_messages_dropped_before_a_request_are_reported() {
        let reports = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut s = synth(FakeVirus::new(0));
        let sink = reports.clone();
        s.on_stale = Some(Box::new(move |n| sink.borrow_mut().push(n)));

        s.fetch().unwrap();
        assert!(reports.borrow().is_empty(), "nothing is reported when the line was already quiet");

        for program in 70..73u8 {
            s.transport.push_stale(virus_midi::MidiEvent::SysEx(virus_syx::encode_program_dump(0, 1, program, &pattern(program))));
        }
        let (bank, error) = s.get_bank(1, |_| {});
        assert!(error.is_none());
        assert_eq!(bank.started_mid_stream(), None, "the stale tail was not mistaken for the answer");
        assert_eq!(*reports.borrow(), vec![3]);
    }

    #[test]
    fn address_lists_are_limited() {
        let all: Vec<ParamAddress> = (0..5).map(ParamAddress::a).collect();
        assert_eq!(list_addresses(&all, 2), "A0, A1, … (5 total)");
    }
}

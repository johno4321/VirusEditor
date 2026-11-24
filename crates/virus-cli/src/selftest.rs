//! The automatable part of the G2 live-test checklist, as a pass/fail report.

use std::fmt;

use virus_syx::Patch;

use crate::address::ParamAddress;
use crate::messages::hex;
use crate::ops::{diff, list_addresses, Synth};
use crate::settings::bank_label;
use crate::transport::{ReceivedDump, Transport};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pass,
    Fail,
    Skip,
    Info,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Skip => "SKIP",
            Self::Info => "INFO",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub status: Status,
    pub detail: String,
}

impl Check {
    fn new(name: impl Into<String>, status: Status, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status,
            detail: detail.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelftestOptions {
    /// Bank to request (`32 bb`); `None` skips the bank step.
    pub bank: Option<u8>,
    pub global_request: bool,
}

/// Parameters used for the round trip: one per page, small audible changes.
pub const ROUND_TRIP_PARAMS: [ParamAddress; 2] = [ParamAddress::a(40), ParamAddress::b(36)];

/// Runs the checks in order. Steps that need the fetched edit buffer are skipped when the
/// fetch fails. The edit buffer is left as it was fetched.
pub fn run_selftest<T: Transport>(
    synth: &mut Synth<T>,
    options: &SelftestOptions,
    progress: &mut dyn FnMut(&Check),
) -> Vec<Check> {
    let mut checks = Vec::new();
    let mut push = |check: Check, checks: &mut Vec<Check>| {
        progress(&check);
        checks.push(check);
    };

    let original = match synth.fetch() {
        Ok(dump) => {
            let patch = Patch::from_bytes(&dump.data);
            let status = if dump.checksum_ok {
                Status::Pass
            } else {
                Status::Fail
            };
            push(
                Check::new(
                    "fetch edit buffer (30 00 pp)",
                    status,
                    format!(
                        "'{}', sound version {}, checksum {}, device 0x{:02X}",
                        patch.name(),
                        patch.sound_version(),
                        if dump.checksum_ok { "ok" } else { "WRONG" },
                        dump.device_id
                    ),
                ),
                &mut checks,
            );
            Some(dump)
        }
        Err(e) => {
            push(
                Check::new("fetch edit buffer (30 00 pp)", Status::Fail, e.to_string()),
                &mut checks,
            );
            None
        }
    };

    match &original {
        Some(original) => {
            for address in ROUND_TRIP_PARAMS {
                let check = param_round_trip(synth, original, address);
                push(check, &mut checks);
            }
            let check = edit_buffer_round_trip(synth, original);
            push(check, &mut checks);
        }
        None => {
            for address in ROUND_TRIP_PARAMS {
                push(
                    Check::new(round_trip_name(address), Status::Skip, "no edit buffer"),
                    &mut checks,
                );
            }
            push(
                Check::new(SEND_NAME, Status::Skip, "no edit buffer"),
                &mut checks,
            );
        }
    }

    match options.bank {
        Some(bank) => {
            let name = format!("bank request (32 {bank:02X}): bank {}", bank_label(bank));
            let (collector, error) = synth.get_bank(bank, |_| {});
            let bad = collector.bad_checksums();
            let check = match error {
                None if bad.is_empty() => {
                    Check::new(name, Status::Pass, "128 dumps, all checksums ok")
                }
                None => Check::new(
                    name,
                    Status::Fail,
                    format!("128 dumps, wrong checksums in programs {bad:?}"),
                ),
                Some(e) => Check::new(name, Status::Fail, e.to_string()),
            };
            push(check, &mut checks);
        }
        None => push(
            Check::new("bank request (32 bb)", Status::Skip, "--skip-bank"),
            &mut checks,
        ),
    }

    if options.global_request {
        let check = match synth.global() {
            Ok(frame) => Check::new(
                "global request (35)",
                Status::Info,
                format!(
                    "answered with command 0x{:02X}, {} bytes: {}…",
                    frame.get(6).copied().unwrap_or(0),
                    frame.len(),
                    hex(&frame[..frame.len().min(16)])
                ),
            ),
            Err(e) => Check::new(
                "global request (35)",
                Status::Info,
                format!("no answer ({e})"),
            ),
        };
        push(check, &mut checks);
    }
    checks
}

fn round_trip_name(address: ParamAddress) -> String {
    format!(
        "param change {address} {} (7{}) → read back",
        address.name(),
        if address.offset() < 128 { 0 } else { 1 }
    )
}

const SEND_NAME: &str = "send edit buffer (10 00 pp) → read back identical";

/// Changes `address` by one step, checks only that byte changed, then puts it back and checks
/// the whole edit buffer is the original again.
fn param_round_trip<T: Transport>(
    synth: &mut Synth<T>,
    original: &ReceivedDump,
    address: ParamAddress,
) -> Check {
    let name = round_trip_name(address);
    let old = original.data[address.offset()];
    let new = if old < 127 { old + 1 } else { old - 1 };
    let result = (|| -> Result<String, String> {
        synth.set_param(address, new)?;
        let changed = synth.fetch().map_err(|e| e.to_string())?;
        let differences = diff(&original.data, &changed.data);
        let read = changed.data[address.offset()];
        synth.set_param(address, old)?;
        if read != new {
            return Err(format!("sent {address} = {new} but read back {read}"));
        }
        if differences != [address] {
            return Err(format!(
                "after {address} {old}→{new} the changed bytes were [{}]",
                list_addresses(&differences, 8)
            ));
        }
        let back = synth.fetch().map_err(|e| e.to_string())?;
        let differences = diff(&original.data, &back.data);
        if !differences.is_empty() {
            return Err(format!(
                "after restoring {address} bytes differ: {}",
                list_addresses(&differences, 8)
            ));
        }
        Ok(format!("{old}→{new}→{old}, only {address} changed"))
    })();
    match result {
        Ok(detail) => Check::new(name, Status::Pass, detail),
        Err(detail) => Check::new(name, Status::Fail, detail),
    }
}

fn edit_buffer_round_trip<T: Transport>(synth: &mut Synth<T>, original: &ReceivedDump) -> Check {
    let result = (|| -> Result<(), String> {
        synth.send_edit_buffer(&original.data)?;
        let back = synth.fetch().map_err(|e| e.to_string())?;
        let differences = diff(&original.data, &back.data);
        if differences.is_empty() {
            Ok(())
        } else {
            Err(format!("bytes differ: {}", list_addresses(&differences, 8)))
        }
    })();
    match result {
        Ok(()) => Check::new(SEND_NAME, Status::Pass, "byte-identical"),
        Err(detail) => Check::new(SEND_NAME, Status::Fail, detail),
    }
}

/// G2 items that need ears, eyes or the GUI.
pub const MANUAL_CHECKS: &[&str] = &[
    "turn a knob on the Virus while `virus monitor` runs: CC (Low Page = Contr) or SysEx param changes appear with the right names",
    "`virus set A40 20` then `virus set A40 100`: the cutoff change is audible",
    "Multi part addressing: `virus fetch --part 1` in Multi mode returns part 1's sound",
    "back up, then `virus put-bank A <file> --yes --verify`: no buffer overflow, no checksum error on the display",
];

pub fn format_report(checks: &[Check]) -> String {
    let mut out = String::new();
    for check in checks {
        out.push_str(&format!(
            "[{}] {}\n       {}\n",
            check.status, check.name, check.detail
        ));
    }
    let failed = checks.iter().filter(|c| c.status == Status::Fail).count();
    let passed = checks.iter().filter(|c| c.status == Status::Pass).count();
    out.push_str(&format!(
        "\n{passed} passed, {failed} failed\n\nStill to check by hand (G2):\n"
    ));
    for item in MANUAL_CHECKS {
        out.push_str(&format!("  - {item}\n"));
    }
    out
}

pub fn all_passed(checks: &[Check]) -> bool {
    checks.iter().all(|c| c.status != Status::Fail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{synth, FakeVirus};
    use virus_syx::Page;

    fn options() -> SelftestOptions {
        SelftestOptions {
            bank: Some(1),
            global_request: true,
        }
    }

    #[test]
    fn selftest_passes_against_a_well_behaved_synth_and_leaves_the_buffer() {
        let mut s = synth(FakeVirus::new(0));
        let before = s.fetch().unwrap().data;
        let checks = run_selftest(&mut s, &options(), &mut |_| {});
        assert_eq!(checks.len(), 6, "{checks:#?}");
        assert!(all_passed(&checks), "{}", format_report(&checks));
        assert_eq!(checks[5].status, Status::Info);
        assert_eq!(s.fetch().unwrap().data, before);
        assert!(format_report(&checks).contains("5 passed, 0 failed"));
    }

    #[test]
    fn selftest_fails_and_skips_when_the_synth_does_not_answer() {
        let mut fake = FakeVirus::new(0);
        fake.silent = true;
        let mut s = synth(fake);
        let checks = run_selftest(
            &mut s,
            &SelftestOptions {
                bank: None,
                global_request: false,
            },
            &mut |_| {},
        );
        let statuses: Vec<Status> = checks.iter().map(|c| c.status).collect();
        assert_eq!(
            statuses,
            [
                Status::Fail,
                Status::Skip,
                Status::Skip,
                Status::Skip,
                Status::Skip
            ]
        );
        assert!(!all_passed(&checks));
    }

    #[test]
    fn round_trip_detects_a_synth_that_clamps_the_value() {
        let mut fake = FakeVirus::new(0);
        fake.max_values.insert((Page::PageB, 36), 0);
        let mut s = synth(fake);
        let original = s.fetch().unwrap();
        let mut data = original.data;
        data[128 + 36] = 50;
        s.send_edit_buffer(&data).unwrap();
        let original = s.fetch().unwrap();
        let check = param_round_trip(&mut s, &original, ParamAddress::b(36));
        assert_eq!(check.status, Status::Fail, "{check:?}");
        assert!(check.detail.contains("read back 0"));
        let original = s.fetch().unwrap();
        assert_eq!(
            param_round_trip(&mut s, &original, ParamAddress::a(40)).status,
            Status::Pass
        );
    }
}

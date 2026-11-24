//! Guided probes for open questions Q1 (destination names) and Q3 (value lists).
//!
//! A probe writes successive raw values to one parameter of the edit buffer with SysEx
//! parameter changes, reads the edit buffer back to see whether the synth kept the value,
//! and (interactively) records what the Virus display shows. The edit buffer is fetched
//! first and restored at the end. Results are saved as JSON after every step.

use std::collections::BTreeMap;
use std::io::BufRead;

use serde::{Deserialize, Serialize};
use virus_syx::{ValueKind, UNVERIFIED_ASSIGN_DESTINATION_NAMES, UNVERIFIED_DEFINABLE_DESTINATION_NAMES};

use crate::address::ParamAddress;
use crate::ops::{diff, list_addresses, Synth};
use crate::transport::{ReceivedDump, Transport};

/// Which destination parameter the Q1 probe writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum DestinationSlot {
    /// Assign 1 destination (B65), source = ModWheel (B64 = 3), amount = +63 (B66 = 127).
    Assign1,
    /// LFO1 assign destination (B79), amount = +63 (B80 = 127).
    Lfo1,
    /// Definable 1 (Single) destination (B62).
    Definable1,
}

/// What a probe does, independent of any I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbePlan {
    /// `"q1-destinations"` or `"q3-values"`.
    pub kind: &'static str,
    pub label: String,
    pub param: ParamAddress,
    /// Parameter changes sent once before probing.
    pub setup: Vec<(ParamAddress, u8)>,
    pub values: Vec<u8>,
    /// Names printed in the manuals for raw value `n`, shown as unverified hints.
    pub hints: &'static [&'static str],
    /// What the user should look at on the Virus display.
    pub look_at: String,
}

impl ProbePlan {
    pub fn hint(&self, value: u8) -> Option<&'static str> {
        self.hints.get(value as usize).copied()
    }
}

/// The Q1 plan for one destination slot over `values`.
pub fn destination_plan(slot: DestinationSlot, values: Vec<u8>) -> ProbePlan {
    let (label, param, setup, hints, look_at) = match slot {
        DestinationSlot::Assign1 => (
            "assign1",
            ParamAddress::b(65),
            vec![(ParamAddress::b(64), 3), (ParamAddress::b(66), 127)],
            UNVERIFIED_ASSIGN_DESTINATION_NAMES,
            "the Assign 1 destination (Mod Matrix slot 1, source ModWheel)",
        ),
        DestinationSlot::Lfo1 => (
            "lfo1",
            ParamAddress::b(79),
            vec![(ParamAddress::b(80), 127)],
            UNVERIFIED_ASSIGN_DESTINATION_NAMES,
            "the LFO 1 assign destination",
        ),
        DestinationSlot::Definable1 => (
            "definable1",
            ParamAddress::b(62),
            Vec::new(),
            UNVERIFIED_DEFINABLE_DESTINATION_NAMES,
            "the Definable 1 (Single) destination",
        ),
    };
    ProbePlan {
        kind: "q1-destinations",
        label: label.to_owned(),
        param,
        setup,
        values,
        hints,
        look_at: look_at.to_owned(),
    }
}

/// Suggested `--to` for the Q3 parameters: a little past the highest value seen in the
/// factory data or documented range (SPEC §4.1–4.2, §7).
pub fn suggested_max(address: ParamAddress) -> u8 {
    match (address.page, address.number) {
        (virus_syx::Page::PageA, 49) => 40,
        (virus_syx::Page::PageA, 112) => 40,
        (virus_syx::Page::PageA, 102) => 15,
        (virus_syx::Page::PageB, 39) => 20,
        _ => 127,
    }
}

/// The Q3 plan for one parameter over `values`. Refuses A0 and non-sound parameters.
pub fn value_plan(address: ParamAddress, values: Vec<u8>) -> Result<ProbePlan, String> {
    if let Some(reason) = address.edit_warning() {
        return Err(format!("{address} is {reason}; only documented sound parameters can be probed"));
    }
    let hints: &'static [&'static str] = match address.def().map(|d| d.kind) {
        Some(ValueKind::Choice(texts)) => texts,
        _ => &[],
    };
    Ok(ProbePlan {
        kind: "q3-values",
        label: address.to_string(),
        param: address,
        setup: Vec::new(),
        values,
        hints,
        look_at: format!("{address} {}", address.name()),
    })
}

/// Inclusive range as a value list.
pub fn range(from: u8, to: u8) -> Result<Vec<u8>, String> {
    if from > to || to > 127 {
        return Err(format!("invalid range {from}..={to} (0..=127, from <= to)"));
    }
    Ok((from..=to).collect())
}

/// One probed value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeRecord {
    pub value: u8,
    /// The byte read back from the edit buffer after the change (`None` without read-back).
    pub readback: Option<u8>,
    /// `readback == value`; `false` suggests the synth clamped/rejected the value.
    pub accepted: Option<bool>,
    /// What the user read on the Virus display.
    pub display: Option<String>,
    /// The (unverified) printed name for this raw value.
    pub printed_hint: Option<String>,
    /// Other bytes that changed, or other remarks.
    pub note: Option<String>,
}

/// The JSON file written by a probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeFile {
    pub kind: String,
    pub label: String,
    pub param: String,
    pub device_id: u8,
    pub part: u8,
    pub setup: Vec<String>,
    pub results: Vec<ProbeRecord>,
}

impl ProbeFile {
    pub fn new(plan: &ProbePlan, device_id: u8, part: u8) -> Self {
        Self {
            kind: plan.kind.to_owned(),
            label: plan.label.clone(),
            param: plan.param.to_string(),
            device_id,
            part,
            setup: plan.setup.iter().map(|(a, v)| format!("{a}={v}")).collect(),
            results: Vec::new(),
        }
    }

    /// Inserts or replaces the record for its value, keeping results sorted by value.
    pub fn upsert(&mut self, record: ProbeRecord) {
        match self.results.binary_search_by_key(&record.value, |r| r.value) {
            Ok(i) => self.results[i] = record,
            Err(i) => self.results.insert(i, record),
        }
    }

    pub fn get(&self, value: u8) -> Option<&ProbeRecord> {
        self.results.iter().find(|r| r.value == value)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("probe file serialises")
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("not a probe results file: {e}"))
    }

    /// A Rust array of display names indexed by raw value, for turning verified results into
    /// a name table. Values without a display reading become `"#n"`; the array ends at the
    /// last accepted value.
    pub fn rust_table(&self, const_name: &str) -> String {
        let by_value: BTreeMap<u8, &ProbeRecord> = self.results.iter().map(|r| (r.value, r)).collect();
        let last = self
            .results
            .iter()
            .filter(|r| r.accepted != Some(false) && (r.display.is_some() || r.accepted == Some(true)))
            .map(|r| r.value)
            .max();
        let mut out = format!(
            "/// Verified on the synth: {} {} (probe '{}').\npub const {const_name}: &[&str] = &[\n",
            self.kind, self.param, self.label
        );
        if let Some(last) = last {
            for value in 0..=last {
                let text = by_value
                    .get(&value)
                    .and_then(|r| r.display.clone())
                    .unwrap_or_else(|| format!("#{value}"));
                out.push_str(&format!("    {:?}, // {value}\n", text));
            }
        }
        out.push_str("];\n");
        out
    }
}

/// The user's answer to "what does the display show?".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Text(String),
    /// Record no display reading for this value.
    Skip,
    /// Send the value again (e.g. the display didn't refresh) and ask again.
    Repeat,
    /// Stop probing (results so far are kept; the edit buffer is restored).
    Quit,
}

pub trait Prompter {
    fn ask(&mut self, question: &str) -> Answer;
}

/// Interprets one typed line: empty = skip, `r` = resend, `q` = quit, anything else = text.
pub fn parse_answer(line: &str) -> Answer {
    match line.trim() {
        "" => Answer::Skip,
        "r" | "R" => Answer::Repeat,
        "q" | "Q" => Answer::Quit,
        text => Answer::Text(text.to_owned()),
    }
}

/// Reads answers from a line-based input (stdin); end of input quits.
pub struct LinePrompter<R> {
    input: R,
}

impl<R: BufRead> LinePrompter<R> {
    pub fn new(input: R) -> Self {
        Self { input }
    }
}

impl<R: BufRead> Prompter for LinePrompter<R> {
    fn ask(&mut self, question: &str) -> Answer {
        use std::io::Write;
        print!("{question}");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        match self.input.read_line(&mut line) {
            Ok(0) | Err(_) => Answer::Quit,
            Ok(_) => parse_answer(&line),
        }
    }
}

/// How a probe runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeOptions {
    /// Ask the user for the display reading after each value.
    pub interactive: bool,
    /// Read the edit buffer back after each value.
    pub readback: bool,
    /// Stop after this many consecutive values that didn't read back (0 = never).
    pub stop_after_rejected: u32,
}

/// Progress messages for the caller to print.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeEvent {
    Original { name: String },
    Skipped { value: u8 },
    Recorded(ProbeRecord),
    Stopped(String),
    Restored { identical: bool, differences: String },
}

/// The outcome of a probe run. The edit buffer has been restored (or the restore failed,
/// see `restore_error`); `error` is set when probing stopped because of an error.
#[derive(Debug)]
pub struct ProbeOutcome {
    pub file: ProbeFile,
    pub original: ReceivedDump,
    pub error: Option<String>,
    pub restore_error: Option<String>,
}

/// Runs `plan`. `file` holds results to resume from (values with a display reading, or any
/// record in non-interactive mode, are skipped). `on_original` is called with the fetched
/// edit buffer before anything is changed (save a backup there); `save` after every record.
#[allow(clippy::too_many_arguments)]
pub fn run_probe<T: Transport>(
    synth: &mut Synth<T>,
    plan: &ProbePlan,
    mut file: ProbeFile,
    options: &ProbeOptions,
    prompter: &mut dyn Prompter,
    on_original: &mut dyn FnMut(&ReceivedDump) -> Result<(), String>,
    save: &mut dyn FnMut(&ProbeFile),
    report: &mut dyn FnMut(ProbeEvent),
) -> Result<ProbeOutcome, String> {
    let original = synth.fetch().map_err(|e| format!("could not fetch the edit buffer first: {e}"))?;
    report(ProbeEvent::Original { name: virus_syx::Patch::from_bytes(&original.data).name() });
    on_original(&original)?;

    let error = probe_values(synth, plan, &mut file, options, prompter, &original, save, report).err();

    let restore_error = restore(synth, &original, report).err();
    Ok(ProbeOutcome { file, original, error, restore_error })
}

#[allow(clippy::too_many_arguments)]
fn probe_values<T: Transport>(
    synth: &mut Synth<T>,
    plan: &ProbePlan,
    file: &mut ProbeFile,
    options: &ProbeOptions,
    prompter: &mut dyn Prompter,
    original: &ReceivedDump,
    save: &mut dyn FnMut(&ProbeFile),
    report: &mut dyn FnMut(ProbeEvent),
) -> Result<(), String> {
    for &(address, value) in &plan.setup {
        synth.set_param(address, value)?;
    }
    // What the edit buffer should hold if only the probed byte changes.
    let mut expected = original.data;
    for &(address, value) in &plan.setup {
        expected[address.offset()] = value;
    }
    let mut rejected_in_a_row = 0;

    for &value in &plan.values {
        let done = file.get(value).is_some_and(|r| r.display.is_some() || !options.interactive);
        if done {
            report(ProbeEvent::Skipped { value });
            continue;
        }
        let mut record = ProbeRecord {
            value,
            readback: None,
            accepted: None,
            display: None,
            printed_hint: plan.hint(value).map(str::to_owned),
            note: None,
        };
        loop {
            synth.set_param(plan.param, value)?;
            if options.readback {
                let dump = synth.fetch().map_err(|e| format!("read-back of value {value} failed: {e}"))?;
                let readback = dump.data[plan.param.offset()];
                record.readback = Some(readback);
                record.accepted = Some(readback == value);
                let mut others = diff(&expected, &dump.data);
                others.retain(|a| *a != plan.param);
                record.note = (!others.is_empty())
                    .then(|| format!("other bytes changed: {}", list_addresses(&others, 8)));
                expected = dump.data;
            }
            if !options.interactive {
                break;
            }
            let question = format!(
                "{} = {value}{}{} — display shows? [text / Enter=skip / r=resend / q=quit] ",
                plan.param,
                record.readback.filter(|r| *r != value).map_or(String::new(), |r| format!(" (read back {r}!)")),
                record.printed_hint.as_deref().map_or(String::new(), |h| format!(" (printed list: {h})")),
            );
            match prompter.ask(&question) {
                Answer::Text(text) => record.display = Some(text),
                Answer::Skip => {}
                Answer::Repeat => continue,
                Answer::Quit => {
                    report(ProbeEvent::Stopped("stopped by the user".to_owned()));
                    return Ok(());
                }
            }
            break;
        }
        let accepted = record.accepted;
        file.upsert(record.clone());
        save(file);
        report(ProbeEvent::Recorded(record));

        if accepted == Some(false) {
            rejected_in_a_row += 1;
            if options.stop_after_rejected > 0 && rejected_in_a_row >= options.stop_after_rejected {
                report(ProbeEvent::Stopped(format!(
                    "{rejected_in_a_row} values in a row did not read back; the range probably ends before {value}"
                )));
                return Ok(());
            }
        } else {
            rejected_in_a_row = 0;
        }
    }
    Ok(())
}

/// Sends the original edit buffer back and checks it reads back byte-identically. A read-back
/// that differs is an error too, so the caller prints the restore command and exits non-zero.
fn restore<T: Transport>(
    synth: &mut Synth<T>,
    original: &ReceivedDump,
    report: &mut dyn FnMut(ProbeEvent),
) -> Result<(), String> {
    synth.send_edit_buffer(&original.data)?;
    let back = synth.fetch().map_err(|e| format!("restore read-back failed: {e}"))?;
    let differences = list_addresses(&diff(&original.data, &back.data), 8);
    let identical = differences.is_empty();
    report(ProbeEvent::Restored { identical, differences: differences.clone() });
    if identical {
        Ok(())
    } else {
        Err(format!("the edit buffer read back differs from the original at {differences}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{synth, FakeVirus};
    use std::collections::VecDeque;
    use virus_syx::Page;

    struct Scripted(VecDeque<Answer>, Vec<String>);

    impl Prompter for Scripted {
        fn ask(&mut self, question: &str) -> Answer {
            self.1.push(question.to_owned());
            self.0.pop_front().unwrap_or(Answer::Quit)
        }
    }

    fn options(interactive: bool) -> ProbeOptions {
        ProbeOptions { interactive, readback: true, stop_after_rejected: 3 }
    }

    fn run(
        s: &mut Synth<FakeVirus>,
        plan: &ProbePlan,
        file: ProbeFile,
        options: &ProbeOptions,
        prompter: &mut dyn Prompter,
    ) -> (ProbeOutcome, Vec<ProbeEvent>, usize) {
        let mut events = Vec::new();
        let mut saves = 0;
        let outcome = run_probe(
            s,
            plan,
            file,
            options,
            prompter,
            &mut |_| Ok(()),
            &mut |_| saves += 1,
            &mut |e| events.push(e),
        )
        .unwrap();
        (outcome, events, saves)
    }

    #[test]
    fn destination_probe_records_readback_and_stops_when_values_are_clamped() {
        let mut fake = FakeVirus::new(0);
        fake.max_values.insert((Page::PageB, 65), 4);
        let mut s = synth(fake);
        let plan = destination_plan(DestinationSlot::Assign1, range(0, 127).unwrap());
        let file = ProbeFile::new(&plan, 0, 0x40);
        let before = s.fetch().unwrap().data;

        let (outcome, events, saves) = run(&mut s, &plan, file, &options(false), &mut Scripted(VecDeque::new(), vec![]));

        assert!(outcome.error.is_none() && outcome.restore_error.is_none());
        let values: Vec<u8> = outcome.file.results.iter().map(|r| r.value).collect();
        assert_eq!(values, vec![0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(saves, 8);
        assert_eq!(outcome.file.get(4).unwrap().accepted, Some(true));
        assert_eq!(outcome.file.get(5).unwrap().readback, Some(4));
        assert_eq!(outcome.file.get(1).unwrap().printed_hint.as_deref(), Some("PatchVol"));
        assert!(outcome.file.results.iter().all(|r| r.note.is_none()));
        assert!(events.iter().any(|e| matches!(e, ProbeEvent::Stopped(_))));
        assert!(events.contains(&ProbeEvent::Restored { identical: true, differences: String::new() }));
        assert_eq!(s.fetch().unwrap().data, before, "edit buffer restored");
        let setup: Vec<&Vec<u8>> = s.transport.received.iter().filter(|m| m[6] == 0x71 && m[8] == 64).collect();
        assert_eq!(setup[0][9], 3, "Assign 1 source = ModWheel");
    }

    #[test]
    fn interactive_probe_records_answers_resends_and_quits() {
        let mut s = synth(FakeVirus::new(0));
        let plan = value_plan(ParamAddress::a(112), range(0, 10).unwrap()).unwrap();
        let answers = [
            Answer::Text("Off".into()),
            Answer::Repeat,
            Answer::Text("Delay".into()),
            Answer::Skip,
            Answer::Quit,
        ];
        let mut prompter = Scripted(answers.into_iter().collect(), Vec::new());
        let (outcome, _, _) = run(&mut s, &plan, ProbeFile::new(&plan, 0, 0x40), &options(true), &mut prompter);
        let r = &outcome.file.results;
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].display.as_deref(), Some("Off"));
        assert_eq!(r[1].display.as_deref(), Some("Delay"));
        assert_eq!(r[2].display, None);
        assert!(prompter.1[0].contains("printed list: Off"));
        let a112_changes = s.transport.received.iter().filter(|m| m[6] == 0x70 && m[8] == 112).count();
        assert_eq!(a112_changes, 5, "value 1 was sent twice");
    }

    #[test]
    fn resume_skips_values_with_display_readings() {
        let mut s = synth(FakeVirus::new(0));
        let plan = value_plan(ParamAddress::b(39), range(0, 2).unwrap()).unwrap();
        let mut file = ProbeFile::new(&plan, 0, 0x40);
        file.upsert(ProbeRecord {
            value: 1,
            readback: Some(1),
            accepted: Some(true),
            display: Some("Osc".into()),
            printed_hint: None,
            note: None,
        });
        let json = file.to_json();
        let file = ProbeFile::from_json(&json).unwrap();
        let answers = [Answer::Text("Off".into()), Answer::Text("OscHold".into())];
        let (outcome, events, _) =
            run(&mut s, &plan, file, &options(true), &mut Scripted(answers.into_iter().collect(), vec![]));
        assert!(events.contains(&ProbeEvent::Skipped { value: 1 }));
        let names: Vec<_> = outcome.file.results.iter().map(|r| r.display.clone().unwrap()).collect();
        assert_eq!(names, ["Off", "Osc", "OscHold"]);
        assert!(outcome.file.rust_table("VOCODER").contains("\"OscHold\", // 2"));
    }

    #[test]
    fn probe_without_an_answer_from_the_synth_changes_nothing() {
        let mut fake = FakeVirus::new(0);
        fake.silent = true;
        let mut s = synth(fake);
        let plan = destination_plan(DestinationSlot::Lfo1, range(0, 3).unwrap());
        let result = run_probe(
            &mut s,
            &plan,
            ProbeFile::new(&plan, 0, 0x40),
            &options(false),
            &mut Scripted(VecDeque::new(), vec![]),
            &mut |_| Ok(()),
            &mut |_| {},
            &mut |_| {},
        );
        assert!(result.unwrap_err().contains("fetch the edit buffer first"));
        assert!(s.transport.received.iter().all(|m| m[6] == 0x30), "only the request was sent");
    }

    #[test]
    fn a_restore_that_reads_back_differently_is_a_restore_error() {
        let mut fake = FakeVirus::new(0);
        fake.ignore_dumps = true;
        fake.edit_buffers.insert(0x40, crate::fake::pattern(0)); // A112 = 80, so probing 0..=1 changes it
        let mut s = synth(fake);
        let plan = value_plan(ParamAddress::a(112), range(0, 1).unwrap()).unwrap();
        let (outcome, events, _) =
            run(&mut s, &plan, ProbeFile::new(&plan, 0, 0x40), &options(false), &mut Scripted(VecDeque::new(), vec![]));
        assert!(outcome.error.is_none());
        let error = outcome.restore_error.expect("the restore did not take");
        assert!(error.contains("differs from the original at A112"), "{error}");
        assert!(events.contains(&ProbeEvent::Restored { identical: false, differences: "A112".to_owned() }));
    }

    #[test]
    fn value_plans_refuse_non_sound_parameters_and_bad_ranges() {
        assert!(value_plan(ParamAddress::a(0), vec![0]).is_err());
        assert!(value_plan(ParamAddress::a(1), vec![0]).is_err());
        assert_eq!(value_plan(ParamAddress::a(49), vec![]).unwrap().hint(6), Some("Shaper"));
        assert!(range(5, 4).is_err());
        assert!(range(0, 128).is_err());
        assert_eq!(suggested_max(ParamAddress::a(49)), 40);
        assert_eq!(parse_answer(" Delay 2 \n"), Answer::Text("Delay 2".into()));
        assert_eq!(parse_answer("\n"), Answer::Skip);
        assert_eq!(parse_answer("q"), Answer::Quit);
    }
}

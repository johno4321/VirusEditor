//! Command implementations: the thin I/O shell around the pure modules.
//!
//! Every function returns `Ok(true)` on success, `Ok(false)` when it ran but something
//! failed or needs confirmation (exit code 1), and `Err` for errors (exit code 2).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use virus_db::{ImportTarget, PatchDatabase};
use virus_midi::{find_virus_ports, list_input_ports, list_output_ports};
use virus_syx::Patch;

use crate::address::ParamAddress;
use crate::cli::{Cli, Command, ConnArgs, ProbeArgs};
use crate::decode::describe;
use crate::files::{write_syx, LoadedFile};
use crate::midi_link::{select_port, MidiLink};
use crate::ops::{diff, list_addresses, to_incoming, verify_bank, Synth};
use crate::probe::{
    destination_plan, range, run_probe, suggested_max, value_plan, LinePrompter, ProbeEvent, ProbeFile,
    ProbeOptions, ProbePlan, Prompter,
};
use crate::selftest::{all_passed, format_report, run_selftest, SelftestOptions};
use crate::settings::bank_label;
use crate::stamp::now_stamp;
use crate::summary::patch_summary;

pub type Outcome = Result<bool, String>;

pub fn run(cli: Cli) -> Outcome {
    let conn = &cli.conn;
    match cli.command {
        Command::Ports => ports(conn),
        Command::Monitor { seconds } => monitor(conn, seconds),
        Command::Fetch { out_file, brief } => fetch(conn, out_file.as_deref(), brief),
        Command::Send { file, program, from_bank, allow_bad_checksum, verify } => {
            send(conn, &file, program, from_bank, allow_bad_checksum, verify)
        }
        Command::GetBank { bank, out_file, to_library, db, allow_partial } => {
            get_bank(conn, bank, &out_file, to_library.then_some(db), allow_partial)
        }
        Command::PutBank { bank, file, yes, gap_ms, from_bank, allow_partial, allow_bad_checksum, no_backup, verify } => {
            put_bank(
                conn,
                bank,
                &file,
                PutOptions { yes, gap_ms, from_bank, allow_partial, allow_bad_checksum, no_backup, verify },
            )
        }
        Command::Set { param, value, force, verify } => set(conn, param, value, force, verify),
        Command::SendHex { hex, yes, listen_ms } => send_hex(conn, &hex.join(" "), yes, listen_ms),
        Command::ProbeDestinations { slot, from, to, out_file, probe } => {
            let plan = destination_plan(slot, range(from, to)?);
            probe_command(conn, &plan, &out_file, &probe)
        }
        Command::ProbeValues { param, from, to, out_file, probe } => {
            let plan = value_plan(param, range(from, to.unwrap_or_else(|| suggested_max(param)))?)?;
            let out_file = out_file.unwrap_or_else(|| PathBuf::from(format!("q3_{param}.json")));
            probe_command(conn, &plan, &out_file, &probe)
        }
        Command::ProbeTable { file, name } => {
            let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            print!("{}", ProbeFile::from_json(&text)?.rust_table(&name));
            Ok(true)
        }
        Command::Selftest { bank, skip_bank, skip_global } => selftest(
            conn,
            &SelftestOptions { bank: (!skip_bank).then_some(bank), global_request: !skip_global },
        ),
    }
}

fn open(conn: &ConnArgs) -> Result<Synth<MidiLink>, String> {
    let link = MidiLink::open(conn.input.as_deref(), conn.output.as_deref(), true, true, conn.verbose)?;
    eprintln!(
        "in: {} | out: {} | device {} | {} | channel {}",
        link.input.as_deref().unwrap_or("-"),
        link.output.as_deref().unwrap_or("-"),
        conn.device,
        conn.part,
        conn.channel + 1
    );
    let mut synth = Synth::new(
        link,
        conn.target(),
        Duration::from_millis(conn.timeout_ms),
        Duration::from_millis(conn.settle_ms),
    )
    .with_quiet(Duration::from_millis(conn.quiet_ms), Duration::from_millis(conn.bank_quiet_ms));
    if conn.verbose {
        synth.on_stale = Some(Box::new(|n| {
            eprintln!("dropped {n} stale message(s) while waiting for a quiet line before the request")
        }));
    }
    Ok(synth)
}

fn ports(conn: &ConnArgs) -> Outcome {
    let inputs = list_input_ports()?;
    let outputs = list_output_ports()?;
    for (title, names, pattern, kind, flag) in [
        ("MIDI inputs", &inputs, conn.input.as_deref(), "input", "--in"),
        ("MIDI outputs", &outputs, conn.output.as_deref(), "output", "--out"),
    ] {
        let virus = find_virus_ports(names);
        println!("{title}:");
        if names.is_empty() {
            println!("  (none)");
        }
        for name in names {
            println!("  {} {name}", if virus.contains(name) { "*" } else { " " });
        }
        match select_port(names, pattern, kind, flag) {
            Ok(name) => println!("  → would use: {name}\n"),
            Err(e) => println!("  → {e}\n"),
        }
    }
    println!("* = looks like a Virus by name. The Virus Rack has no USB; your MIDI interface's name is what counts.");
    Ok(true)
}

fn monitor(conn: &ConnArgs, seconds: Option<u64>) -> Outcome {
    let mut link = MidiLink::open(conn.input.as_deref(), None, true, false, conn.verbose)?;
    let target = conn.target();
    eprintln!(
        "Monitoring {} (device {}, channel {}, {}). Ctrl+C to stop.",
        link.input.as_deref().unwrap_or("-"),
        conn.device,
        conn.channel + 1,
        conn.part
    );
    let start = Instant::now();
    let end = seconds.map(|s| start + Duration::from_secs(s));
    while end.is_none_or(|end| Instant::now() < end) {
        if let Some(event) = crate::transport::Transport::recv(&mut link, Duration::from_millis(200)) {
            if let Some(line) = describe(&event, &target) {
                println!("{:>9.3}s  {line}", start.elapsed().as_secs_f64());
            }
        }
    }
    Ok(true)
}

fn fetch(conn: &ConnArgs, out_file: Option<&Path>, brief: bool) -> Outcome {
    let mut synth = open(conn)?;
    let dump = synth.fetch().map_err(|e| e.to_string())?;
    let patch = Patch::from_bytes(&dump.data);
    let summary = patch_summary(&patch);
    if brief {
        summary.lines().take(3).for_each(|l| println!("{l}"));
    } else {
        print!("{summary}");
    }
    println!("Checksum:      {}", if dump.checksum_ok { "ok" } else { "WRONG (the synth sent a bad checksum)" });
    if let Some(path) = out_file {
        write_syx(path, &[&dump.frame]).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("Saved the dump as received ({} bytes) to {}", dump.frame.len(), path.display());
    }
    Ok(dump.checksum_ok)
}

fn send(
    conn: &ConnArgs,
    file: &Path,
    program: Option<u8>,
    from_bank: Option<u8>,
    allow_bad_checksum: bool,
    verify: bool,
) -> Outcome {
    let loaded = LoadedFile::load(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let dump = loaded.pick_single(program, from_bank)?.clone();
    let name = Patch::from_bytes(&dump.data).name();
    if !dump.checksum_ok && !allow_bad_checksum {
        return Err(format!(
            "'{name}' has a wrong checksum in the file (damaged data?); use --allow-bad-checksum to send it anyway"
        ));
    }
    let mut synth = open(conn)?;
    synth.send_edit_buffer(&dump.data)?;
    println!("Sent '{name}' to the edit buffer ({}). It is not stored until you STORE it on the synth.", conn.part);
    if verify {
        let back = synth.fetch().map_err(|e| e.to_string())?;
        let differences = diff(&dump.data, &back.data);
        if differences.is_empty() {
            println!("Verify: read back byte-identical.");
        } else {
            println!(
                "Verify: {} byte(s) differ: {} (A0 = sound version update by the synth is expected for old sounds, SPEC §5.2)",
                differences.len(),
                list_addresses(&differences, 16)
            );
            return Ok(false);
        }
    }
    Ok(true)
}

fn progress_line(label: &str, done: usize, total: usize) {
    eprint!("\r{label}: {done}/{total}   ");
    let _ = std::io::stderr().flush();
}

/// Requests a bank and writes it. Returns the received dumps' count and any problem.
fn receive_bank(synth: &mut Synth<MidiLink>, bank: u8, out_file: &Path, allow_partial: bool) -> Result<crate::transport::BankCollector, String> {
    let label = format!("bank {}", bank_label(bank));
    let (collector, error) = synth.get_bank(bank, |n| progress_line(&label, n, 128));
    eprintln!();
    if collector.duplicates() > 0 {
        eprintln!("note: {} program(s) arrived more than once (last one kept)", collector.duplicates());
    }
    let bad = collector.bad_checksums();
    if !bad.is_empty() {
        eprintln!("warning: wrong checksums in programs {bad:?}");
    }
    if let Some(error) = &error {
        if !allow_partial || collector.count() == 0 {
            return Err(format!("{label} incomplete, nothing written: {error}"));
        }
        eprintln!("warning: {label} incomplete ({} of 128); writing what arrived: {error}", collector.count());
    }
    let frames: Vec<&[u8]> = collector.dumps().iter().map(|d| d.frame.as_slice()).collect();
    write_syx(out_file, &frames).map_err(|e| format!("{}: {e}", out_file.display()))?;
    println!("Saved {} dump(s) of {label} to {}", frames.len(), out_file.display());
    Ok(collector)
}

fn get_bank(conn: &ConnArgs, bank: u8, out_file: &Path, library: Option<Option<PathBuf>>, allow_partial: bool) -> Outcome {
    if bank > 4 {
        eprintln!("note: bank {} (bb=0x{bank:02X}) addressing is undocumented (open question Q2); recording whatever arrives.", bank_label(bank));
    }
    let mut synth = open(conn)?;
    let collector = receive_bank(&mut synth, bank, out_file, allow_partial)?;
    if let Some(db_path) = library {
        let path = match db_path {
            Some(path) => path,
            None => default_db_path()?,
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let db = PatchDatabase::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let name = format!("Synth {} backup {}", bank_label(bank), now_stamp());
        let id = db.create_bank(&name).map_err(|e| e.to_string())?;
        let report = db
            .import_single_dumps(&to_incoming(&collector.dumps()), ImportTarget::Bank(id))
            .map_err(|e| e.to_string())?;
        println!("Library: {} patch(es) saved as bank '{name}' in {}", report.imported, path.display());
    }
    Ok(collector.count() == 128 && collector.bad_checksums().is_empty())
}

/// The editor's library location (same as `virus-app`).
fn default_db_path() -> Result<PathBuf, String> {
    directories::ProjectDirs::from("", "", "VirusEditor")
        .map(|dirs| dirs.data_dir().join("virus_library.db"))
        .ok_or_else(|| "no user data directory could be determined; pass --db".to_owned())
}

struct PutOptions {
    yes: bool,
    gap_ms: u64,
    from_bank: Option<u8>,
    allow_partial: bool,
    allow_bad_checksum: bool,
    no_backup: bool,
    verify: bool,
}

fn put_bank(conn: &ConnArgs, bank: u8, file: &Path, options: PutOptions) -> Outcome {
    if !matches!(bank, 1 | 2) {
        return Err(format!(
            "refusing to send to bank {}: only RAM banks A and B are writable (C/D are ROM)",
            bank_label(bank)
        ));
    }
    let letter = bank_label(bank);
    let loaded = LoadedFile::load(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let selection = loaded.pick_bank(options.from_bank)?;
    if !selection.missing.is_empty() && !options.allow_partial {
        return Err(format!(
            "the file's bank {} has only {} of 128 programs; use --allow-partial to send just those",
            bank_label(selection.source_bank),
            selection.programs.len()
        ));
    }
    if !selection.bad_checksums.is_empty() && !options.allow_bad_checksum {
        return Err(format!(
            "programs {:?} have wrong checksums in the file (damaged data?); use --allow-bad-checksum to send anyway",
            selection.bad_checksums
        ));
    }
    let backup_command =
        format!("virus{} get-bank {letter} --out-file backup_bank_{letter}_{}.syx", conn.repeat_options(), now_stamp());
    if !options.yes {
        println!(
            "This overwrites RAM bank {letter} on the Virus with {} program(s) from {} (file bank {}). It cannot be undone.",
            selection.programs.len(),
            file.display(),
            bank_label(selection.source_bank)
        );
        println!("Stop the clock/transport on your sequencer first.");
        println!("Back up both RAM banks first, e.g.:\n  {backup_command}");
        println!("Then re-run with --yes (an automatic backup of bank {letter} is also taken unless --no-backup).");
        return Ok(false);
    }

    let mut synth = open(conn)?;
    if !options.no_backup {
        let backup = PathBuf::from(format!("backup_bank_{letter}_{}.syx", now_stamp()));
        eprintln!("Backing up bank {letter} first…");
        receive_bank(&mut synth, bank, &backup, false).map_err(|e| format!("backup failed, nothing was sent: {e}"))?;
    } else {
        eprintln!("No automatic backup (--no-backup). Manual backup command:\n  {backup_command}");
    }
    let label = format!("sending to bank {letter}");
    synth.put_bank(bank, &selection.programs, Duration::from_millis(options.gap_ms), &mut |n, total| {
        progress_line(&label, n, total)
    })?;
    eprintln!();
    println!(
        "Sent {} program(s) to bank {letter} with {} ms gaps. Check the Virus display for checksum or MIDI buffer errors.",
        selection.programs.len(),
        options.gap_ms
    );
    if options.verify {
        std::thread::sleep(Duration::from_secs(1));
        let (collector, error) = synth.get_bank(bank, |n| progress_line("verify", n, 128));
        eprintln!();
        if let Some(error) = error {
            println!("Verify failed: {error}");
            return Ok(false);
        }
        let mismatched = verify_bank(&selection.programs, &collector);
        if mismatched.is_empty() {
            println!("Verify: all {} program(s) read back byte-identical.", selection.programs.len());
        } else {
            println!("Verify: programs {mismatched:?} differ from what was sent.");
            return Ok(false);
        }
    }
    Ok(true)
}

fn set(conn: &ConnArgs, param: ParamAddress, value: u8, force: bool, verify: bool) -> Outcome {
    if param == ParamAddress::a(0) {
        return Err("A0 is the Sound Version; the editor never changes it (SPEC §5.2)".to_owned());
    }
    if verify && !param.in_single_dump() {
        return Err(format!(
            "{param} is a Page C parameter and isn't part of the single dump, so --verify can't read it back \
             (for C32/C33 check the selected sound with `fetch --brief`)"
        ));
    }
    if let Some(reason) = param.edit_warning() {
        if !force {
            return Err(format!("{param} is {reason}; use --force to send it anyway"));
        }
        eprintln!("warning: {param} is {reason}");
    }
    let mut synth = open(conn)?;
    synth.set_param(param, value)?;
    println!("Sent {param} {} = {} to {}", param.name(), param.describe_value(value), conn.part);
    if verify {
        let back = synth.fetch().map_err(|e| e.to_string())?;
        let read = back.data[param.offset()];
        println!("Verify: edit buffer holds {param} = {}", param.describe_value(read));
        return Ok(read == value);
    }
    Ok(true)
}

/// Sends one raw SysEx message (experiments), then prints what arrives for `listen_ms`.
fn send_hex(conn: &ConnArgs, text: &str, yes: bool, listen_ms: u64) -> Outcome {
    let message = crate::messages::parse_hex_sysex(text)?;
    let target = conn.target();
    let meaning = describe(&virus_midi::MidiEvent::SysEx(message.clone()), &target)
        .unwrap_or_else(|| "not a Virus message this tool can decode".to_owned());
    println!("Message ({} bytes): {}\n  = {meaning}", message.len(), crate::messages::hex(&message));
    if !yes {
        println!("Nothing sent. Raw messages bypass every safety check of this tool; re-run with --yes to send it.");
        return Ok(false);
    }
    let mut synth = open(conn)?;
    synth.send_raw(&message)?;
    println!("Sent. Listening for {listen_ms} ms…");
    let start = Instant::now();
    let arrived = synth.listen(Duration::from_millis(listen_ms), &mut |event| {
        if let Some(line) = describe(event, &target) {
            println!("{:>9.3}s  {line}", start.elapsed().as_secs_f64());
        }
    });
    println!("{arrived} message(s) arrived.");
    Ok(true)
}

/// Prompter for `--non-interactive` runs (never asked).
struct NoPrompt;

impl Prompter for NoPrompt {
    fn ask(&mut self, _question: &str) -> crate::probe::Answer {
        crate::probe::Answer::Skip
    }
}

fn probe_command(conn: &ConnArgs, plan: &ProbePlan, out_file: &Path, args: &ProbeArgs) -> Outcome {
    let file = if args.resume {
        let text = std::fs::read_to_string(out_file).map_err(|e| format!("--resume: {}: {e}", out_file.display()))?;
        let file = ProbeFile::from_json(&text)?;
        if file.param != plan.param.to_string() || file.kind != plan.kind {
            return Err(format!("{} holds a {} probe of {}, not {}", out_file.display(), file.kind, file.param, plan.param));
        }
        file
    } else {
        if out_file.exists() {
            return Err(format!("{} exists; use --resume to continue it or --out-file for a new file", out_file.display()));
        }
        ProbeFile::new(plan, conn.device.0, conn.part.raw())
    };
    let interactive = !args.non_interactive;
    let options = ProbeOptions {
        interactive,
        readback: !args.no_readback,
        stop_after_rejected: if args.no_readback { 0 } else { args.stop_after_rejected },
    };

    println!("Probing {} ({}) over {} value(s); results → {}", plan.param, plan.label, plan.values.len(), out_file.display());
    for (address, value) in &plan.setup {
        println!("  setup: {address} {} = {}", address.name(), address.describe_value(*value));
    }
    if interactive {
        println!("On the Virus, bring {} onto the display now.", plan.look_at);
        println!("For each value type what the display shows; Enter = skip, r = resend, q = stop (the sound is restored).");
    }

    let mut synth = open(conn)?;
    let backup_file = args.backup_file.clone();
    let mut stdin_prompter;
    let mut no_prompt = NoPrompt;
    let prompter: &mut dyn Prompter = if interactive {
        stdin_prompter = LinePrompter::new(std::io::stdin().lock());
        &mut stdin_prompter
    } else {
        &mut no_prompt
    };
    let outcome = run_probe(
        &mut synth,
        plan,
        file,
        &options,
        prompter,
        &mut |original| {
            write_syx(&backup_file, &[&original.frame]).map_err(|e| format!("{}: {e}", backup_file.display()))
        },
        &mut |file| {
            if let Err(e) = std::fs::write(out_file, file.to_json()) {
                eprintln!("warning: could not save {}: {e}", out_file.display());
            }
        },
        &mut |event| print_probe_event(&event),
    )?;

    let answered = outcome.file.results.iter().filter(|r| r.display.is_some()).count();
    println!(
        "{} value(s) recorded, {answered} with a display reading, saved to {}",
        outcome.file.results.len(),
        out_file.display()
    );
    if let Some(error) = &outcome.error {
        println!("Probing stopped by an error: {error}");
    }
    if let Some(error) = &outcome.restore_error {
        println!(
            "RESTORE FAILED: {error}\nSend the original back with:\n  virus{} send {}",
            conn.repeat_options(),
            backup_file.display()
        );
    }
    Ok(outcome.error.is_none() && outcome.restore_error.is_none())
}

fn print_probe_event(event: &ProbeEvent) {
    match event {
        ProbeEvent::Original { name } => println!("Original edit buffer: '{name}' (backed up)"),
        ProbeEvent::Skipped { value } => println!("  {value:>3}: already recorded, skipped"),
        ProbeEvent::Recorded(r) => println!(
            "  {:>3}: read back {:<4} display {:<14} printed {}{}",
            r.value,
            r.readback.map_or("-".to_owned(), |v| v.to_string()),
            r.display.as_deref().unwrap_or("-"),
            r.printed_hint.as_deref().unwrap_or("-"),
            r.note.as_deref().map_or(String::new(), |n| format!("  [{n}]"))
        ),
        ProbeEvent::Stopped(reason) => println!("Stopped: {reason}"),
        ProbeEvent::Restored { identical: true, .. } => println!("Edit buffer restored (read back byte-identical)."),
        ProbeEvent::Restored { identical: false, differences } => {
            println!("Edit buffer restored, but read back differs at {differences}")
        }
    }
}

fn selftest(conn: &ConnArgs, options: &SelftestOptions) -> Outcome {
    let mut synth = open(conn)?;
    let checks = run_selftest(&mut synth, options, &mut |check| eprintln!("[{}] {}", check.status, check.name));
    println!();
    print!("{}", format_report(&checks));
    Ok(all_passed(&checks))
}

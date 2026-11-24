//! Read-only live probe for the Access Virus Rack: lists ports, fetches the edit buffer,
//! and backs up RAM banks. Never sends anything that changes the synth.
use std::time::{Duration, Instant};
use virus_midi::{list_input_ports, list_output_ports, MidiCommand, MidiEvent, MidiService};
use virus_syx::{
    encode_dump_request, encode_single_bank_request, format_value, parse_sysex, Patch, SysExMessage,
    SINGLE_PARAMS, ParamClass,
};

fn pick(ports: &[String], wanted: Option<&String>) -> Option<String> {
    if let Some(w) = wanted {
        let w = w.to_lowercase();
        return ports.iter().find(|p| p.to_lowercase().contains(&w)).cloned();
    }
    ports.iter().find(|p| !p.to_lowercase().contains("microsoft")).cloned()
}

fn connect(args: &[String]) -> MidiService {
    let ins = list_input_ports().unwrap_or_default();
    let outs = list_output_ports().unwrap_or_default();
    let inp = pick(&ins, args.iter().position(|a| a == "--in").and_then(|i| args.get(i + 1)))
        .expect("no MIDI input found (use --in <name part>)");
    let out = pick(&outs, args.iter().position(|a| a == "--out").and_then(|i| args.get(i + 1)))
        .expect("no MIDI output found (use --out <name part>)");
    println!("using input  '{inp}'\nusing output '{out}'");
    let svc = MidiService::spawn().expect("MIDI service");
    svc.send_command(MidiCommand::ConnectInput(inp)).unwrap();
    svc.send_command(MidiCommand::ConnectOutput(out)).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    svc
}

fn device(args: &[String]) -> u8 {
    // 0x10 = Omni: every Virus answers regardless of its Device ID.
    args.iter().position(|a| a == "--device").and_then(|i| args.get(i + 1)).map(|s| {
        if s.eq_ignore_ascii_case("omni") { 0x10 } else { s.parse::<u8>().expect("device 1-16") - 1 }
    }).unwrap_or(0x10)
}

fn collect(svc: &MidiService, want: usize, timeout: Duration) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    let mut last = Instant::now();
    while frames.len() < want && last.elapsed() < timeout {
        match svc.try_recv_event() {
            Some(MidiEvent::SysEx(f)) => { frames.push(f); last = Instant::now(); if want > 1 { print!("\rreceived {}/{}", frames.len(), want); } }
            Some(MidiEvent::Error(e)) => println!("MIDI error: {e}"),
            Some(MidiEvent::ControlChange { channel, controller, value }) => println!("CC ch{} #{} = {}", channel + 1, controller, value),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    if want > 1 { println!(); }
    frames
}

fn describe(frame: &[u8]) {
    match parse_sysex(frame) {
        Ok(SysExMessage::SingleDump { device_id, bank, program, checksum_ok, .. }) => {
            let (p, _) = Patch::from_sysex(frame).unwrap();
            println!("single dump: device {:02X}, bank {}, program {:02X}, checksum ok: {}", device_id, bank, program, checksum_ok);
            println!("name '{}'  category {} / {}  sound version {}", p.name(), p.category1().name(), p.category2().name(), p.sound_version());
            for d in SINGLE_PARAMS.iter().filter(|d| d.class == ParamClass::Sound) {
                println!("  {:?}{:<3} {:<28} {}", d.page, d.number, d.name, format_value(d, p.get(d.page, d.number)));
            }
        }
        Ok(other) => println!("other message: {other:?}"),
        Err(e) => println!("unparsable ({e}): {:02X?}", frame),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("fetch") => {
            let svc = connect(&args);
            let req = encode_dump_request(device(&args), 0x00, 0x40);
            println!("sending single request (read-only): {:02X?}", req);
            svc.send_command(MidiCommand::Send(req)).unwrap();
            let frames = collect(&svc, 1, Duration::from_secs(3));
            if frames.is_empty() { println!("NO ANSWER within 3 s. Check cables (Virus MIDI OUT -> interface IN), device ID, and the Virus' MIDI Dump RX/TX settings."); return; }
            std::fs::write("edit_buffer.syx", &frames[0]).unwrap();
            describe(&frames[0]);
            println!("saved edit_buffer.syx");
        }
        Some("backup") => {
            let svc = connect(&args);
            for (letter, bb) in [("A", 1u8), ("B", 2u8)] {
                let req = encode_single_bank_request(device(&args), bb);
                println!("requesting bank {letter} (read-only): {:02X?}", req);
                svc.send_command(MidiCommand::Send(req)).unwrap();
                let frames = collect(&svc, 128, Duration::from_secs(5));
                let ok = frames.iter().filter(|f| matches!(parse_sysex(f), Ok(SysExMessage::SingleDump { checksum_ok: true, .. }))).count();
                let file = format!("backup_bank_{letter}.syx");
                std::fs::write(&file, frames.concat()).unwrap();
                println!("bank {letter}: {} messages, {} single dumps with valid checksum -> {file}", frames.len(), ok);
                if let Some(f) = frames.first() { if let Ok((p, _)) = Patch::from_sysex(f) { println!("  first patch: '{}'", p.name()); } }
            }
        }
        Some("monitor") => {
            let svc = connect(&args);
            println!("monitoring for 60 s - turn a knob on the Virus...");
            for f in collect(&svc, usize::MAX, Duration::from_secs(60)) { describe(&f); }
        }
        _ => {
            println!("inputs:  {:?}", list_input_ports().unwrap_or_default());
            println!("outputs: {:?}", list_output_ports().unwrap_or_default());
            println!("\nusage: virus-probe [ports] | fetch | backup | monitor   [--in name] [--out name] [--device 1-16|omni]");
        }
    }
}

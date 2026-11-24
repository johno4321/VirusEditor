# VirusEditor

A desktop editor and patch librarian for the **Access Virus B / Rack / Classic** synthesizers, written in Rust.

It talks to the synth over MIDI (SysEx, CC and Poly Pressure), edits the current sound live, and keeps a searchable library of patches in a local SQLite database.

> **Work in progress.** The parameter layout doesn't yet match the hardware, and loading and saving a patch can change it. Don't use it on patches you can't replace. See [TODO.md](TODO.md).

## Features

- **Sound editor:** oscillators, dual filters with a frequency-response view, filter and amp envelopes with curve views, LFOs, EQ, phaser and analog boost.
- **Mod matrix and arpeggiator:** six matrix slots, each with a source, amount and two destinations.
- **Librarian:** banks A–D, search by name and tags, filter by the 17 Virus categories, favourites, and `.syx` / `.mid` bank import and `.syx` export.
- **MIDI:** port auto-detection (names containing "Virus", "Access" or "OSR1"), device ID selection, fetch and send the edit buffer, and Rx/Tx activity lights.

## Requirements

- Rust (stable) with Cargo: <https://rustup.rs>
- A MIDI interface connected to the Virus (only needed for live editing)

SQLite is built in (`rusqlite` with the `bundled` feature), so nothing else needs installing.

## Getting started

```powershell
cargo run -p virus-app --release
```

Other useful commands:

```powershell
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
```

The app creates `virus_library.db` in the folder you run it from.

## Using it

1. Connect the Virus. Pick the MIDI in and out ports in the bottom bar (or let auto-detect find them) and set the device ID to match the synth (or Omni).
2. **📥 Fetch Synth Buffer** loads the synth's current sound into the editor.
3. Turn knobs in **🎹 Sound Engine** or **🔀 Mod Matrix & Arp**; each change is sent to the synth straight away.
4. **📤 Send to Synth Buffer** sends the whole patch to the synth's edit buffer.
5. In **📚 Patch Librarian**, import a bank file, then **Load**, **Send** or **Store Active** on any row.

Factory banks to try are in `VirusOSR200_INT/OSR200_INT/Goodies/Factory soundset/`.

## Project layout

| Crate | Purpose |
|---|---|
| [`crates/virus-syx`](crates/virus-syx) | Core domain: SysEx framing and checksums, the 256-byte `Patch` (Page A + Page B), parameter types and enums. No I/O |
| [`crates/virus-midi`](crates/virus-midi) | MIDI I/O on a background thread (`midir`), SysEx stream assembly, and command/event channels for the UI |
| [`crates/virus-db`](crates/virus-db) | SQLite librarian: banks, patches, categories, search, and bank import/export |
| [`crates/virus-app`](crates/virus-app) | The desktop app (`eframe` / `egui`): panels, knobs and curve widgets |

## Documentation

- [SPEC.md](SPEC.md): architecture, the SysEx protocol, and the build log.
- [TODO.md](TODO.md): known issues and planned work.
- `OSR1_Manual_International.pdf`: the Access Virus Rack manual. Chapter 20 covers SysEx.
- `VirusOSR200_INT/`: the OS 2.00 update, factory soundsets and addenda.

## License

MIT or Apache-2.0, at your option (see `Cargo.toml`).

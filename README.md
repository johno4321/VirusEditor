# VirusEditor

A desktop sound editor and patch librarian for the **Access Virus Rack (classic), OS 2.00**, written in Rust. Other Virus models are out of scope for now.

It talks to the synth over MIDI SysEx, edits the current sound live, and keeps a searchable library of patches in a local SQLite database. Patches are stored as their exact 256 dump bytes, so a sound that goes in and comes back out untouched is byte-identical.

> **Work in progress.** The editor hasn't been tried against real hardware yet, and the assign/definable destination names are unverified (see below). See [TODO.md](TODO.md).

## Features

- **Sound editor:** generated from the OS 2.0 parameter table, grouped by section (common, oscillators, filters, envelopes, LFO 1–3, unison, input, effects incl. phaser, arpeggiator, clocks, velocity, vocoder, analog boost). Knobs for continuous values, lists for choices (undocumented values show as `#n` and are kept), checkboxes for switches. Filter-response and envelope views. Name (10 ASCII characters) and the two categories are written into the patch; the Sound Version is shown read-only.
- **Modulation:** Assign 1 (1 destination), Assign 2 (2), Assign 3 (3), LFO 1/2 assign and the two Definable knobs. Destinations are edited as **raw numbers**; the printed names are shown greyed as unverified hints, because the raw value → name mapping isn't confirmed yet (open question Q1 in [SPEC.md](SPEC.md)).
- **Librarian:** banks A–D (C and D marked ROM), all 128 slots per bank including empty ones, search by name and tags, category and favourite filters, tags, Load / Send to edit buffer / Store with overwrite confirmation.
- **Import / export:** `.syx`, `.bin` and `.mid` import honours the bank byte (edit-buffer dumps are offered as loose patches, never put into a slot), and reports wrong checksums, skipped multi dumps and other messages. Export writes a bank as `.syx`.
- **Synth transfer:** fetch and send the edit buffer (Single mode or Multi part 1–16), request banks A–D, send library bank A or B to the synth (with a warning and a backup option).
- **MIDI:** ports chosen by name with auto-detection (names containing "Virus", "Access" or "OSR"), synth channel, device ID (default 1, or Omni), Rx/Tx lights. Edits go out as SysEx parameter changes; incoming CC/Poly Pressure is accepted only on the synth's channel and only for sound parameters.

## Requirements

- Rust (stable) with Cargo: <https://rustup.rs>
- A MIDI interface connected to the Virus (only needed for live editing)

SQLite is built in (`rusqlite` with the `bundled` feature), so nothing else needs installing.

Behind a proxy that blocks certificate revocation checks (`CRYPT_E_NO_REVOCATION_CHECK`), add this to `%USERPROFILE%\.cargo\config.toml`:

```toml
[http]
check-revoke = false
```

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

The library is stored in your user data folder (on Windows `%APPDATA%\VirusEditor\data\virus_library.db`); the path is shown in the bottom bar. If it can't be opened, the app runs with an in-memory library and shows a red warning that nothing will be saved.

The app needs OpenGL 2.0 or later (it won't start in a remote desktop session on a VM without a GPU).

## Using it

1. Connect the Virus. Pick the MIDI in and out ports in the bottom bar (or let auto-detect find them), and set the synth's MIDI channel, device ID (global C93; default 1) and the part you're editing (Single, or a Multi part).
2. **📥 Fetch** loads the synth's edit buffer into the editor; **📤 Send** sends the edited patch to it.
3. Turn knobs in **🎹 Sound Editor** or **🔀 Modulation**; each change is sent straight away as a SysEx parameter change.
4. **💾 Store…** saves the edited patch to a library slot (you're asked before overwriting).
5. In **📚 Librarian**, import a bank file, then **Load** or **Send** any patch, edit its tags or favourite flag, or export a bank. Edit-buffer dumps from a file appear in a separate list from which you can load or store them.
6. To put a bank on the synth, select library bank A or B and use **📤 Send bank to synth…**. This overwrites the synth's RAM bank, so back it up first: **Request bank** sends the synth's bank to the app, and **Received from synth → Import as a new library bank** keeps it safe.

Factory banks to try: the clean copies `testdata/factory/Bank_A.syx` and `Bank_B.syx`. The originals in `VirusOSR200_INT/OSR200_INT/Goodies/Factory soundset/` are partly damaged; see SPEC §6.

## Command-line tool

`virus` (crate [`crates/virus-cli`](crates/virus-cli)) talks to the synth without the GUI. It's meant for the first live session and the open questions. It can:

- list ports and monitor incoming MIDI, decoded;
- fetch and send the edit buffer, and set single parameters;
- request banks (with an optional backup into the library) and send RAM banks A/B (with confirmation and an automatic backup);
- probe destination names (Q1) and value lists (Q3);
- run a self-test.

```powershell
cargo run -p virus-cli --release -- --help
```

See the [crate README](crates/virus-cli/README.md) for the commands and the **first cable session runbook**.

## Project layout

| Crate | Purpose |
|---|---|
| [`crates/virus-syx`](crates/virus-syx) | Core domain: SysEx framing and checksums, the raw 256-byte `Patch`, the parameter table (SPEC §4), and `.syx`/`.mid` reading. No I/O beyond reading files |
| [`crates/virus-midi`](crates/virus-midi) | MIDI I/O on a background thread (`midir`), SysEx stream assembly, and command/event channels for the UI |
| [`crates/virus-db`](crates/virus-db) | SQLite librarian: banks, patches, categories, search, and bank import/export |
| [`crates/virus-app`](crates/virus-app) | The desktop app (`eframe` / `egui`). Non-UI logic (MIDI interpretation, outgoing SysEx, import/export, storage) lives in `session.rs`, `library.rs` and `storage.rs` and is unit-tested; `views/` and `widgets/` draw it |
| [`crates/virus-cli`](crates/virus-cli) | The `virus` command-line tool for live sessions: fetch, send, banks, monitor, Q1/Q3 probes, self-test. Logic is tested against an in-memory fake Virus |

## Documentation

- [SPEC.md](SPEC.md): architecture, the SysEx protocol, and the build log.
- [TODO.md](TODO.md): known issues and planned work.
- `OSR1_Manual_International.pdf`: the Access Virus Rack manual. Chapter 20 covers SysEx.
- `VirusOSR200_INT/`: the OS 2.00 update, factory soundsets and addenda.

## License

MIT or Apache-2.0, at your option (see `Cargo.toml`).

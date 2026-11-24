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
- **Settings:** MIDI ports, channel, device ID, part, audition settings (Hold always starts off), last tab and bank, import/export folders and window size/position are remembered between runs. File dialogs and import/export don't block the window.
- **Select on synth (optional):** in the Librarian, tick *Select on synth* so that **Load** from bank A–D also sends a SysEx Bank Change and Program Change to the edited part. Only meaningful when the library bank mirrors the synth's bank.
- **Note audition:** a collapsible keyboard strip on every tab plays the synth so you can hear your edits without a keyboard: an on-screen piano (velocity by click height or a fixed velocity), computer keys, octave shift, Hold (for pads and the arpeggiator), an **Audition** button (C3 for 1 s, like the Virus' own), test phrases (note, chord, bass line) at a set tempo, and **Panic**. Every note gets a note off when you release it, change octave, channel or part, disconnect, switch to another window, or quit.

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

The Rust version is pinned in `rust-toolchain.toml` (rustup installs it automatically). CI (`.github/workflows/ci.yml`) runs clippy, the tests and a release build on Windows and Ubuntu; the Windows `virus-app.exe` can be downloaded from each run's artifacts.

To check the MIDI connection without the GUI, the read-only probe lists ports, fetches the edit buffer, backs up banks A and B, or monitors incoming MIDI. It never changes the synth:

```powershell
cargo run -p virus-app --example virus_probe --release -- [ports|fetch|backup|monitor] [--in <name part>] [--out <name part>] [--device 1-16|omni]
```

The library is stored in your user data folder (on Windows `%APPDATA%\VirusEditor\data\virus_library.db`); the path is shown in the bottom bar. If it can't be opened, the app runs with an in-memory library and shows a red warning that nothing will be saved. Your settings are in `settings.json` in the same folder; if that file is corrupt it is renamed to `settings.json.bak` and the app starts with defaults.

The app draws with OpenGL 2.0 or later. Where that isn't available (for example a VM over remote desktop with no GPU), it automatically switches to Direct3D 12, which falls back to Windows' WARP software renderer; this uses more CPU but works. To pick a renderer yourself, pass `--renderer glow` or `--renderer wgpu` (e.g. `cargo run -p virus-app --release -- --renderer wgpu`), or set the `VIRUS_RENDERER` environment variable to `glow` or `wgpu`.

## Using it

1. Connect the Virus. Pick the MIDI in and out ports in the bottom bar (or let auto-detect find them), and set the synth's MIDI channel, device ID (global C93; default 1) and the part you're editing (Single, or a Multi part).
2. **📥 Fetch** loads the synth's edit buffer into the editor; **📤 Send** sends the edited patch to it.
3. Turn knobs in **🎹 Sound Editor** or **🔀 Modulation**; each change is sent straight away as a SysEx parameter change.
4. To hear the change, play the **🎹 Keyboard** strip above the bottom bar: click or drag on the keys (lower on a key = louder), or use the computer keys `A W S E D F T G Y H U J K` (C up to C) and `Z`/`X` for octave down/up. Computer keys don't play while you're typing in a text field. **▶ Audition** plays C3; **Note / Chord / Bass line** play short phrases at the bpm shown; **Hold** keeps notes on until you click them again (handy for pads and arpeggios); **🛑 Panic** silences everything. Notes go to the synth channel in Single mode and to channel *n* for Multi part *n*; pick another channel in the strip's channel list if your Multi is set up differently.
5. **💾 Store…** saves the edited patch to a library slot (you're asked before overwriting).
6. In **📚 Librarian**, import a bank file, then **Load** or **Send** any patch, edit its tags or favourite flag, or export a bank. Edit-buffer dumps from a file appear in a separate list from which you can load or store them.
7. To put a bank on the synth, select library bank A or B and use **📤 Send bank to synth…**. This overwrites the synth's RAM bank, so back it up first: **Request bank** sends the synth's bank to the app, and **Received from synth → Import as a new library bank** keeps it safe.

Factory banks to try: the clean copies `testdata/factory/Bank_A.syx` and `Bank_B.syx`. The originals in `VirusOSR200_INT/OSR200_INT/Goodies/Factory soundset/` are partly damaged; see SPEC §6.

## Project layout

| Crate | Purpose |
|---|---|
| [`crates/virus-syx`](crates/virus-syx) | Core domain: SysEx framing and checksums, the raw 256-byte `Patch`, the parameter table (SPEC §4), and `.syx`/`.mid` reading. No I/O beyond reading files |
| [`crates/virus-midi`](crates/virus-midi) | MIDI I/O on a background thread (`midir`), SysEx stream assembly, and command/event channels for the UI |
| [`crates/virus-db`](crates/virus-db) | SQLite librarian: banks, patches, categories, search, and bank import/export |
| [`crates/virus-app`](crates/virus-app) | The desktop app (`eframe` / `egui`). Non-UI logic (MIDI interpretation, outgoing SysEx, import/export, storage) lives in `session.rs`, `library.rs` and `storage.rs` and is unit-tested; `views/` and `widgets/` draw it |

## Documentation

- [SPEC.md](SPEC.md): architecture, the SysEx protocol, and the build log.
- [TODO.md](TODO.md): known issues and planned work.
- `OSR1_Manual_International.pdf`: the Access Virus Rack manual. Chapter 20 covers SysEx.
- `VirusOSR200_INT/`: the OS 2.00 update, factory soundsets and addenda.

## License

MIT or Apache-2.0, at your option (see `Cargo.toml`).

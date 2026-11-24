# VirusEditor todo list

**Updated Tue 2026-11-24 10:15 UTC**, after the code review and a full re-read of the manual, both addenda, the patch list and the OS 2.00 readme. Reference facts are in [SPEC.md](SPEC.md).
Status: ⬜ pending · 🔄 in progress · 🟡 PR open · ✅ done · ⏸️ parked

## ☀️ Start here

**The goal is an editor and librarian that never corrupts a sound (SPEC §1.1).** As of 24 Nov the rework is on branch **`rework/integration`** (not merged to `main`). All 256 factory patches round-trip byte-identically; the editor, Assign panel, librarian and MIDI layer are rebuilt on the verified parameter map. What's left needs the real synth (Q1–Q3, live tests) or is feature work.

1. ✅ **Baseline (24 Nov):** Rust 1.100.0 + VS 2022 Build Tools. Behind the corporate proxy, cargo needs `[http] check-revoke = false` in `%USERPROFILE%\.cargo\config.toml`.
2. ✅ **Sources re-read (24 Nov)** and SPEC.md rewritten from them.
3. ✅ **Data-loss bugs** D1–D8, **patch model** P1–P8, **import rules** L1–L4, **golden tests** T1–T4 (24 Nov, sub-agents + sub-chats).
4. **Next without hardware:** G1 (the GUI can't start on this VM), the red & black theme (V1), then features F1–F7 and U5.
5. **When the MIDI cable arrives:** live tests (G2), then Q1–Q3.
6. Review and merge `rework/integration` into `main`.
7. ✅ **Note audition** (owner request, N1–N9, SPEC §5.9): play notes on the Virus from the editor (piano strip, computer keys, Audition, hold, phrases, panic, no stuck notes). Left: live test N8, real Multi part channels N9.

## Where we stand

| Area | State |
|---|---|
| **Build** | Builds (debug + release); 98 tests pass; `clippy --workspace --all-targets -D warnings` clean (24 Nov, after the app rebuild) |
| **`virus-syx`** | Raw 256-byte `Patch`; parameter table from the addendum; lossless enums; SysEx parse/encode with checksum reporting; `.syx`/SMF reader tolerant of the damaged factory files. 30 tests incl. strict factory round trips |
| **`virus-midi`** | Plain transport: ports by name, channel-tagged input events, robust parser (R3), paced batches, all errors reported. 11 tests |
| **`virus-db`** | Byte-exact storage, schema v2 with migration, bank-aware import (one transaction) and export (A–D = 1–4), favourites/tags separate from data. 18 tests |
| **`virus-app`** | Rebuilt on the raw-byte crates (24 Nov): generated editor, Assign/LFO-assign panel, librarian with import report and bank transfer, name-based MIDI ports. Logic in `session.rs`/`library.rs`/`storage.rs` with unit tests. Not yet tried with real hardware |

### Evidence from the documents and factory data

- **Parameter map:** the Rack addendum's Single Dump table. Data byte `n` = parameter A`n`, byte `128+n` = B`n`. For example, Cutoff = A40, Filter Routing = A53, name = B112–121, categories = B123–124. The code uses different offsets for almost everything (SPEC §4.1–4.2).
- **Modulation:** the Virus has **Assign 1 (1 destination), Assign 2 (2), Assign 3 (3)** plus LFO1/LFO2 assign (B64–82). It does not have six slots of source + 2 destinations at B16+.
- **Bank byte:** `00` = edit buffer, `01–04` = A–D. Factory Bank A uses `01`, Bank B `02`.
- **Byte preservation:** **33 undocumented offsets hold data** in the factory patches, and the addendum says undefined bytes must be sent back unchanged. A simulated `from_bytes` → `to_bytes` round-trip alters **all 128 Bank A patches (1,625 bytes)**.
- **Values beyond the documented ranges:** saturation up to 30, delay mode up to 29, assign destination up to 114 (the printed list has 105 names). Enums must keep unknown raw values.
- **MIDI defaults:** PolyPressure for Page B is **off by default**. CC is received only on the synth's channel, and only by the lowest-numbered part on it.

## 🔴 Data loss (do first)

| # | Status | Item | Where | Notes |
|---|---|---|---|---|
| D1 | ✅ | The favourite star saves the **editor's** patch over the clicked row | `virus-app/src/views/librarian_panel.rs` (fav button) | Fixed: `PatchDatabase::set_favorite` (db rework); the app's star and checkbox call only that |
| D2 | ✅ | "💾 Store Active to DB" always writes to **slot 0** and reports success even on failure | `virus-app/src/app.rs` (header button) | Fixed: "💾 Store…" opens a dialog (bank + slot; default = selected or first free slot); errors shown |
| D3 | ✅ | `StoreActiveToSlot` ignores `save_patch` errors | `virus-app/src/app.rs` | Fixed: `library::store_patch` returns the `DbError`; shown in the dialog (tested) |
| D4 | ✅ | Export maps Bank A→`0` (edit buffer) and B→`1` (Bank A) | `virus-db` `export_bank_dumps` | Bank byte from the letter, A–D = 1–4; test `export_frames_use_bank_byte_and_exact_data` |
| D5 | ✅ | Incoming CC/SysEx param rebuilds the page from a **stale** `raw` buffer, so unsaved knob edits are lost | `virus-app/src/app.rs` `ParamReceived` | Fixed: one raw `Patch`; `Session::handle_midi` sets a single byte (tested) |
| D6 | ✅ | Incoming CCs on **any channel** are written to `raw[cc]`, including performance controllers (mod wheel CC1 → A1, volume CC7 → A7) | `virus-midi/src/engine.rs` `handle_incoming_midi_bytes` | Fixed in `virus-app/src/session.rs`: CC/PolyPressure only on the synth channel and only `is_sound_param`; SysEx param changes only for the edited part, never class `p`, A0 or Page C (tested) |
| D7 | ✅ | Non-ASCII names put bytes > 127 into program dumps (invalid SysEx); can split a UTF-8 char | `virus-syx/src/domain/page_a.rs` `set_name`; `synth_panel.rs` name box | Fixed: `Patch::set_name` + `session::sanitize_name` (ASCII 32–126, 10 chars); only changed name bytes are sent (tested) |
| D8 | ✅ | DB opened at a relative path; on failure the app silently falls back to in-memory | `virus-app/src/app.rs` `VirusApp::new` | Fixed: `storage.rs` opens the `directories` data dir (`…\VirusEditor\data\virus_library.db`); on failure a red "in memory only, NOT saved" banner with the reason (tested) |

## 🟠 Patch model rebuild (SPEC §4–§5)

| # | Status | Item | Notes |
|---|---|---|---|
| P1 | ✅ | **One parameter table** (page, number, name, class, kind: unipolar/bipolar/enum/bool/note/char, value texts) transcribed from SPEC §4.1–4.2. UI, MIDI and decoding all use it | `virus-syx` `params::SINGLE_PARAMS`; the editor is generated from it |
| P2 | ✅ | `Patch` = **`[u8; 256]` as the single source of truth**; typed get/set accessors touch only their own byte; `to_bytes(&self)` | Test: editing one parameter changes exactly one byte on all 256 factory patches |
| P3 | ✅ | Enums never lose data: `Unknown(u8)` variants; bools read/write the whole byte | `Category`, `AssignSource`; Choice values beyond the list kept and shown as `#n` |
| P4 | ✅ | Replace the invented 6-slot matrix with **Assign 1/2/3 (1+2+3 destinations) and LFO1/LFO2 assign** (B64–82); 28 sources (SPEC §4.5) | Done: Modulation tab from `ASSIGN_LAYOUT`/`LFO_ASSIGN_LAYOUT`; raw destination numbers with greyed unverified name hints (Q1); Definable 1/2 likewise |
| P5 | ✅ | Name and categories read from and written to the patch (B112–121, B123–124), using the 17 categories in SPEC §4.5 order | Done: editor header writes the patch bytes and sends B112–121/B123–124 param changes; DB derives its search copies from the bytes |
| P6 | ✅ | A0 Sound Version is read-only; performance controllers (class `p`) aren't editable patch fields | Done: Sound Version shown read-only; `Internal`/`Performance` rows not in the editor; `Session::edit` refuses A0 |
| P7 | ✅ | Rebuild the synth and matrix panels around the real Rack parameters: Osc Shape continuum + Wave Select, Sub osc, Osc balance/volume, Saturation, Filter Select, envelope Sustain Time, LFO3, Unison, Punch, Velocity, Chorus, Delay/Reverb, Phaser, Vocoder, Arp, Clocks | Done: editor generated from `SINGLE_PARAMS` by section (knobs / combos keeping `#n` / checkboxes / note); curves fed from A40–A43, A51–A53, B32, A54–A63. A test checks every sound parameter has exactly one control |
| P8 | ✅ | Live edits send SysEx `70`/`71` to part `40` (or the selected Multi part); never PolyPressure | Done: `Session::edit` → `encode_param_change` to `0x40` or Multi part 1–16 (tested) |

## 🧪 Tests (SPEC §6)

| # | Status | Item | Notes |
|---|---|---|---|
| T1 | ✅ | **Golden tests:** `crates/virus-syx/tests/factory.rs` (damaged `.mid` originals; exact bad-checksum counts) and `tests/clean_factory.rs` (recovered `testdata/factory/Bank_A/B.syx`): all 256 patches parse with valid checksums and round-trip **byte-identically**; names and categories decode; editing one parameter changes exactly one byte | Recovered banks from the SoundDiver `.lib` (sub-chat, 24 Nov) |
| T2 | ✅ | DB tests: the favourite toggle and slot store don't touch patch data | `favourite_and_tags_do_not_touch_data`, `save_and_load_is_byte_exact` |
| T3 | ✅ | Incoming param doesn't revert other edits; performance CCs and other channels are ignored | Covers D5–D6. `virus-app/src/session.rs` tests |
| T4 | ✅ | Import `multis.mid`: banks A and B land in A and B; edit-buffer and multi dumps aren't put in bank slots | Routing unit-tested (`import_original_banks_routes_by_bank_byte`); `multis.mid` checked end-to-end by hand (A 128, B 128, 33 loose, 129 multi + 1 other skipped). Not a committed test |

## 📚 Librarian import and export (SPEC §5.6)

| # | Status | Item | Notes |
|---|---|---|---|
| L1 | ✅ | Import honours `bb`: `01–04` → bank A–D; `00` → edit-buffer dump (offer it as a loose patch, never slot = part number) | Done: `import_single_dumps` (db) + app lists loose dumps with Load / Store… |
| L2 | ✅ | Multi files: `multis.mid` is a Total dump with A, B, edit buffers, 128 multis and a `0x12` message. Import singles per bank; keep multis and unknown messages raw (or skip with a count) | Done: singles per bank; multis and other messages skipped with counts in the import report (checked on the real `multis.mid`: 256 imported, 33 loose, 129 multi + 1 other skipped) |
| L3 | ✅ | Report checksum mismatches and skipped messages in the import result rather than silently dropping them | Done: import summary + ⚠ per patch (factory Bank A: 20 flagged, as SPEC §6) |
| L4 | ✅ | Banks C/D are ROM: don't offer "send bank" to C/D; warn that sending A/B overwrites RAM irrevocably, and offer a backup first | Done: "Send bank to synth…" only for library banks A/B, with a warning dialog and a "Back up synth bank" request; received dumps can be imported as a new library bank |
| L5 | ⏸️ | SoundDiver `.lib` import | Proprietary format; only if needed |

## 🚀 Features toward the goal (SPEC §1.1, §8)

| # | Status | Item | Notes |
|---|---|---|---|
| F1 | ⬜ | **Request banks** A–D from the synth (`32 bb`) into the library, with progress | Also the backup in L4 |
| F2 | ⬜ | **Send a patch to a bank slot** (single dump with `bb=01/02`), with confirmation | RAM banks only |
| F3 | ⬜ | Read the synth's globals (Global request `35`, or Page C): device ID, global channel, C94/C95 Low/High Page, play mode | Lets the app pick the right channel and part, and warn when PolyPressure or CC is off |
| F4 | ⬜ | Multi mode: choose the part (`00–0F`) for edits and fetches; part list from the Multi edit buffer | SPEC §4.4 |
| F5 | ⬜ | Multi editor and librarian (dump `11`, layout SPEC §4.4) | Later |
| F6 | ⬜ | "Pass through the synth" step after import (send to edit buffer, request back) so the Virus updates the sound version | SPEC §5.2 |
| F7 | 🔄 | Bank/program change via SysEx (C31–33) so selecting a library patch can also select it on the synth | SPEC §3.5. Done: opt-in "Select on synth" checkbox in the Librarian sends C32 + C33 to the edited part on **Load** from bank A–D (`synth_select.rs`, tested). Open: the C32 bank value is sent 0-based (A=0…D=3), unverified on hardware (G2) |
| F8 | ⬜ | Banks E–H (1,024-sound OS) | Blocked on Q2 |

## 🎹 Note audition (SPEC §5.9)

| # | Status | Item | Notes |
|---|---|---|---|
| N1 | ✅ | Pure MIDI builders `note_on`/`note_off`/`control_change`/`all_notes_off`/`all_sound_off` | `virus-midi/src/notes.rs` |
| N2 | ✅ | `NoteTracker`: every note on tracked per channel; note offs on release, octave/channel/part change, disconnect, focus loss, exit; panic = tracked note offs + CC123 + CC120 (+ CC64 0) | `virus-midi/src/notes.rs`; tests for each trigger |
| N3 | ✅ | `PhrasePlayer`: plays a list of (start ms, note, vel, length ms) from a clock the GUI passes in; stop releases sounding notes; built-in test phrases (note, chord, bass line) at a tempo | `virus-midi/src/notes.rs` |
| N4 | ✅ | egui **piano keyboard** widget: click/drag, velocity from click height or a fixed slider, held keys highlighted | `virus-app/src/widgets/piano.rs`; geometry and velocity tested |
| N5 | ✅ | Audition state: computer keys `A W S E D F T G Y H U J K` = C..C, `Z`/`X` octave, only without text focus, no key-repeat retrigger; octave shift; hold/latch; Audition button (note/chord, length); test phrases with tempo | `virus-app/src/audition.rs`, tested |
| N6 | ✅ | Wired into the app: collapsible "🎹 Keyboard" strip above the MIDI bar on every tab; channel = synth channel (Single) or part *n* → channel *n* (Multi, see N9), with a manual override; disabled without output | `app.rs`, `audition::auto_channel` / `sync_target` |
| N7 | ✅ | Safety wiring: note offs before output disconnect/change (also when a port vanishes), on channel and part change, window focus loss, collapsing the strip, and app exit (`on_exit` waits ≤ 300 ms for the worker to send them); Panic button | `app.rs` `release_notes_before`, `on_exit` |
| N8 | ⬜ | **Live test when the MIDI cable arrives:** notes sound on the Global Channel in Single mode and on the part channel in Multi mode; velocity changes the sound with B60 Velocity→Amp ≠ 0; Hold latches the arp (B1 ≠ Off); Panic silences long releases; no stuck notes after unplugging/alt-tab/quitting; note 60 is C3 on the Virus display; Tx LED flashes; computer keys don't play while typing in the name/search fields | Needs the synth. Also check the UI on a machine with OpenGL 2+ (this dev box can't start the app) |
| N9 | ⬜ | Multi mode: use the part's real MIDI channel (C34 / multi dump bytes 64–79) instead of assuming part *n* = channel *n* | Needs F4 (read the Multi edit buffer) |

## 🖥️ Running and live testing

| # | Status | Item | Notes |
|---|---|---|---|
| G1 | ⬜ | **The GUI can't start on this dev VM** (Hyper-V over RDP, no GPU): `egui_glow requires opengl 2.0+`; wgpu also fails (`NoSuitableAdapterFound`). This happens before any app code runs; the old app had the same backend | Check on a machine with a GPU first. Fallback options: a software OpenGL (Mesa `opengl32.dll` / llvmpipe next to the exe), or the wgpu renderer with a fallback adapter. Not done |
| G2 | ⬜ | **Live test checklist** (when the MIDI cable arrives): ports auto-detect; device ID/channel; fetch edit buffer (`30 00 40`) loads the sound; knob → SysEx `70/71` audible; incoming knob moves update the editor; send to edit buffer; request bank A (`32 01`) lands in "Received from synth"; back up then send bank A with 150 ms gaps (no buffer overflow, display shows no checksum error); Multi part addressing; note audition (N items) | Record results here; adjust `BANK_SEND_GAP_MS` if needed |
| G3 | ✅ | Persist settings between runs (ports, channel, device ID, part, window) | Versioned `settings.json` in the data dir (`settings.rs`, tested): ports, channel, device ID, part, audition settings (Hold always starts off), tab, bank, import/export folders, window geometry. Bad fields fall back to defaults, a corrupt file becomes `.bak` with a banner, vanished ports aren't selected. The window position isn't checked against the monitor layout |

## ❓ Open questions (SPEC §7)

| # | Question | Blocks |
|---|---|---|
| Q1 | Raw value → name for assign / LFO-assign / definable destinations (factory values up to 114; the list has 105 names) | P4 labels. **Live test when the MIDI cable arrives** |
| Q2 | SysEx addressing of ROM banks E–H | F8. Live test |
| Q3 | Full value lists for A49 Saturation, A112 Delay/Reverb mode, A102 Input Select, B39 Vocoder mode | P7 labels. Live test |
| ~~Q4~~ | ✅ Decided 24 Nov: the repo is **private** and stays private, so the Access material stays | — |
| ~~Q5~~ | ✅ Decided 24 Nov: **Virus Rack (classic) only** for now | — |

## 🟡 Robustness

| # | Status | Item | Where |
|---|---|---|---|
| R1 | ✅ | Connect MIDI ports by **name**, not index; clear stale selection after refresh | Done: `session::Ports` (auto-detect via `find_virus_ports`, stale selections cleared and disconnected; tested) |
| R2 | ✅ | Report MIDI send failures; don't flash Tx when no output is connected | Done: Tx lights only on `MidiEvent::Sent`; errors in the status bar; nothing is sent without an output |
| R3 | ✅ | SysEx assembler: abort a frame on a non-real-time status byte (0x80–0xEF, 0xF1–0xF6) | `virus-midi` parser; test `status_byte_aborts_sysex_and_is_processed` |
| R4 | ✅ | Wrap `import_syx_bank` in one transaction | `import_single_dumps` writes in one transaction (rollback tested) |
| R5 | ✅ | *(moved to L1)* | |
| R6 | ✅ | Show an error, not a zeroed patch, when a stored blob isn't 256 bytes | `DbError::CorruptPatch`; test `corrupt_blob_is_an_error` |
| R7 | ✅ | Escape `%` and `_` in librarian search (`LIKE … ESCAPE`) | `escape_like`; test `query_filters_and_like_escaping` |
| R8 | ✅ | Show export write failures in the status bar | Done: `library::export_bank_file` errors (empty bank, write failure) shown (tested) |

## ⚪ Performance and UX

| # | Status | Item | Notes |
|---|---|---|---|
| U1 | ✅ | Librarian queries the DB every frame; cache results and refresh on change | Done: `library::LibraryCache` re-queries only when `data_version()` or the query changes (tested) |
| U2 | ✅ | Remove the 30 ms forced repaint; repaint on MIDI events and while LEDs are lit | Done: `set_event_notifier` → `request_repaint`; `request_repaint_after` only while an LED glows |
| U3 | ✅ | Allow storing into empty slots (show all 128 slots per bank) | Done: 128-slot list from `get_bank_slots`, empty slots selectable |
| U4 | ✅ | Confirm before overwriting a slot | Done: the store dialog names the occupant and the button becomes "Overwrite A005" |
| U5 | ✅ | Move file dialogs and import off the UI thread | `dialogs.rs`: `rfd::AsyncFileDialog` and file read/write run on one worker thread, one task at a time, with a busy indicator. Parsing and the DB insert still run on the UI thread (well under a second per bank) |

## 🎨 Look and feel

| # | Status | Item | Notes |
|---|---|---|---|
| V1 | ⬜ | **Match the Virus Rack's colours: red and black** (owner, 24 Nov). The front panel is signal red with black legends; the chassis, knobs and buttons are black or dark grey; the LEDs are amber; the LCD is dark with light characters | Palette in SPEC §5.10. Apply to the egui `Visuals` theme, knob widget, LEDs, tabs/headers and curve views. Keep text contrast readable (WCAG AA) on red. One central `theme.rs` so it can be tuned later |

## 🧹 Housekeeping

| # | Status | Item | Notes |
|---|---|---|---|
| H1 | ✅ | Stop tracking `virus_library.db`, `-shm`, `-wal`; add `*.db*` to `.gitignore` | `git rm --cached` |
| H2 | ✅ | Commit `Cargo.lock` (it's an application); remove it from `.gitignore` | |
| H3 | ✅ | Clippy clean-up: unused `bounded`/`Duration` imports in `engine.rs`; `Arc<Mutex>` around the assembler isn't needed; `SoundCategory::from_str` → implement `FromStr` | Done with the crate rewrites: `cargo clippy --workspace --all-targets -- -D warnings` passes (24 Nov) |
| H4 | ✅ | `Patch::from_sysex` returns `InvalidPayloadLength` for any non-dump message; add a proper error | `SyxError::NotASingleDump` |
| H5 | ✅ | Short-message parsing: check `F7` sits right after the payload | Test `fixed_size_messages_require_exact_length` |
| H6 | ✅ | SPEC.md: `D:\VirusEditor` paths and the unverified "Phase 1" claims corrected (24 Nov rewrite) | |
| H7 | ⬜ | Add a CI workflow (build, test, clippy) on GitHub Actions | |
| H8 | ✅ | **16 `float_literal_f32_fallback` warnings will become hard errors** in a future Rust release: write `1.0_f32` (e.g. in `Stroke::new(1.0, …)`) | Done: app rewritten with typed literals; no warnings or future-incompat notes |
| H9 | ✅ | Q4 decided: the repo stays private, so the Access material stays | |
| H10 | ⬜ | README: say "Virus Rack (classic), OS 2.00" as the verified target, and mention open questions Q1/Q5 | |

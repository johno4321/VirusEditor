# VirusEditor todo list

**Updated Tue 2026-11-24 10:15 UTC**, after the code review and a full re-read of the manual, both addenda, the patch list and the OS 2.00 readme. Reference facts are in [SPEC.md](SPEC.md).
Status: ⬜ pending · 🔄 in progress · 🟡 PR open · ✅ done · ⏸️ parked

## ☀️ Start here

**The goal is an editor and librarian that never corrupts a sound (SPEC §1.1).** Today, loading and re-saving a patch changes it, most knobs edit the wrong parameter on the synth, and several librarian actions overwrite stored patches. SPEC.md now has the verified parameter map (§4) and the rules the code must follow (§5).

1. ✅ **Baseline (24 Nov):** Rust 1.100.0 + VS 2022 Build Tools. `cargo test --workspace`: **16 passed, 0 failed**. `cargo clippy --workspace --all-targets`: no errors, ~40 warnings (see H8). Behind the corporate proxy, cargo needs `[http] check-revoke = false` in `%USERPROFILE%\.cargo\config.toml`.
2. ✅ **Sources re-read (24 Nov)** and SPEC.md rewritten: parameter tables, bank numbers, MIDI defaults, editor rules, open questions.
3. **Fix the quick data-loss bugs** (D1–D4). Each is a few lines.
4. **Add the factory golden tests** (T1) before touching the patch model, so the rework has a target.
5. **Rebuild the patch model** (P1–P8) from SPEC §4–§5.
6. Then the librarian import rules (L1–L4), and only then new features (F1–F8).
7. **New (owner request): note audition** (N1–N8, SPEC §5.9): play notes on the Virus from the editor to hear patch changes — on-screen piano, computer keyboard, Audition button, hold, panic, no stuck notes. Built on branch `rework/note-audition`.

## Where we stand

| Area | State |
|---|---|
| **Build** | Builds (debug + release); 98 tests pass; `clippy --workspace --all-targets -D warnings` clean (24 Nov, after the app rebuild) |
| **`virus-syx`** | SysEx framing, checksum and encoders are right. **The parameter map is wrong almost everywhere** (SPEC §4). It invents a 6-slot matrix the Virus doesn't have, and decoding normalises bytes (breaks SPEC §5.1) |
| **`virus-midi`** | Thread, channels and SysEx assembler work. Incoming CCs on any channel are treated as parameters; ports chosen by index; send errors ignored |
| **`virus-db`** | Schema and queries fine. Export bank numbering wrong; import ignores the bank byte; names and categories duplicated outside the patch |
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
| D4 | ⬜ | Export maps Bank A→`0` (edit buffer) and B→`1` (Bank A) | `virus-db/src/db.rs` `export_syx_bank` | Use A=1, B=2, C=3, D=4 (SPEC §3.2) |
| D5 | ✅ | Incoming CC/SysEx param rebuilds the page from a **stale** `raw` buffer, so unsaved knob edits are lost | `virus-app/src/app.rs` `ParamReceived` | Fixed: one raw `Patch`; `Session::handle_midi` sets a single byte (tested) |
| D6 | ✅ | Incoming CCs on **any channel** are written to `raw[cc]`, including performance controllers (mod wheel CC1 → A1, volume CC7 → A7) | `virus-midi/src/engine.rs` `handle_incoming_midi_bytes` | Fixed in `virus-app/src/session.rs`: CC/PolyPressure only on the synth channel and only `is_sound_param`; SysEx param changes only for the edited part, never class `p`, A0 or Page C (tested) |
| D7 | ✅ | Non-ASCII names put bytes > 127 into program dumps (invalid SysEx); can split a UTF-8 char | `virus-syx/src/domain/page_a.rs` `set_name`; `synth_panel.rs` name box | Fixed: `Patch::set_name` + `session::sanitize_name` (ASCII 32–126, 10 chars); only changed name bytes are sent (tested) |
| D8 | ✅ | DB opened at a relative path; on failure the app silently falls back to in-memory | `virus-app/src/app.rs` `VirusApp::new` | Fixed: `storage.rs` opens the `directories` data dir (`…\VirusEditor\data\virus_library.db`); on failure a red "in memory only, NOT saved" banner with the reason (tested) |

## 🟠 Patch model rebuild (SPEC §4–§5)

| # | Status | Item | Notes |
|---|---|---|---|
| P1 | ⬜ | **One parameter table** (page, number, name, class, kind: unipolar/bipolar/enum/bool/note/char, value texts) transcribed from SPEC §4.1–4.2. UI, MIDI and decoding all use it | Replaces the hand-written offsets in `page_a.rs`/`page_b.rs`. Each entry cites the addendum row |
| P2 | ⬜ | `Patch` = **`[u8; 256]` as the single source of truth**; typed get/set accessors touch only their own byte; `to_bytes(&self)` | SPEC §5.1. Removes the stale-buffer bugs (D5) |
| P3 | ⬜ | Enums never lose data: `Unknown(u8)` variants; bools read/write the whole byte | Factory values exceed the documented ranges (A49 ≤ 30, A112 ≤ 29, destinations ≤ 114) |
| P4 | ✅ | Replace the invented 6-slot matrix with **Assign 1/2/3 (1+2+3 destinations) and LFO1/LFO2 assign** (B64–82); 28 sources (SPEC §4.5) | Done: Modulation tab from `ASSIGN_LAYOUT`/`LFO_ASSIGN_LAYOUT`; raw destination numbers with greyed unverified name hints (Q1); Definable 1/2 likewise |
| P5 | ✅ | Name and categories read from and written to the patch (B112–121, B123–124), using the 17 categories in SPEC §4.5 order | Done: editor header writes the patch bytes and sends B112–121/B123–124 param changes; DB derives its search copies from the bytes |
| P6 | ✅ | A0 Sound Version is read-only; performance controllers (class `p`) aren't editable patch fields | Done: Sound Version shown read-only; `Internal`/`Performance` rows not in the editor; `Session::edit` refuses A0 |
| P7 | ✅ | Rebuild the synth and matrix panels around the real Rack parameters: Osc Shape continuum + Wave Select, Sub osc, Osc balance/volume, Saturation, Filter Select, envelope Sustain Time, LFO3, Unison, Punch, Velocity, Chorus, Delay/Reverb, Phaser, Vocoder, Arp, Clocks | Done: editor generated from `SINGLE_PARAMS` by section (knobs / combos keeping `#n` / checkboxes / note); curves fed from A40–A43, A51–A53, B32, A54–A63. A test checks every sound parameter has exactly one control |
| P8 | ✅ | Live edits send SysEx `70`/`71` to part `40` (or the selected Multi part); never PolyPressure | Done: `Session::edit` → `encode_param_change` to `0x40` or Multi part 1–16 (tested) |

## 🧪 Tests (SPEC §6)

| # | Status | Item | Notes |
|---|---|---|---|
| T1 | ✅ | **Golden tests:** `crates/virus-syx/tests/factory.rs` (damaged `.mid` originals; exact bad-checksum counts) and `tests/clean_factory.rs` (recovered `testdata/factory/Bank_A/B.syx`): all 256 patches parse with valid checksums and round-trip **byte-identically**; names and categories decode; editing one parameter changes exactly one byte | Recovered banks from the SoundDiver `.lib` (sub-chat, 24 Nov) |
| T2 | ⬜ | DB tests: the favourite toggle and slot store don't touch patch data | Covers D1–D2 |
| T3 | ✅ | Incoming param doesn't revert other edits; performance CCs and other channels are ignored | Covers D5–D6. `virus-app/src/session.rs` tests |
| T4 | ⬜ | Import `multis.mid`: banks A and B land in A and B; edit-buffer and multi dumps aren't put in bank slots | Covers L1–L2 |

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
| F7 | ⬜ | Bank/program change via SysEx (C31–33) so selecting a library patch can also select it on the synth | SPEC §3.5 |
| F8 | ⬜ | Banks E–H (1,024-sound OS) | Blocked on Q2 |

## 🎹 Note audition (SPEC §5.9)

| # | Status | Item | Notes |
|---|---|---|---|
| N1 | ✅ | Pure MIDI builders `note_on`/`note_off`/`control_change`/`all_notes_off`/`all_sound_off` | `virus-midi/src/notes.rs` |
| N2 | ✅ | `NoteTracker`: every note on tracked per channel; note offs on release, octave/channel/part change, disconnect, focus loss, exit; panic = tracked note offs + CC123 + CC120 (+ CC64 0) | `virus-midi/src/notes.rs`; tests for each trigger |
| N3 | ✅ | `PhrasePlayer`: plays a list of (start ms, note, vel, length ms) from a clock the GUI passes in; stop releases sounding notes; built-in test phrases (note, chord, bass line) at a tempo | `virus-midi/src/notes.rs` |
| N4 | 🔄 | egui **piano keyboard** widget: click/drag, velocity from click height or a fixed slider, held keys highlighted | `virus-app/src/widgets/piano.rs` |
| N5 | 🔄 | Audition state: computer keys `A W S E D F T G Y H U J K` = C..C, `Z`/`X` octave, only without text focus, no key-repeat retrigger; octave shift; hold/latch; Audition button (note/chord, length) | `virus-app/src/audition.rs`. Written and unit-tested in a standalone check crate; `mod` declarations added when wired (N6) |
| N6 | ⬜ | Wire into the app: collapsible strip above the MIDI bar on every tab; target channel = Global Channel (Single) or the edited part's channel (Multi); disabled without output | After the app rebuild lands |
| N7 | ⬜ | Safety wiring: note offs on disconnect, part/channel change, window focus loss, app exit; Panic button | After the app rebuild lands |
| N8 | ⬜ | **Live test when the MIDI cable arrives:** notes sound on the Global Channel in Single mode and on the part channel in Multi mode; velocity changes the sound with B60 Velocity→Amp ≠ 0; Hold latches the arp (B1 ≠ Off); Panic silences long releases; no stuck notes after unplugging/alt-tab; note 60 is C3 on the Virus display; Tx LED flashes | Needs the synth |

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
| R3 | ⬜ | SysEx assembler: abort a frame on a non-real-time status byte (0x80–0xEF, 0xF1–0xF6) | `virus-midi/src/stream.rs` |
| R4 | ⬜ | Wrap `import_syx_bank` in one transaction | `virus-db/src/db.rs` |
| R5 | ✅ | *(moved to L1)* | |
| R6 | ⬜ | Show an error, not a zeroed patch, when a stored blob isn't 256 bytes | `row_to_patch_record` |
| R7 | ⬜ | Escape `%` and `_` in librarian search (`LIKE … ESCAPE`) | `query_patches` |
| R8 | ✅ | Show export write failures in the status bar | Done: `library::export_bank_file` errors (empty bank, write failure) shown (tested) |

## ⚪ Performance and UX

| # | Status | Item | Notes |
|---|---|---|---|
| U1 | ✅ | Librarian queries the DB every frame; cache results and refresh on change | Done: `library::LibraryCache` re-queries only when `data_version()` or the query changes (tested) |
| U2 | ✅ | Remove the 30 ms forced repaint; repaint on MIDI events and while LEDs are lit | Done: `set_event_notifier` → `request_repaint`; `request_repaint_after` only while an LED glows |
| U3 | ✅ | Allow storing into empty slots (show all 128 slots per bank) | Done: 128-slot list from `get_bank_slots`, empty slots selectable |
| U4 | ✅ | Confirm before overwriting a slot | Done: the store dialog names the occupant and the button becomes "Overwrite A005" |
| U5 | ⬜ | Move file dialogs and import off the UI thread | Still synchronous (a bank imports in well under a second) |

## 🧹 Housekeeping

| # | Status | Item | Notes |
|---|---|---|---|
| H1 | ✅ | Stop tracking `virus_library.db`, `-shm`, `-wal`; add `*.db*` to `.gitignore` | `git rm --cached` |
| H2 | ✅ | Commit `Cargo.lock` (it's an application); remove it from `.gitignore` | |
| H3 | ✅ | Clippy clean-up: unused `bounded`/`Duration` imports in `engine.rs`; `Arc<Mutex>` around the assembler isn't needed; `SoundCategory::from_str` → implement `FromStr` | Done with the crate rewrites: `cargo clippy --workspace --all-targets -- -D warnings` passes (24 Nov) |
| H4 | ⬜ | `Patch::from_sysex` returns `InvalidPayloadLength` for any non-dump message; add a proper error | `patch.rs` |
| H5 | ⬜ | Short-message parsing: check `F7` sits right after the payload | `sysex.rs` |
| H6 | ✅ | SPEC.md: `D:\VirusEditor` paths and the unverified "Phase 1" claims corrected (24 Nov rewrite) | |
| H7 | ⬜ | Add a CI workflow (build, test, clippy) on GitHub Actions | |
| H8 | ✅ | **16 `float_literal_f32_fallback` warnings will become hard errors** in a future Rust release: write `1.0_f32` (e.g. in `Stroke::new(1.0, …)`) | Done: app rewritten with typed literals; no warnings or future-incompat notes |
| H9 | ✅ | Q4 decided: the repo stays private, so the Access material stays | |
| H10 | ⬜ | README: say "Virus Rack (classic), OS 2.00" as the verified target, and mention open questions Q1/Q5 | |

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

## Where we stand

| Area | State |
|---|---|
| **Build** | Builds; 16/16 tests pass; clippy has warnings only (24 Nov) |
| **`virus-syx`** | SysEx framing, checksum and encoders are right. **The parameter map is wrong almost everywhere** (SPEC §4). It invents a 6-slot matrix the Virus doesn't have, and decoding normalises bytes (breaks SPEC §5.1) |
| **`virus-midi`** | Thread, channels and SysEx assembler work. Incoming CCs on any channel are treated as parameters; ports chosen by index; send errors ignored |
| **`virus-db`** | Schema and queries fine. Export bank numbering wrong; import ignores the bank byte; names and categories duplicated outside the patch |
| **`virus-app`** | UI complete. Edits the wrong parameters; favourite toggle and "Store Active to DB" overwrite patches; incoming MIDI reverts edits |

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
| D1 | ⬜ | The favourite star saves the **editor's** patch over the clicked row | `virus-app/src/views/librarian_panel.rs` (fav button) | Add `set_favorite(id, bool)` to `PatchDatabase` and update only the flag |
| D2 | ⬜ | "💾 Store Active to DB" always writes to **slot 0** and reports success even on failure | `virus-app/src/app.rs` (header button) | Ask for the slot (or the first free one); show errors |
| D3 | ⬜ | `StoreActiveToSlot` ignores `save_patch` errors | `virus-app/src/app.rs` | Report errors in the status bar |
| D4 | ⬜ | Export maps Bank A→`0` (edit buffer) and B→`1` (Bank A) | `virus-db/src/db.rs` `export_syx_bank` | Use A=1, B=2, C=3, D=4 (SPEC §3.2) |
| D5 | ⬜ | Incoming CC/SysEx param rebuilds the page from a **stale** `raw` buffer, so unsaved knob edits are lost | `virus-app/src/app.rs` `ParamReceived` | Goes away with P2; short-term, call `sync_raw_from_fields()` first |
| D6 | ⬜ | Incoming CCs on **any channel** are written to `raw[cc]`, including performance controllers (mod wheel CC1 → A1, volume CC7 → A7) | `virus-midi/src/engine.rs` `handle_incoming_midi_bytes` | SPEC §5.5: accept only on the synth's channel and only class `a`/`b` parameters. Apply SysEx param changes only for the edited part |
| D7 | ⬜ | Non-ASCII names put bytes > 127 into program dumps (invalid SysEx); can split a UTF-8 char | `virus-syx/src/domain/page_a.rs` `set_name`; `synth_panel.rs` name box | SPEC §5.4: ASCII 32–127, 10 chars, space-padded |
| D8 | ⬜ | DB opened at a relative path; on failure the app silently falls back to in-memory | `virus-app/src/app.rs` `VirusApp::new` | Use the user data dir (e.g. `directories` crate); show a visible error |

## 🟠 Patch model rebuild (SPEC §4–§5)

| # | Status | Item | Notes |
|---|---|---|---|
| P1 | ⬜ | **One parameter table** (page, number, name, class, kind: unipolar/bipolar/enum/bool/note/char, value texts) transcribed from SPEC §4.1–4.2. UI, MIDI and decoding all use it | Replaces the hand-written offsets in `page_a.rs`/`page_b.rs`. Each entry cites the addendum row |
| P2 | ⬜ | `Patch` = **`[u8; 256]` as the single source of truth**; typed get/set accessors touch only their own byte; `to_bytes(&self)` | SPEC §5.1. Removes the stale-buffer bugs (D5) |
| P3 | ⬜ | Enums never lose data: `Unknown(u8)` variants; bools read/write the whole byte | Factory values exceed the documented ranges (A49 ≤ 30, A112 ≤ 29, destinations ≤ 114) |
| P4 | ⬜ | Replace the invented 6-slot matrix with **Assign 1/2/3 (1+2+3 destinations) and LFO1/LFO2 assign** (B64–82); 28 sources (SPEC §4.5) | Destination names: show raw numbers where the mapping is unconfirmed (Q1) |
| P5 | ⬜ | Name and categories read from and written to the patch (B112–121, B123–124), using the 17 categories in SPEC §4.5 order | Fix `SoundCategory` order ("Pluck" = 5) and stop keeping separate DB copies that can drift (or derive them) |
| P6 | ⬜ | A0 Sound Version is read-only; performance controllers (class `p`) aren't editable patch fields | SPEC §5.2 |
| P7 | ⬜ | Rebuild the synth and matrix panels around the real Rack parameters: Osc Shape continuum + Wave Select, Sub osc, Osc balance/volume, Saturation, Filter Select, envelope Sustain Time, LFO3, Unison, Punch, Velocity, Chorus, Delay/Reverb, Phaser, Vocoder, Arp, Clocks | Drop the invented "3-band EQ"; the Rack has no EQ parameters |
| P8 | ⬜ | Live edits send SysEx `70`/`71` to part `40` (or the selected Multi part); never PolyPressure | PolyPressure output is off by default (C95) |

## 🧪 Tests (SPEC §6)

| # | Status | Item | Notes |
|---|---|---|---|
| T1 | ✅ | **Golden tests:** `crates/virus-syx/tests/factory.rs` (damaged `.mid` originals; exact bad-checksum counts) and `tests/clean_factory.rs` (recovered `testdata/factory/Bank_A/B.syx`): all 256 patches parse with valid checksums and round-trip **byte-identically**; names and categories decode; editing one parameter changes exactly one byte | Recovered banks from the SoundDiver `.lib` (sub-chat, 24 Nov) |
| T2 | ⬜ | DB tests: the favourite toggle and slot store don't touch patch data | Covers D1–D2 |
| T3 | ⬜ | Incoming param doesn't revert other edits; performance CCs and other channels are ignored | Covers D5–D6 |
| T4 | ⬜ | Import `multis.mid`: banks A and B land in A and B; edit-buffer and multi dumps aren't put in bank slots | Covers L1–L2 |

## 📚 Librarian import and export (SPEC §5.6)

| # | Status | Item | Notes |
|---|---|---|---|
| L1 | ⬜ | Import honours `bb`: `01–04` → bank A–D; `00` → edit-buffer dump (offer it as a loose patch, never slot = part number) | Today the bank byte is ignored and `prog 0x40` lands at slot 64 |
| L2 | ⬜ | Multi files: `multis.mid` is a Total dump with A, B, edit buffers, 128 multis and a `0x12` message. Import singles per bank; keep multis and unknown messages raw (or skip with a count) | Today B would overwrite A |
| L3 | ⬜ | Report checksum mismatches and skipped messages in the import result rather than silently dropping them | The Virus itself accepts wrong checksums (SPEC §3.2) |
| L4 | ⬜ | Banks C/D are ROM: don't offer "send bank" to C/D; warn that sending A/B overwrites RAM irrevocably, and offer a backup first | SPEC §5.7; manual p. 177 |
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
| R1 | ⬜ | Connect MIDI ports by **name**, not index; clear stale selection after refresh | `engine.rs`, `midi_bar.rs`, `app.rs` |
| R2 | ⬜ | Report MIDI send failures; don't flash Tx when no output is connected | `engine.rs`, `app.rs` |
| R3 | ⬜ | SysEx assembler: abort a frame on a non-real-time status byte (0x80–0xEF, 0xF1–0xF6) | `virus-midi/src/stream.rs` |
| R4 | ⬜ | Wrap `import_syx_bank` in one transaction | `virus-db/src/db.rs` |
| R5 | ✅ | *(moved to L1)* | |
| R6 | ⬜ | Show an error, not a zeroed patch, when a stored blob isn't 256 bytes | `row_to_patch_record` |
| R7 | ⬜ | Escape `%` and `_` in librarian search (`LIKE … ESCAPE`) | `query_patches` |
| R8 | ⬜ | Show export write failures in the status bar | `librarian_panel.rs` |

## ⚪ Performance and UX

| # | Status | Item | Notes |
|---|---|---|---|
| U1 | ⬜ | Librarian queries the DB every frame; cache results and refresh on change | `librarian_panel.rs` |
| U2 | ⬜ | Remove the 30 ms forced repaint; repaint on MIDI events and while LEDs are lit | `app.rs` |
| U3 | ⬜ | Allow storing into empty slots (show all 128 slots per bank) | `librarian_panel.rs` |
| U4 | ⬜ | Confirm before overwriting a slot | `librarian_panel.rs` |
| U5 | ⬜ | Move file dialogs and import off the UI thread | `librarian_panel.rs` |

## 🧹 Housekeeping

| # | Status | Item | Notes |
|---|---|---|---|
| H1 | ✅ | Stop tracking `virus_library.db`, `-shm`, `-wal`; add `*.db*` to `.gitignore` | `git rm --cached` |
| H2 | ✅ | Commit `Cargo.lock` (it's an application); remove it from `.gitignore` | |
| H3 | ⬜ | Clippy clean-up: unused `bounded`/`Duration` imports in `engine.rs`; `Arc<Mutex>` around the assembler isn't needed; `SoundCategory::from_str` → implement `FromStr` | |
| H4 | ⬜ | `Patch::from_sysex` returns `InvalidPayloadLength` for any non-dump message; add a proper error | `patch.rs` |
| H5 | ⬜ | Short-message parsing: check `F7` sits right after the payload | `sysex.rs` |
| H6 | ✅ | SPEC.md: `D:\VirusEditor` paths and the unverified "Phase 1" claims corrected (24 Nov rewrite) | |
| H7 | ⬜ | Add a CI workflow (build, test, clippy) on GitHub Actions | |
| H8 | ⬜ | **16 `float_literal_f32_fallback` warnings will become hard errors** in a future Rust release: write `1.0_f32` (e.g. in `Stroke::new(1.0, …)`) | `app.rs`, `midi_bar.rs`, `curves.rs`, `knob.rs`. Also: unused imports, dead code, collapsible `if`s, `save_patch` has 8 args |
| H9 | ✅ | Q4 decided: the repo stays private, so the Access material stays | |
| H10 | ⬜ | README: say "Virus Rack (classic), OS 2.00" as the verified target, and mention open questions Q1/Q5 | |

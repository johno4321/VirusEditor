# Code review: `rework/integration` @ 5df9bb7 vs `774dfa7`

Scope: `git diff 774dfa7..HEAD`, all four crates, against SPEC.md §3–§5, TODO.md and the Rack addendum's Single Dump table (pp. 13–24).

Baseline: `cargo test --workspace` passes all 98 tests (app 39, db 18, midi 11, syx 24 + 3 + 3). `cargo clippy --workspace --all-targets -- -D warnings` is clean.

The library crates are in good shape. The SysEx framing, checksums, bank bytes, parameter table and MIDI parser all match the spec. Most of the remaining risk is in **librarian workflows**, where library or synth data can be overwritten or damaged without the user noticing. No Critical findings.

---

## High

### H1. Importing a file or received dumps silently overwrites occupied library slots
* **Where:**
  * [librarian.rs:294](crates/virus-app/src/views/librarian.rs) `import_from_file` (default target `ImportTarget::OriginalBanks`, line 304)
  * [librarian.rs:238](crates/virus-app/src/views/librarian.rs) "Import into library banks A–D"
  * [db.rs:45](crates/virus-db/src/db.rs) `IMPORT_PATCH_SQL` (`ON CONFLICT … DO UPDATE`)
* **What:** The import buttons write every dump into its slot with no confirmation and no undo. This applies to the 📂 Import button (checkbox off = `bb` → A–D, on = the selected bank) and to the received-dumps panel. Any slot already in use is replaced. Its tags and favourite flag are also reset when the incoming name differs.
* **Why it matters:** This is the library-data-loss path TODO U4 ("confirm before overwriting a slot") was meant to close, but only the single-slot Store dialog got the confirmation. A user who has stored edited sounds in library bank A, then imports a factory or downloaded bank "to have a look", loses all of them in one click. Nothing in the UI says it happened.
* **Fix:**
  * Parse first, then compute the slots that would be overwritten (bank, program, old name → new name).
  * If any exist, show a confirmation: "Overwrite N slots" / "Import as a new library bank instead" / Cancel.
  * Make "Import as a new library bank" the default for files.
  * Optionally keep the replaced rows (a `patch_history` table or a soft-delete) so the action can be undone.

### H2. Bank send and export re-checksum damaged patches, hiding the damage, with no warning
* **Where:**
  * [app.rs:209](crates/virus-app/src/app.rs) `send_bank` → [db.rs:467](crates/virus-db/src/db.rs) `export_bank_dumps` → [patch_data.rs:80](crates/virus-db/src/patch_data.rs) `encode_single_dump`
  * [librarian.rs:560](crates/virus-app/src/views/librarian.rs) `send_bank_dialog`
  * [library.rs:134](crates/virus-app/src/library.rs) `export_bank_file`
* **What:** Patches imported with `checksum_ok = false` are kept and flagged ⚠, which is correct. But when a bank is sent to the synth or exported to `.syx`, every frame gets a **freshly computed, valid checksum**. The send dialog doesn't mention flagged slots either.
* **Why it matters:** The known source of bad checksums is the LF→CR-damaged factory `.mid` files (20 bad dumps in Bank A, 32 in Bank B; SPEC §6). Those 52 patches have damaged *data*. Sending such a bank writes them into the synth's RAM bank A/B, which can't be undone (manual p. 177). And because the checksum is now valid, the Virus won't show its checksum error, which was the last signal of damage. Exported files hide the damage in the same way.
* **Fix:**
  * In the send dialog, count `checksum_ok = false` slots in the bank. When there are any, require an explicit choice: "skip flagged slots" (default) or "send anyway".
  * On export, either warn and offer to skip flagged slots, or keep the original (wrong) checksum so the damage stays detectable.
  * Show the ⚠ count in the export notice.

---

## Medium

### M1. Data bytes ≥ 0x80 are accepted from SMF files, stored and sent; the editor then rewrites them without user input
* **Where:**
  * [file.rs:89](crates/virus-syx/src/domain/file.rs) `sysex_data` / [file.rs:183–186](crates/virus-syx/src/domain/file.rs) (the `F0` event takes everything up to the first `F7`)
  * [sysex.rs:281](crates/virus-syx/src/domain/sysex.rs) (`parse_sysex` doesn't check data bytes are 7-bit)
  * [sysex.rs:336](crates/virus-syx/src/domain/sysex.rs) and [patch_data.rs:80](crates/virus-db/src/patch_data.rs) (data emitted verbatim)
  * [controls.rs:87, 90, 138](crates/virus-app/src/views/controls.rs) (`DragValue::clamp_range(0..=127)`)
* **What:** I checked each step with a scratch program:
  1. A `.mid` whose SysEx event data contains e.g. `0x85` parses as a valid `SingleDump`.
  2. It imports, and `export_bank_dumps` / `Patch::to_sysex` emit the `0x85` inside the frame. That is invalid SysEx: a status byte that aborts the dump on the synth.
  3. In egui **0.27.2**, `DragValue` clamps and **writes back** the bound value every time it is drawn ([egui drag_value.rs:447–450](https://github.com/emilk/egui/blob/0.27.2/crates/egui/src/widgets/drag_value.rs)). So loading such a patch and opening the Modulation tab (destinations, Definables), or any Raw/Note control, turns the byte into 127. The app treats that as a user edit and sends a parameter change, with no user action at all. This breaks rule §5.1.

  The raw `.syx` stream path and the MIDI input parser already abort frames on status bytes, so only the SMF path is open.
* **Fix:**
  * In `extract_from_smf`, drop or report `F0`/`F7` event data that contains any byte ≥ 0x80 other than the final `F7`.
  * Defence in depth:
    * have `parse_sysex` return an error (or a flag) for 8-bit data;
    * `import_single_dumps` / `save_patch` reject such data;
    * `encode_*` `debug_assert!` 7-bit data.
  * In the app, make controls emit an `Edit` only when `response.changed()` is true, not when `value != raw`. That keeps any widget-side clamping from ever producing an edit.

### M2. Duplicate program numbers in one import silently overwrite each other
* **Where:** [db.rs:374](crates/virus-db/src/db.rs) `import_single_dumps`. The behaviour is acknowledged in the `factory_bank_a_round_trip` test comment ("two dumps with ss = 13 … the later one wins").
* **What:** If a file holds two dumps for the same `bb`/`ss`, both are written and the report says `imported = 2`. The real damaged factory Bank A `.mid` does exactly this: program 10 is stored as 13. The result is that A013 holds program 10's sound, A010 is empty, and the report claims 128 imports.
* **Why it matters:** The wrong sound ends up in a slot, and a later bank send (H2) writes it to the synth.
* **Fix:** Track the `(bank_id, program)` pairs written in the transaction. Count repeats in a new `ImportReport::duplicates` field (with the slot labels), show them in `ImportOutcome::summary`, and include them in `has_warnings()`. Consider keeping the first occurrence, or asking the user.

### M3. An existing v1 library is never found, so the migration never runs for real users
* **Where:** [storage.rs:20](crates/virus-app/src/storage.rs) `default_db_path` (`%APPDATA%\VirusEditor\data\virus_library.db`). The v1 app opened `PatchDatabase::open("virus_library.db")` relative to the working directory (`774dfa7:crates/virus-app/src/app.rs:52`).
* **What:** On first run the new app creates an empty v2 library in the new location. The user's v1 library, in the repo or working folder, is neither migrated nor mentioned. The tested `migrate_v1_to_v2` code is unreachable in practice.
* **Why it matters:** To the user it looks like the whole library is gone. If they then re-import into the new library and later find the old file, the two have diverged.
* **Fix:** On startup, when the new DB doesn't exist and a `virus_library.db` exists in the working directory or next to the executable, offer to copy it to the new location (keeping the original as a backup) and open it, which migrates it. Otherwise show a notice with both paths.

### M4. In Multi mode, CC/PolyPressure are applied to the edited part whatever part they actually reach
* **Where:** [session.rs:118–119](crates/virus-app/src/session.rs) `apply_channel_param`
* **What:** The filter is `channel == settings.channel` only. SPEC §1.4 says CC is "received only by the **lowest-numbered part** on that channel". With `EditPart::Multi(n)`, CCs on the configured channel are written into part *n*'s patch even when part *n* listens on another channel, or isn't the lowest part on it.
* **Why it matters:** The editor's copy drifts from the synth's buffer. Storing it then saves values that belong to another part.
* **Fix:** Until F3/F4 provide the part→channel map, ignore CC/PolyPressure when `settings.part != EditPart::Single`, and rely on SysEx parameter changes, which carry the part. Add a session test for this.

### M5. Editor and synth silently drift apart (Load "not sent", Init, part change, incoming dump)
* **Where:**
  * [app.rs:170](crates/virus-app/src/app.rs) `load` (Load, loose "Load", Init at line 246)
  * [midi_bar.rs:62](crates/virus-app/src/views/midi_bar.rs) (changing Part)
  * [session.rs:135–136](crates/virus-app/src/session.rs) (an incoming edit-buffer dump replaces the patch)
* **What:** After Load/Init, or after switching Part, the editor shows sound X while the synth's addressed buffer still holds sound Y. Every knob move is then sent as a single parameter change into Y. You hear Y plus your edits, but 💾 Store saves X plus your edits. The opposite case also exists: an unsolicited edit-buffer dump from the synth, a fetch, or a MIDI loop echo replaces unsaved editor work with no prompt. There is no dirty or in-sync state at all.
* **Fix:**
  * Track `synced: bool`, cleared by Load/Init/part change and set by Send or by a received edit-buffer dump.
  * While unsynced, either show a prominent "editor ≠ synth — Send?" banner, or make the first edit send the full edit-buffer dump before the parameter change.
  * On Part change, offer Fetch.
  * Track `dirty` and ask before an incoming dump or Load discards unsaved edits.

---

## Low

### L1. Migrated v1 patches look as trustworthy as byte-exact imports
[db.rs:130](crates/virus-db/src/db.rs) sets `checksum_ok = 1` for every v1 row. But v1 stored `Patch::to_bytes()` output from the old lossy model (`774dfa7:crates/virus-db/src/db.rs:144`), which TODO says changes all 128 Bank A patches. v1 import also put dumps into whatever bank the user had selected, regardless of `bb`. The migration is atomic, but it is one-way: the v1 binary can't open a v2 file, and no backup is made.

**Fix:** Copy the file to `virus_library.v1.bak` before migrating, and mark migrated rows. Either add a `source`/`provenance` column ("v1, may be altered") or reuse the ⚠ flag with its own tooltip, and recommend re-importing from original files.

### L2. The bank-send dialog doesn't enforce the backup it recommends; edits are interleaved with the paced send
[librarian.rs:582–586](crates/virus-app/src/views/librarian.rs): "Send N patch(es)" is enabled straight away, even before the requested backup has arrived. While a 128 × 150 ms batch is running, editor edits (`MidiCommand::Send`) go out immediately between dumps, while the synth is writing RAM.

**Fix:**
* Disable Send (or require a tick box) until a backup of that bank has been received or explicitly skipped.
* While a batch is pending, either queue `Send` behind it or block editing and show progress.

### L3. Received dumps from two banks can't be imported "as a new library bank"
[library.rs:120](crates/virus-app/src/library.rs) uses `ImportTarget::Bank`, which fails with `MixedSourceBanks` once A and B (or a repeated request) are both in `Session::received`. That leaves only the overwriting "A–D" button (see H1). `received` is also unbounded and never de-duplicated, so a repeated bank request doubles the list (and runs into M2).

**Fix:** Create one new bank per source `bb`, and de-duplicate by `(bb, ss)`, keeping the latest.

### L4. Unverified: which part byte the synth uses for its own SysEx parameter changes in Single mode
[session.rs](crates/virus-app/src/session.rs) `handle_sysex` accepts only `part == 0x40` in Single mode. If the Rack transmits panel moves in Single mode with another part byte (for example `00`), every incoming edit is ignored and the editor drifts again (M5).

**Fix:** Add this to the G2 live-test list, and log ignored parameter changes with their part byte so the cause is visible.

---

## Test gaps (each would plausibly catch a real bug above)

1. **Controls never produce edits on their own.** Run `param_control` and `destination_control` headless (`egui::Context::run`) on patches with out-of-range or extreme bytes: A49 = 30, A112 = 29, B2 = 40, B65 = 114, Toggle = 2, and a byte > 127. Assert `edits.is_empty()`. Catches M1 and any future widget that clamps.
2. **SMF event with an 8-bit data byte** is rejected or reported (M1).
3. **Duplicate `(bb, ss)` in one import** is reported (M2). The damaged factory Bank A `.mid` is a ready-made fixture.
4. **Import over occupied slots** goes through a "would overwrite" pre-check (H1).
5. **Bank-send frames for flagged patches** are skipped or warned about (H2).
6. **Multi mode ignores CC/PolyPressure** (M4).
7. **Startup finds a legacy `virus_library.db`** (M3). Also a migration test from an on-disk v1 file with a `-wal` that hasn't been checkpointed, not only in-memory.
8. **T4 `multis.mid` end-to-end** (A 128, B 128, 33 loose, 129 multi + 1 other skipped) is still only checked by hand. Commit it as a test next to `crates/virus-syx/tests/factory.rs`.

---

## Verified OK

* **SysEx framing (§3):**
  * Header `F0 00 20 33 01 dd`.
  * Fixed lengths per command: dump 267; `30`/`31`/`37` 10; `32`/`33` 9; `34`–`36` 8; `7x` 11.
  * The manual's example `F0 00 20 33 01 10 70 05 28 5F F7` round-trips.
  * Checksum = (dd + cmd + bb + ss + Σdata) & 0x7F in both `virus-syx` and `virus-db`.
  * Bad checksums are reported, not rejected; unknown commands (`12`) become `Other`.
* **Bank and part bytes:**
  * `SingleBank` A–D ↔ 1–4; `DumpTarget` `00` = edit buffer (`ss` = part `00–0F`/`40`).
  * Export uses the bank letter → `bb` (D4 fixed).
  * Requests: `30 00 pp`, `32 bb`, `37 00 ss`.
  * Page C bank/program change: C31/32/33/105.
  * Edits go to `40` or Multi part `00–0F` (P8).
* **Parameter table vs addendum pp. 13–21, row by row:** every documented A/B number is present, with no extras. Class `p` rows (A1–4, 6–9, 11–16, 32, 64–66, 123) are Performance, and A0 is Internal. `ms`/`np` flags match (A113–119, B20). Bipolar/unipolar/choice kinds and value lists match. Assign layout B64–78 (1 + 2 + 3) and LFO assign B79–82 match. Small differences, all harmless because nothing clamps:
  * A19/A24 range (addendum "0..64", 64 names → 0..63);
  * A101/A112 (addendum range narrower than its own value list);
  * the addendum prints B55 as "Filter1" twice (the code correctly names it Filter2).
* **Byte preservation:**
  * `Patch` is `[u8; 256]`; `set` touches only its own byte and refuses A0 and Page C.
  * Enums keep `Unknown(u8)`.
  * Knobs and combos show out-of-range values and keep them until moved.
  * Clean factory banks A/B round-trip byte-identically.
  * The DB stores and returns blobs byte-exact.
  * Favourite and tags never touch the data (D1).
* **Incoming MIDI (§5.5):**
  * CC/PolyPressure only on the selected channel and only for class `a`/`b`.
  * SysEx parameter changes only for the edited part, never `p`/A0/Page C.
  * Device ID matching with Omni on either side.
  * Bank and other-part dumps are queued, never loaded.
* **virus-midi:**
  * The parser handles running status, split SysEx, real-time bytes inside SysEx, status bytes aborting SysEx (R3), and caps runaway frames at 64 KiB.
  * The worker is the only owner of the connections; channels are unbounded crossbeam.
  * A failed batch send aborts the batch and reports it.
  * The notifier is behind an `RwLock` (no deadlock path found).
  * Dropping `MidiService` stops the thread.
* **DB:**
  * Import is one transaction (rollback tested).
  * Edit-buffer dumps never go into slots (L1).
  * Program > 127 and unknown `bb` are skipped and counted.
  * The `LIKE` search is escaped.
  * A blob that isn't 256 bytes is a `CorruptPatch` error, not a zeroed patch.
  * v1 → v2 migration is atomic and re-derives the name and categories from the bytes. v1 had no category editing, so nothing user-entered is lost.
* **File reader:** bounds-safe on truncated or garbage input (all slicing via `get`/`checked_add`/`min`). The SMF fallback scan resynchronises on `F7`. No panics reachable from file content found.
* **Names:** `sanitize_name` and `Patch::set_name` only write ASCII 32–126/127. Only changed name bytes are sent (D7).

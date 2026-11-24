# virus-cli (`virus`)

A headless command-line tool for working with an **Access Virus Rack (OS 2.00)** over MIDI.
It runs the G2 live tests and open questions Q1–Q3 (SPEC §7) quickly without the GUI. It was
first used with the real synth on 24 Nov (results in TODO G2).

It uses the workspace crates: `virus-syx` for SysEx encoding, parsing, the parameter table and
`Patch`; `virus-midi` (`MidiService`) for the transport; and `virus-db` for the optional library
backup.

```powershell
$env:Path="$env:USERPROFILE\.cargo\bin;$env:Path"; $env:CARGO_HTTP_CHECK_REVOKE="false"
cargo build --release -p virus-cli          # → target\release\virus.exe
.\target\release\virus.exe --help
.\target\release\virus.exe <command> --help
```

## Options shared by every command

| Option | Default | Meaning |
|---|---|---|
| `--in <name part>` / `--out <name part>` | the only Virus-looking port, else the only real port | MIDI ports. Matching is a case-insensitive substring; an exact name wins |
| `--device <1-16\|omni>` | `1` (`0x00`, the default since OS 1.50) | SysEx device ID |
| `--channel <1-16>` | `1` | The synth's channel. `monitor` uses it to interpret CC and Poly Pressure. **The owner's Virus is on 11**, so pass `--channel 11` |
| `--part <single\|1-16>` | `single` (`0x40`) | Which edit buffer to use: the Single buffer or a Multi part |
| `--timeout-ms <ms>` | `3000` | How long to wait for an answer. For bank requests it's the longest allowed gap between two dumps |
| `--settle-ms <ms>` | `60` | Pause after a parameter change or dump before reading it back |
| `--quiet-ms <ms>` | `500` | Silence required on the input before each request. The Virus ignores requests that arrive too soon after it has sent or applied something (SPEC §3.7). Anything that arrives meanwhile, such as the rest of an interrupted bank stream, is dropped |
| `--bank-quiet-ms <ms>` | `2000` | The same before a bank request, which needs longer |
| `-v`, `--verbose` | off | Log every sent (`TX`) and received (`RX`) message as hex to stderr, with a millisecond timestamp. Also reports how many stale messages were dropped while waiting for a quiet line |

Exit codes: `0` = OK; `1` = the command ran but a check failed, or it needs `--yes`; `2` = error.

## Commands

| Command | What it does |
|---|---|
| `ports` | Lists MIDI inputs and outputs, marks the ones that look like a Virus (`*`), and shows which port would be used |
| `monitor [--seconds N]` | Prints incoming MIDI, decoded. SysEx goes through `parse_sysex`: dumps show name, categories and checksum; parameter changes show name and formatted value. CC and Poly Pressure show the Page A/B parameter they'd set on `--channel`, and flag performance controllers and other channels |
| `fetch [--out-file x.syx] [--brief]` | Requests the edit buffer (`30 00 pp`) and prints the name, categories, sound version, every sound parameter with its formatted value, and the non-zero undocumented bytes. Saves the dump exactly as received |
| `send <file.syx\|.mid> [--program N] [--from-bank X] [--verify]` | Sends one patch to the edit buffer (`10 00 pp`). For bank files, pick the patch with `--program`. Refuses dumps with a bad checksum unless you pass `--allow-bad-checksum` |
| `get-bank <A-D> --out-file bank.syx [--to-library] [--db file]` | Sends a bank request (`32 bb`) and collects the 128 dumps, showing progress and checking checksums. Writes them as received, in program order. `--to-library` also saves them as a new bank in the editor's library. Banks E–H are allowed for Q2 (undocumented addressing) |
| `put-bank <A\|B> <file> --yes [--gap-ms 300] [--verify]` | Sends a bank to RAM bank A or B, paced with `SendBatch`. Refuses banks C/D. Without `--yes` it prints the warning and the exact backup command, then stops. With `--yes` it **backs up the target bank first** (`backup_bank_X_<stamp>.syx`) unless you pass `--no-backup`. `--verify` requests the bank back and compares every program |
| `set <A\|B\|C><num> <value> [--verify]` | Sends one parameter change to `--part`, e.g. `set A40 64`, `set A10 -5` (bipolar) or `set C32 1`. Page C (`72`) covers part, multi and global settings; it isn't in the single dump, so `--verify` only works for A/B. A0 is always refused. These need `--force`: performance controllers, undocumented bytes, and globals that can break the session (C93 Device ID, C94/C95 Control Pages, C98/C99 Dump Tx/Rx, C118 Memory Protect, C120 Soft Thru, C122 Play Mode, C123 Part Number, C124 Global Channel) |
| `send-hex "F0 … F7" [--yes] [--listen-ms 1000]` | For experiments: sends one raw SysEx message, checking that it is framed `F0 … F7` with 7-bit data. It always prints the message and what it decodes to. Nothing is sent without `--yes`, because raw messages bypass every safety check. It waits for a quiet line, sends, then prints what arrives within `--listen-ms` |
| `probe-destinations [--slot assign1\|lfo1\|definable1]` | Q1 resolver: steps a destination through raw values `--from 0 --to 127` (details below). Writes `q1_destinations.json` |
| `probe-values <A49\|A112\|A102\|B39>` | Q3 helper: the same flow for value lists. Writes `q3_<param>.json`. The default `--to` is 40, 40, 15 and 20 respectively |
| `probe-table <file.json> [--name CONST]` | Turns probe results into a Rust `&[&str]` table, indexed by raw value |
| `selftest [--bank A] [--skip-bank] [--skip-global]` | Runs the G2 checks that can be automated and prints a pass/fail report (details below) |

### How the probes work

1. The probe fetches the edit buffer and saves it to `probe_editbuffer_backup.syx` before it
   changes anything. If the fetch fails, nothing is sent.
2. It sends the setup parameter changes. For `assign1` that's Assign 1 source = ModWheel (`B64=3`)
   and amount = +63 (`B66=127`); for `lfo1` it's LFO1 assign amount = +63 (`B80=127`).
3. For each value it sends the parameter change and requests the edit buffer back. It records the
   value read back (a different value suggests the synth clamped or rejected it) and any *other*
   bytes that changed. Then it asks what the Virus display shows:
   * type the text;
   * `Enter` = skip;
   * `r` = resend the value (if the display didn't refresh);
   * `q` = stop.

   `--non-interactive` only records the read-back values. Each value also shows the
   **unverified** printed name for comparison.
4. Results are written to the JSON file after every value, so you can stop at any time and
   continue later with `--resume`. By default the probe stops after 3 values in a row that read
   back differently (`--stop-after-rejected`).
5. At the end, even after `q` or an error, the probe sends the original edit buffer back and
   checks that it reads back byte-identically. If the restore fails, the probe prints the exact
   `virus send probe_editbuffer_backup.syx` command.

### What selftest checks

| Check | Pass when |
|---|---|
| Fetch the edit buffer | A dump arrives with a correct checksum |
| Parameter round trip on Page A (`A40` Cutoff, SysEx `70`) and Page B (`B36` Punch Intensity, `71`) | After a ±1 change, only that byte differs. After setting it back, the whole buffer matches the original |
| Send the original edit buffer (`10 00 pp`) | It reads back byte-identically |
| Bank request (`32 01`) | 128 dumps arrive, all with correct checksums |
| Global request (`35`) | Informational only: prints the raw answer, for F3 |

The report ends with the G2 items that still need ears, eyes or the GUI.

### Testing C32 (F7: is Part Bank Change 0-based or 1-based?)

The editor's "Select on synth" sends C32 Part Bank Change as **A = 0, B = 1, …** (unverified).
The dump byte `bb` uses A = 1. To find out which is right:

1. Put the Virus in Single mode. Store anything you want to keep: selecting a program replaces the
   edit buffer.
2. Pick a program number whose sounds differ between RAM banks A and B, e.g. 5. Note both names
   (from the Virus display, or from the "Synth A/B backup" banks in the editor's library).
3. `virus set C32 1`, then `virus set C33 5`, then `virus fetch --brief`.
   * B005's name means **0-based** (1 = B), which is what the editor sends.
   * A005's name means **1-based** (1 = A).
4. Cross-check with `virus set C32 0`, `virus set C33 5`, `virus fetch --brief`. With 0-based this
   gives A005; with 1-based, 0 is out of range and the sound probably doesn't change.
5. If nothing changes at all, the Single part (`0x40`) may not accept C32/C33. Try the same in
   Multi mode with `--part 1`. Raw equivalent for experiments, with device 1 and part `0x40`:
   `virus send-hex "F0 00 20 33 01 00 72 40 20 01 F7" --yes`.

Record the result in TODO F7/G2. If it's 1-based, change `virus-app/src/synth_select.rs`
(`bank.to_raw() - 1` → `bank.to_raw()`) and its tests.

## First cable session runbook

**Status (24 Nov):** steps 1–6 are done (results in TODO G2). Steps 7–9 were deferred until audio
was wired; steps 10–14 and 12a are still to do. Stop the sequencer transport (MIDI clock) before
dump requests and bank sends.

**Before you start**
* Connect **both** cables: the Virus MIDI OUT to the interface IN, and the interface OUT to the
  Virus MIDI IN. Every request needs the answer path.
* Close the editor GUI, DAWs and anything else that might hold the ports. On Windows, a MIDI
  port can only be open in one program at a time.
* On the Virus, find the global MIDI settings and write down:
  * **Device ID** (C93; it should be 1);
  * **global channel** (C124);
  * **MIDI Control Low/High Page** (C94/C95);
  * **Memory Protect** (C118);
  * **Soft Thru** (C120). Turn it off so our own messages don't echo back.
* Put the Virus in **Single mode**. Use `--part` only when testing Multi.
* In the commands below, `virus` stands for `.\target\release\virus.exe`. Add `--in`/`--out`
  if `ports` can't pick the interface by itself. Add `-v` whenever something looks wrong.

**Steps, in this order**

| # | Command | What to look for |
|---|---|---|
| 1 | `virus ports` | Your interface's input and output appear. Note a short, unique part of the name (e.g. `UM-ONE`) for `--in`/`--out` |
| 2 | `virus monitor --seconds 30` | Turn a few knobs and change a program on the Virus. Lines should appear: CC on ch 1 → `A40 Cutoff = …`, or SysEx `Param …` lines. Nothing = the input path or channel is wrong. "other channel" = pass `--channel` |
| 3 | `virus fetch --brief` | The current sound's name appears and matches the Virus display. A timeout means the output path or device ID is wrong: try `--device omni`, check C93, check that the Virus isn't in Multi mode |
| 4 | **Back up both RAM banks before anything else:**<br>`virus get-bank A --out-file backup_A.syx --to-library`<br>`virus get-bank B --out-file backup_B.syx --to-library` | Each shows `bank A: 128/128` and "Saved 128 dump(s)" with no checksum warnings. Copy both files somewhere safe, such as OneDrive. The banks also appear in the editor's library as "Synth A/B backup …". If a bank stalls, increase `--timeout-ms` |
| 5 | `virus get-bank C --out-file rom_C.syx` and `… D …` (optional) | ROM banks C/D. Useful to compare with the patch list |
| 6 | `virus selftest` | All PASS. The Page A/B round trips prove that SysEx `70`/`71` work and that no other byte changes |
| 7 | `virus set A40 20`, then `virus set A40 110 --verify` | You hear the cutoff close and open. The Virus display (if it shows Cutoff) follows |
| 8 | `virus fetch --out-file current.syx` | The full parameter list. Compare a few values with the Virus display |
| 9 | `virus send testdata\factory\Bank_A.syx --program 1 --verify` | "Smoog   OK" plays from the edit buffer (not stored). Verify should report byte-identical, or only `A0` if the synth updated the sound version |
| 10 | **Q1:** `virus probe-destinations` | Show Assign 1 destination on the Virus display (Mod Matrix / Assign 1). Type the name it shows for each value. Then `--slot lfo1 --out-file q1_lfo1.json` and `--slot definable1 --out-file q1_definable1.json` |
| 11 | **Q3:** `virus probe-values A49`, `A112`, `A102`, `B39` | Show that parameter on the display and type each value's name. Watch for values that read back differently: that's where the range ends |
| 12 | **Q2:** `virus get-bank E --out-file probe_E.syx --timeout-ms 2000` | Do dumps arrive for `bb=05`? With which bank byte? Use `-v` to see the raw answers. Repeat for F–H |
| 12a | **F7 C32:** `virus set C32 1`, `virus set C33 5`, `virus fetch --brief` (and the cross-check with `C32 0`) | Which bank's program 5 loaded? See "Testing C32" above. Needs the bank backups from step 4, because selecting a program replaces the edit buffer |
| 13 | `virus monitor` with the Virus set to Multi mode, then `virus fetch --part 1 --brief` | Multi part addressing |
| 14 | **Bank send last** (G2): `virus put-bank A backup_A.syx --yes --verify` | Stop the clock/transport on your sequencer first. This sends your own backup back, so the content doesn't change. It takes a fresh automatic backup first. Watch the Virus display for a checksum or buffer error during the 128 dumps (with 300 ms gaps it takes about 50 s). Verify should say "all 128 program(s) read back byte-identical". If you see errors, retry with `--gap-ms 500`. If Memory Protect is on, the Virus may ignore the bank |

Record the results (pass/fail and the gap that worked) in TODO G2. Keep the probe JSON files: run
`virus probe-table q1_destinations.json --name ASSIGN_DESTINATION_NAMES` to turn them into the
verified name tables.

## Design

The modules that hold the logic are pure and unit-tested without hardware:

| Module | What it does |
|---|---|
| `settings`, `address` | Parse device IDs, parts, channels, bank letters, `A40`/`B39`/`C32` and values; guard risky parameters (`--force`) |
| `messages` | Build every outgoing message; parse and check `send-hex` input |
| `transport` | The `Transport` trait, plus collectors fed with `MidiEvent`s: edit buffer, 128-dump bank, first Virus answer. Also `exchange()`, which handles idle timeouts and diagnostics |
| `ops` | Fetch, send, set, get/put bank, verify |
| `probe` | Plans, records/JSON, the runner with restore, and the prompter |
| `selftest` | Checks and the report |
| `decode` | Monitor lines |
| `summary` | The patch summary |
| `files` | Pick a patch or bank from `.syx`/`.mid` files |

The tests run against `fake::FakeVirus`, an in-memory synth that implements `Transport`. It
answers requests, applies parameter changes, stores dumps, and can clamp values or stay silent.

Only `midi_link` (`MidiService` and port selection) and `commands` (printing and files) touch
real I/O.

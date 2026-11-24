# Access Virus Rack Editor & Librarian — Specification

## 1. Project Overview

### 1.1 Goal
A native desktop **sound editor and patch librarian** for the **Access Virus Rack (classic), OS 2.00**, written in Rust. It must:

1. **Edit** the synth's current sound live over MIDI, with every change heard immediately.
2. **Fetch and send** single sounds and multis between the synth and the computer.
3. **Keep a library** of sounds: import and export bank files, search, categorise, and store back to the synth.
4. **Never corrupt a sound.** A patch that goes in and comes back out, untouched, must be byte-identical (§5.1).

**Scope (decided 24 Nov): Virus Rack (classic) only.** Other models (A, B, kb, Indigo) share the protocol and most of the parameter map, but they're out of scope for now.

### 1.2 Reference documents (in this repo)

| Document | Use it for |
|---|---|
| `OSR1_Manual_International.pdf` | Access Virus Rack manual (227 pp.). Ch. 16 *The Parameters* (pp. 94–160), Ch. 18 *The Virus and Sequencers* (pp. 168–177), Ch. 20 *Appendix: System Exclusive Data* (pp. 204–209). **It does not contain the parameter tables** it refers to |
| `VirusOSR200_INT/OSR200_INT/Addendum/Virus rack Addendum INT.pdf` | **Authoritative for OS 2.0:** classes, Single Dump table (Pages A, B, C), Multi Dump table, Phaser and FX ring modulator (OS 2.0) |
| `VirusOSR200_INT/OSR200_INT/Goodies/OSR15_Addendum_INT.pdf` | Same tables for OS 1.5, plus the **Mod Matrix Sources**, **Mod Matrix Destinations** and **Definable Knobs Destinations** lists |
| `VirusOSR200_INT/OSR200_INT/Goodies/OSR15 Patch list.pdf` | Factory sound names, banks **A–H** × 128 (1,024 sounds) |
| `VirusOSR200_INT/OSR200_INT/Goodies/Factory soundset/*.mid` | **Ground truth** for testing (§6) |
| `VirusOSR200_INT/OSR200_INT/OSR200_readme.htm` | OS version history |

Page numbers below are the manual's printed page numbers (PDF page − 2).

### 1.3 Target hardware (manual pp. 10, 54–58, 66; addendum p. 9; readme)
* 16 voices, 16-part multitimbral. Modes: **Single**, **Multi** and **Multi-Single** (a view of Multi mode where you edit one part's single).
* **Singles:** Banks **A and B are RAM** (writable, 2 × 128). Banks **C and D are Flash ROM** (factory, read-only via STORE). OS 1.5+ adds 512 more ROM sounds (patch list banks **E–H**, 1,024 in total). How banks E–H are addressed over SysEx is **not documented** (open question Q2).
* **Multis:** 128 programs in one Multi bank.
* **Edit buffers:** one Single edit buffer (Single mode, part `0x40`), 16 Single edit buffers (one per Multi part, `0x00–0x0F`) and one Multi edit buffer. Received single dumps go to the edit buffer; the user must STORE them (p. 150).
* **Device ID default:** `1` (`0x00`) since OS 1.50 (readme). `0x10` = Omni.
* OS versions: 1.50 (1,024 sounds; default device ID 1), 1.51, **2.00** (Phaser, FX ring modulator).

### 1.4 MIDI transport (manual pp. 168–170, 204–206; addendum p. 11)
* **SysEx:** full access to Pages A, B, C, all dumps and requests. Parts are addressed by **part number**, regardless of MIDI channel.
* **Control Change → Page A** (`Bc nn vv`). Only when global **C94 MIDI Control Low Page = Contr** (default). Received only by the **lowest-numbered part** on that channel. **Performance controllers** (class `p`, e.g. A1 mod wheel, A7 volume, A64 hold) share CC numbers with Page A but are not sound parameters and are **not stored** in the patch.
* **Poly Pressure → Page B** (`Ac nn vv`). Only when **C95 MIDI Control High Page = PolyPrs**. **The default is SysEx**, so the editor must not rely on Poly Pressure.
* **Page C** (Multi and Global) is **SysEx only**.

### 1.5 Core architecture
* Multi-crate Cargo workspace. Dependencies point one way: `virus-app` and `virus-cli` → `virus-syx`, `virus-midi`, `virus-db`. The three library crates don't depend on each other.
* **`virus-syx`:** pure domain crate. A raw 256-byte `Patch` is the single source of truth (§5.1), described by one parameter table (§4).
* **`virus-midi`:** a plain transport on its own thread (`midir` + channels). It doesn't interpret Virus messages.
* **`virus-db`:** a byte-exact SQLite librarian (`rusqlite`, bundled). The library lives at `%APPDATA%\VirusEditor\data\virus_library.db`.
* **`virus-app`:** a native immediate-mode GUI (`egui`/`eframe` 0.27). It renders with OpenGL 2.0+ (`glow`) and, if that fails at start-up, falls back to `wgpu` (Direct3D 12, which includes the WARP software rasteriser for machines without a GPU). `--renderer glow|wgpu` or `VIRUS_RENDERER` forces one. Logic that can be tested without the GUI lives in `session.rs`, `library.rs` and `storage.rs`.
* **`virus-cli`:** the headless `virus` binary (`clap`) for live sessions: fetch, send, set (Pages A, B and C), raw `send-hex`, get-bank and put-bank, monitor, Q1/Q3 probes and a self-test. Message building, the answer collectors (fed with `MidiEvent`s) and probe planning are pure, and are tested against an in-memory fake Virus. Its README holds the live-session runbook. Used with the real synth on 24 Nov (TODO G2).

---

## 2. Architecture & Crate Layout

```text
VirusEditor\
├── Cargo.toml / Cargo.lock     # Workspace definition (lockfile committed)
├── .gitattributes              # .syx/.mid/.lib/.bin/.pdf are binary (never line-ending converted)
├── SPEC.md · TODO.md · README.md · AGENTS.md (rules for AI agents) · REVIEW.md (24 Nov code review)
├── OSR1_Manual_International.pdf # Virus Rack manual
├── VirusOSR200_INT\            # OS 2.00 update: addenda (parameter tables), patch list, factory soundset (damaged .mid, §6)
├── testdata\factory\           # Recovered clean factory Banks A/B (.syx) + recover.py + provenance README
└── crates\
    ├── virus-syx\              # Domain: SysEx, raw Patch, parameter table (no I/O)
    │   ├── src\domain\
    │   │   ├── checksum.rs     # 7-bit checksum
    │   │   ├── types.rs        # Category, AssignSource (with Unknown(u8))
    │   │   ├── params.rs       # SINGLE_PARAMS table (§4.1–4.2), classes, value kinds, format_value
    │   │   ├── patch.rs        # Patch([u8;256]) get/set, name, categories, DumpTarget, SingleBank
    │   │   ├── multi.rs        # Multi([u8;256]), MULTI_PARAMS (§4.4) with live Page C mapping, MultiDumpTarget, PartFlag
    │   │   ├── sysex.rs        # parse_sysex → SysExMessage (checksum_ok), all encoders (§3), C32 Part Bank mapping (§3.5)
    │   │   └── file.rs         # .syx and SMF (.mid) SysEx extraction, tolerant of the damaged files
    │   └── tests\              # factory.rs (damaged originals), clean_factory.rs (strict round trips), multis.rs (all multis.mid multis)
    ├── virus-midi\             # Transport: ports by name, input parser, batches, MidiService thread
    │   └── src\ engine.rs · events.rs · parser.rs · batch.rs · ports.rs · notes.rs (note audition, §5.9)
    ├── virus-db\               # Librarian: schema v3 + migrations, byte-exact patches and multis, import/export
    │   └── src\ db.rs (+ db\tests.rs, db\multis.rs) · models.rs · patch_data.rs · multi_data.rs · schema.rs · error.rs
    ├── virus-app\              # GUI
    │   └── src\
    │       ├── main.rs · app.rs # Window, tabs, wiring
    │       ├── session.rs      # MIDI settings, incoming interpretation (§5.5), outgoing edits
    │       ├── library.rs      # File import/export glue, store rules, cached queries
    │       ├── storage.rs      # DB location and in-memory fallback
    │       ├── settings.rs     # Persisted settings (settings.json), migration, port restore, window geometry
    │       ├── dialogs.rs      # Off-thread file dialogs and file I/O (one task at a time); folders shared by the Librarian and Multi tabs
    │       ├── synth_select.rs # SysEx bank/program change messages for "Select on synth" (F7)
    │       ├── multi_session.rs # Multi edit buffer, incoming multi dumps/Page C, live edits, multi librarian glue
    │       ├── audition.rs     # Note audition state (§5.9)
    │       ├── theme.rs        # Rack and TI palettes, header logo (§5.10)
    │       ├── comforts.rs · history.rs · compare.rs · sections.rs # Undo/redo, Compare, diff, section copy/paste (E1–E5)
    │       ├── renderer.rs     # Renderer choice: OpenGL first, wgpu/WARP fallback (G1)
    │       ├── views\          # editor.rs (generated from SINGLE_PARAMS), modulation.rs, librarian.rs (+ librarian\files.rs: file import), multi.rs (Multi tab), comforts.rs, midi_bar.rs, controls.rs
    │       └── widgets\        # knob.rs, curves.rs (filter response, envelopes), piano.rs
    └── virus-cli\              # `virus` CLI + README runbook
        └── src\
            ├── main.rs · cli.rs · commands.rs   # clap definition, I/O shell
            ├── settings.rs · address.rs         # device/part/channel/bank parsing; A/B/C parameter addresses and edit guards
            ├── transport.rs    # Transport trait, collectors (edit buffer, bank, first answer), exchange(), wait_quiet
            ├── ops.rs · messages.rs · files.rs  # fetch/send/set/banks, message building, file picking
            ├── probe.rs · selftest.rs           # Q1/Q3 probes (JSON, restore), G2 checks
            ├── decode.rs · summary.rs           # monitor lines, patch summary
            ├── midi_link.rs    # MidiService transport, port selection, --verbose hex log
            └── fake.rs         # in-memory Virus for tests
```

---

## 3. SysEx Protocol (manual pp. 204–209)

All bytes are hex. Every message is `F0 00 20 33 01 dd <message> F7`:
* `00 20 33`: Access Music manufacturer ID. `01`: product ID (Virus).
* `dd`: device ID, `00–0F` = device 1–16, `10` = Omni. All factory `.mid` dumps use `10`.

### 3.1 Parameter change `7x` (p. 206)
`F0 00 20 33 01 dd 7x pp nn vv F7`
* `x`: `70` = Page A, `71` = Page B, `72` = Page C.
* `pp`: part number. `00–0F` = Multi part 1–16; `40` = the Single buffer in Single mode. It is ignored for non-part-sensitive parameters (class `np`, `g`) but must still be sent.
* `nn`: parameter number 0–127; `vv`: value 0–127.
* Manual example: `F0 00 20 33 01 10 70 05 28 5F F7` sets **Cutoff (A40)** of part 6 to 95.

### 3.2 Single dump `10` (p. 207)
`F0 00 20 33 01 dd 10 bb ss [256 bytes] cs F7`, 267 bytes in total.
* `bb`: `00` = edit buffer, `01–04` = Single Bank **A–D**. Factory Bank A dumps use `01`; Bank B dumps use `02`.
* `ss`: program 0–127. When `bb = 00`, `ss` is the **part**: `00–0F` (Multi) or `40` (Single).
* Data: Page A bytes 0–127, then Page B bytes 128–255. **The data offset equals the parameter number** (byte `n` = A`n`; byte `128+n` = B`n`).
* `cs` = (dd + 10 + bb + ss + Σdata) AND 7F. The manual calls the checksum "optional": the Virus **accepts a wrong checksum** but shows an error. The editor must always send a correct checksum for sounds it builds or edits. On import it should report a mismatch rather than silently drop the dump. A dump that arrived with a wrong checksum is never silently "fixed": when the user explicitly sends it anyway (the app's "Send them anyway" / "⚠ as stored", the CLI's `--allow-bad-checksum`), it goes out **as stored, with its checksum still wrong**, so the Virus can still detect the damage.

### 3.3 Multi dump `11` (p. 207; addendum p. 25)
`F0 00 20 33 01 dd 11 bb mm [256 bytes] cs F7`. `bb`: `00` = Multi edit buffer (`mm` ignored), `01` = Multi bank. The checksum is the same formula with `11`. Layout in §4.4.

### 3.4 Requests (pp. 208–209)
| Cmd | Message | Notes |
|---|---|---|
| `30` | Single request `30 bb ss` | `bb`/`ss` as in §3.2 |
| `31` | Multi request `31 bb mm` | |
| `32` | Single bank request `32 bb` | `bb` = `01–04`; the synth answers with 128 single dumps |
| `33` | Multi bank request `33 01` | |
| `34` | Arrangement request | Current multi plus the singles of enabled parts |
| `35` | Global request | |
| `36` | Total request | Banks A+B, multis, edit buffers, globals |
| `37` | Controller dump request `37 00 ss` | The synth sends every parameter of part `ss` as CC/PolyPressure/SysEx parameter changes |

### 3.5 Bank / program change via SysEx (p. 197)
Page C parameter changes, independent of MIDI channel and of the program-change enable settings:
* `72 pp 1F vv`: C31 Part Bank Select (takes effect on the next program change)
* `72 pp 20 vv`: C32 Part Bank Change (immediate)
* `72 pp 21 vv`: C33 Part Program Change
* `72 00 69 vv`: C105 Multi Program Change

**Implementation (F7):** the optional "Select on synth" setting sends C32 then C33 to the edited part (Single `40` or the Multi part) when a library patch from bank A–D is loaded (`synth_select.rs`). The C32 value is sent **0-based** (A=0 … D=3), unlike the dump byte `bb` (A=1 … D=4). **Unverified on hardware** (TODO F7, G2). The mapping lives in one place, `virus-syx` `sysex.rs`: `PART_BANK_C32_BASE` (0) with `part_bank_to_c32` / `part_bank_from_c32`. Both call sites use it: Select on synth, and the Multi tab's Part Bank row (§4.4; edits sent as C32, incoming C32 changes stored back as the 0-based dump byte). If the live test shows C32 is 1-based, only the constant changes. The CLI can test it: `virus set C32 <v>` and `set C33 <p>` send Page C to the part, and `send-hex` sends any raw message (see the CLI README, "Testing C32"). It only makes sense when the library bank mirrors the synth's bank, so the label says so. User banks send nothing.

### 3.6 Observed but undocumented
* **Global dump = command `12`.** Confirmed live (24 Nov): Global request `35` is answered with `F0 00 20 33 01 dd 12 01 …` (267 bytes). The same `12` message is the one in `multis.mid`. Its layout isn't documented; keep it raw.

### 3.7 Live timing (Virus Rack OS 2.00, 24 Nov, via an iConnectivity mioXC)
* **The Virus answers one request at a time and ignores requests while busy:**
  * During a bank stream it ignores everything; cancelling the client doesn't stop the stream.
  * Shortly after it has sent an answer or applied a change, it also ignores requests. Measured: a bank request right after an edit-buffer answer got no reply; 2 s later it did. An edit-buffer request 60 ms after a param change was lost; after 500 ms it worked.
* **CLI diagnostics:** with `-v` it reports how many stale messages it dropped; a bank answer that starts mid-stream (not at program 0) is reported as the rest of an interrupted stream.
* **Rule for clients:** wait for the MIDI input to be silent before every request, about 500 ms for normal requests and 2 s for bank requests. Drain and report anything that arrives meanwhile. The CLI does this (`--quiet-ms`, `--bank-quiet-ms`); if the input never goes quiet it gives up after max(10 × the quiet time, 20 s) with "the MIDI input never went quiet" and sends nothing. The app does the same: `PacedSend` (`session.rs`) holds requests (`30`–`37`) until nothing has been sent or received for 500 ms (2 s before `32/33/34/36`); later live messages queue behind them so the order is kept.
* **Bank transfers are reliable:** two bank A requests 27 minutes apart were byte-identical.
* **Measured latencies** (selftest log, `-v` timestamps):
  * edit-buffer request → answer **≈123 ms** (267 bytes take ≈85 ms on the wire, so the Virus reacts in ≈40 ms);
  * Global request → answer 113 ms;
  * bank request → first dump 122 ms. The whole bank of 128 dumps takes **43 s, one every ≈339 ms**, so the Virus pauses ≈250 ms between its own dumps.
* **Sending banks:** pace our dumps at least as slowly as the Virus sends its own, i.e. ≥ 300 ms of silence after each 267-byte dump. 150 ms is untested and probably too fast. Verify with a read-back.
  * **How the pacing works** (`virus-midi` `BatchQueue`, since CR-M1): the batch `gap` is silence *after* each message has left the wire. The next dump is due at the time `send` returned + the message's wire time (len × 320 µs, ≈85 ms for a 267-byte dump) + `gap`, whether or not the OS send blocks until the bytes are out. The default is `virus_midi::DUMP_GAP_MS` = 300 ms for the app's bank and Multi bank sends and the CLI's `put-bank --gap-ms`: ≈385 ms start-to-start, a little slower than the Virus's own ≈339 ms, so a full bank takes ≈49 s.
  * When a batch ends (all sent, cancelled, or a send failed), the worker reports `MidiEvent::BatchFinished { sent, dropped }`. The app then releases live messages held behind the batch (`PacedSend::finish`) and clears its "sending" state; ⏹ Cancel sending is enabled only while a batch runs. Panic doesn't stop a running bank send.
* **Port polling:** the app asks the worker to re-list the MIDI ports every 2 s (`MidiCommand::SetPortPolling`; the CLI never polls). A send that fails closes the output, and the app reopens any wanted port that is listed but not connected, even under the same name (USB blips, selective suspend). A batch aborted by a failed send isn't resumed.
* **Owner's studio (24 Nov plan):**
  * The Virus MIDI IN comes through a Kenton MERGE-4: the editor's output (mioXC OUT) is merged with the Maschine+ (notes and MIDI clock, via THRU-5). Virus OUT goes straight to mioXC IN, so replies arrive unmerged.
  * Our SysEx can therefore have Maschine+ notes and real-time bytes (`F8` clock etc.) between messages, or real-time bytes inside them. That's legal MIDI. Our own parsers ignore `F8`–`FF` anywhere (`virus-midi` parser, `virus-syx` file reader), and these bytes don't count as line activity.
  * The editor can't see clock arriving at the Virus, so **stop the transport before bank sends and dump requests**. That stays the user's job; the bank-send confirmations remind them. The Maschine+ may keep sending `F8` with its transport stopped (unchecked). Interleaved clock is legal, but it needs a live test with `--verify` (TODO G2). If it causes errors, switch the Maschine+ clock output off for transfers.
  * Virus globals: C120 Soft Thru Off; C121 Panel Destination Int+MIDI (knob moves reach the editor); C94 Low Page Contr; **C95 High Page stays SysEx** (the default). With PolyPrs, poly aftertouch from the Maschine+ on the Virus channel would change Page B parameters. The editor never relies on Poly Pressure. With Low Page = Contr, CCs on channel 11 do change Page A parameters (e.g. CC40 = cutoff).
    * **Port renames:** Windows may rename the mioXC ports when its USB socket changes (e.g. "2- mioXC MIDI OUT"). The app reconnects to an available port with the same base name (leading "<digits>- " removed, case-insensitive) and keeps the remembered name, so it reconnects again when the original name returns. With port polling (above) this happens without pressing ⟳.
    * **Stale edit buffer:** with Soft Thru off, the editor never sees what the Maschine+ and KeyStep do to the Virus, so the synth's edit buffer may differ from the editor's copy. Use "Fetch from synth first" in the Store dialog (§5 rule 12) to store what the synth really has.

---

## 4. Parameter Map (Rack addendum OS 2.0, pp. 13–27)

**Classes:**
* `p`: performance controller, not stored.
* `a`: Page A sound parameter (CC + SysEx + dump).
* `b`: Page B sound parameter (PolyPressure + SysEx + dump).
* `m`: Multi parameter.
* `ms`: stored with the Multi when in Multi mode.
* `np`: not part-sensitive.
* `g`: global.

"±" means the value is stored as 0–127 and shown as −64…+63 (64 = 0).

### 4.1 Page A (dump bytes 0–127)
| No. | Parameter | Values |
|---|---|---|
| 0 | *internal:* **Sound Version Number** (`6` in all factory dumps from the OS 1.5 era; **`7` on the live OS 2.00 synth**, which upgrades older sounds as it loads them) | never change (§5.2) |
| 1–4, 6–9, 11–16 | performance controllers (mod wheel, breath, CC3, foot, data, ch. volume, balance, CC9, expression, CC12–16) | not stored |
| 5 | Portamento Time | 0–127 |
| 10 | Panorama | ± |
| 17 / 22 | Osc1 / Osc2 Shape | ± (Wave … Saw(64) … Pulse) |
| 18 / 23 | Osc1 / Osc2 Pulsewidth | 0–127 |
| 19 / 24 | Osc1 / Osc2 Wave Select | 0–63 (0 Sine, 1 Triangle, 2–63 Waves 3–64) |
| 20 / 25 | Osc1 / Osc2 Semitone | ± |
| 21 / 31 | Osc1 / Osc2 Keyfollow | ± (default +32) |
| 26, 27 | Osc2 Detune, Osc2 FM Amount | 0–127 |
| 28 | Osc2 Sync | 0 Off, 1 On |
| 29, 30 | Osc2 Filt Env Amt, FM Filt Env Amt | ± |
| 32 | Bank Select (performance) | 0–3 |
| 33 | Osc Balance | ± |
| 34, 35 | Sub Osc Volume, Sub Osc Shape | 0–127; 0 Square, 1 Triangle |
| 36 | Osc Main Volume | 0–127 (above 64 = saturation gain) |
| 37, 38, 39 | Noise Volume, Ring Mod Volume, Noise Color | 0–127, 0–127, ± |
| 40, 41 | Cutoff, Cutoff2 | 0–127; ± (offset when Cutoff Link is on) |
| 42, 43 | Filter1 / Filter2 Resonance | 0–127 |
| 44, 45 | Filter1 / Filter2 Env Amt | 0–127 (unipolar; polarity in B30/B31) |
| 46, 47 | Filter1 / Filter2 Keyfollow | ± |
| 48 | Filter Balance | ± |
| 49 | Saturation Curve | 0 Off, 1 Light, 2 Soft, 3 Middle, 4 Hard, 5 Digital, 6 Shaper… (**factory data has values up to 30**) |
| 51, 52 | Filter1 / Filter2 Mode | 0 LP, 1 HP, 2 BP, 3 BS |
| 53 | Filter Routing | 0 Ser4, 1 Ser6, 2 Par4, 3 Split |
| 54–58 | Filter Env Attack, Decay, Sustain, Sustain Time (±, Fall…Inf…Rise), Release | |
| 59–63 | Amp Env Attack, Decay, Sustain, Sustain Time (±), Release | |
| 64–66 | Hold, Portamento, Sostenuto pedals | performance |
| 67–73 | LFO1 Rate, Shape (0 Sine, 1 Tri, 2 Saw, 3 Square, 4 S&H, 5 S&G), Env Mode (0/1), Mode (0 Poly, 1 Mono), Symmetry (±, shown as "Contour"), Keyfollow, Keytrigger (0 Off, 1–127 phase) | |
| 74–78 | LFO1 → Osc1, Osc2, PW, Reso, FiltGain amounts | ± |
| 79–85 | LFO2 Rate, Shape, Env Mode, Mode, Symmetry, Keyfollow, Keytrigger | as LFO1 |
| 86–90 | LFO2 → OscShape, FM Amount, Cutoff1, Cutoff2, Panorama amounts | ± |
| 91 | Patch Volume | 0–127 (unity 100) |
| 93 | Transpose | ± |
| 94 | Key Mode | 0 Poly, 1–4 Mono 1–4 (manual also lists Hold) |
| 97–100 | Unison Mode (0 Off, 1 Twin, 2–15 voices), Detune, Pan Spread, LFO Phase (±) | |
| 101, 102 | Input Mode (0 Off, 1 Dynamic, 2 Static, 3 ToEffects), Input Select (0 In L, 1 In L+R, 2 In R, …) | |
| 105–110 | Chorus Mix, Rate, Depth, Delay, Feedback (±), LFO Shape | |
| 112 | Delay/Reverb Mode | 0 Off, 1 Delay, 2 Reverb, 3 Rev+Feedb1, … (**factory data has values up to 29**, including pattern delays) |
| 113 (`ms`) | Effect Send | 0–127 |
| 114–119 (`ms,np`) | Delay Time · Feedback · Rate / Reverb Decay · Depth / Reverb Room Size (0 Ambience, 1 Small, 2 Large, 3 Hall) · LFO Shape / Reverb Damping · Color (±) | meaning depends on A112 |
| 123 | All Notes Off | performance |
| 92, 95, 96, 103, 104, 111, 120–122, 124–127 | undocumented | preserve |

### 4.2 Page B (dump bytes 128–255)
| No. | Parameter | Values |
|---|---|---|
| 1–6 | Arp Mode (0 Off, 1 Up, 2 Down, 3 Up&Down, 4 AsPlayed, 5 Random, 6 Chord), Pattern (0–31), Octave Range (0–3), Hold (0/1), Note Length (±), Swing (0–127 = 50–75%) | |
| 7–13 | LFO3 Rate, Shape, Mode, Keyfollow, Destination (0 Osc1, 1 Osc1+2, 2 Osc2, 3 PW1, 4 PW1+2, 5 PW2, +SyncPhase since 1.02), Osc Amount, Fade-In | |
| 16 | Clock Tempo | 0–127 = 63–190 BPM |
| 17–21 | Arp Clock (1–17 = 1/64…1/1), LFO1 Clock, LFO2 Clock (0–19 Off, 1/64…4/1), Delay Clock (0–16), LFO3 Clock | |
| 25–28 | Control Smooth Mode (0 Off, 1 On, 2 Auto, 3 Note), Bender Up (±), Bender Down (±), Bender Scale (0 Lin, 1 Exp) | |
| 30, 31 | Filter1 / Filter2 Env Polarity | 0 Neg, 1 Pos |
| 32, 33 | Filter2 Cutoff Link (0/1), Filter Keytrack Base (note) | |
| 35, 36 | Osc Init Phase (0 Off), Punch Intensity | |
| 39 | Vocoder Mode | 0 Off, 1 Osc, 2 OscHold, 3 Noise, 4 In L, 5 In L+R, … (0–12) |
| 47–50 | Velocity → Osc1 Shape, Osc2 Shape, PW, FM Amount | ± |
| 54–57 | Velocity → Filter1 EnvAmt, Filter2 EnvAmt (the table prints "Filter1" twice), Reso1, Reso2 | ± |
| 60, 61 | Velocity → Amp, Panorama | ± |
| 62, 63 | Definable 1 / 2 Single | Definable list (§4.5) |
| 64–66 | **Assign 1:** Source, Destination, Amount (±) | |
| 67–71 | **Assign 2:** Source, Dest1, Amount1, Dest2, Amount2 | |
| 72–78 | **Assign 3:** Source, Dest1, Amt1, Dest2, Amt2, Dest3, Amt3 | |
| 79–82 | LFO1 Assign Dest, Amount (±); LFO2 Assign Dest, Amount (±) | |
| 84–89 | **OS 2.0 Phaser:** Mode (0 Off, 1–6 stages), Mix, Rate, Depth, Feedback (±), Spread | Factory patches already contain phaser settings |
| 97, 98 | Analog Boost Intensity, Tune | |
| 99 | **OS 2.0** Input Ring Modulator | 0 Off, 1–127 Direct…RingMod…Input |
| 112–121 | **Single Name** chars 1–10 | ASCII 32–127 |
| 122 | Filter Select (panel knob target) | 0 Filt1, 1 Filt2, 2 Both |
| 123, 124 | **Category 1, Category 2** | §4.5 |
| 0, 14, 15, 22–24, 29, 34, 37, 38, 40–46, 51–53, 58, 59, 83, 90–96, 100–111, 125–127 | undocumented (vocoder settings, reverb predelay/colour, etc. are probably here) | preserve |

### 4.3 Page C (SysEx only), selected
* **C5–14:** Multi name. **C22:** Delay Output Select.
* **C31–41** (per part): Bank Select, Bank Change, Program Change, MIDI Channel, Low Key, High Key, Transpose, Detune, Part Volume, MIDI Volume Init, Output Select.
* **C72–78:** part enable / volume enable / hold enable / keyboard to MIDI / priority / program change enable.
* **Globals:**
  * C85–87: program change, multi program change and volume enables
  * C90 Input Thru, C91 Input Boost, C92 Master Tune
  * **C93 Device ID** (0–15, 16 = Omni)
  * **C94 / C95 MIDI Control Low / High Page**
  * C96 Arp Send, C97 Knob Display
  * C98 Dump Tx, C99 Dump Rx
  * C105 Multi Program Change, C106 Clock Rx
  * C110–115 Definables, C116 Expert, C117 Knob Mode, C118 Memory Protect, C120 Soft Thru
  * C121 Panel Destination
  * **C122 Play Mode** (0 Single, 1 Multi-Single, 2 Multi)
  * **C123 Part Number** (0–15 or `40`)
  * **C124 Global Channel**, C125 LED Mode, C126 LCD Contrast, C127 Master Volume

### 4.4 Multi dump layout (256 bytes)
* **0–3** internal. **4–13** name. **14** internal.
* **15** Clock Tempo. **16–24** Multi Delay (mode, time, feedback, rate, depth, shape, output, clock, color). **25–31** internal.
* Per part, 16 bytes each:
  * **32–47** bank (0–1)
  * **48–63** program
  * **64–79** MIDI channel
  * **80–95** low key
  * **96–111** high key
  * **112–127** transpose
  * **128–143** detune
  * **144–159** volume
  * **160–175** MIDI volume init
  * **176–191** output
  * **192–207** effect send
* **208–239** internal.
* **240–255** part state bitfield: bit 0 enable, 1 MIDI volume enable, 2 hold enable, 4 internal, 5 priority, 6 program change enable. Bit 3 is undocumented but set in factory data (`0x4F`); keep it like bit 4.
* **Observed in the factory multis (`multis.mid`):** multi 0 and the edit buffer are "Sequencer"; bytes 0–3 look like `01 00 00 mm` but stay internal; part bank values up to **2** occur (documented 0–1), so keep raw values; output select uses 1/4/7 for three stereo outputs, which suggests 0–8 = Out1 L, L+R, R, Out2 …, Out3 … (only 0–2 are documented; live test).
* **Live editing** (`virus_syx::MULTI_PARAMS`): name → C5–C14, delay output → C22, part bank → C32 (immediate), program → C33, channel … output → C34–C41, part-state bits → C72/C73/C74/C77/C78, all addressed to the part (`72 pp nn vv`). Effect send has no Page C number; it is sent as the part's **A113** (class `ms`). Clock tempo and the other multi-delay bytes have no documented parameter change, so they reach the synth only with a Multi dump to the edit buffer (`11 00 00`). All of this needs a live test (TODO G2).

### 4.5 Value lists
* **Categories (B123/B124), manual p. 84, 0-based:** 0 Off, 1 Lead, 2 Bass, 3 Pad, 4 Decay, 5 Pluck ("Plug" in the manual), 6 Acid, 7 Classic, 8 Arpeggiator, 9 Effects, 10 Drums, 11 Percussion, 12 Input, 13 Vocoder, 14 Favourite 1, 15 Favourite 2, 16 Favourite 3.
  * The factory data agrees: *Smoog* has 2 (Bass); *JunoArp* has 8 (Arpeggiator) and 6 (Acid).
  * One factory patch has Category 2 = 127, so out-of-range values exist.
* **Assign sources (B64/67/72), 0-based:** 0 Off, 1 PitchBnd, 2 ChanPres, 3 ModWheel, 4 Breath, 5 Contr3, 6 Foot, 7 Data, 8 Balance, 9 Contr9, 10 Express, 11–15 Contr12–16, 16 HoldPed, 17 PortaSw, 18 SostPed, 19 AmpEnv, 20 FiltEnv, 21 Lfo1, 22 Lfo2, 23 Lfo3, 24 VeloOn, 25 VeloOff, 26 KeyFlw, 27 Random. The factory maximum is 27, which fits.
* **Assign / LFO-assign destinations:** the OSR15 addendum lists **105 names**, but factory patches use values up to **114**. **The printed order is not the raw value mapping.** Show unknown values as raw numbers until the mapping is confirmed (open question Q1).
* **Definable destinations (B62/63, C112/113):** the list is in the OSR15 addendum. OS 1.50 re-sorted it alphabetically in the UI, so the raw mapping is also unconfirmed (Q1).

---

## 5. Editor & Librarian Rules

These follow from the addendum's "Remarks for editor/librarian programs" (p. 12) and the factory data.

1. **Byte-exact preservation.** Not all 256 bytes are defined parameters, and undefined bytes "should be sent to the Virus on the same value as they were received". In the factory banks, **33 undocumented offsets hold non-zero data**, several of them in every patch (e.g. A92, A104, B22, B42, B51–53, B90–96, B125–127). Store the 256 raw bytes as the single source of truth. Typed accessors read and write individual bytes and **never normalise** values they don't change. Enums need an `Unknown(u8)` variant, because factory values already exceed the documented ranges (A49, A112, destination lists).
2. **Sound Version (A0)** must never be changed by the editor. The Virus updates old sounds itself, so an imported sound should ideally be passed through the synth (send, then request back) before editing.
3. **Name and categories live in the patch** (B112–121, B123–124). The librarian should read them from the patch and write them back to the patch, not keep separate copies in the database that can drift.
4. **Names:** exactly 10 characters, ASCII 32–127, padded with spaces. Never write a byte above 127.
5. **Live editing:** send SysEx parameter change `70`/`71` to part `40` (Single mode) or the selected part. Don't use CC/PolyPressure for output; PolyPressure is off by default. Section paste (§5.11) goes out the same way. Undo, redo and Compare do too when a step touches at most 16 documented sound bytes; otherwise they send one edit-buffer dump (§5.11). A0 is never written.
   * **Incoming CC/PolyPressure:** accept only on the synth's channel, and only for sound parameters (class `a`/`b`), never performance controllers.
   * **Incoming SysEx parameter changes:** apply them to the part they address.
6. **Bulk files:** import must honour `bb`. Multi dumps (`11`) in the same file go to the Multi bank (`bb=01`) or the Multi tab's loose list (`bb=00`). Occupied library slots are never overwritten without asking: the Librarian's import confirms occupied single and Multi slots, and the Multi tab's imports (received from the synth, or a file) ask too, with **Overwrite / Only empty slots / Cancel** (`multi_session::prepare_multi_import` / `commit_multi_import`). Duplicate slots within one import are reported, for singles and multis.
   * `00` → edit-buffer dump. Never put it into a bank slot by its part number.
   * `01–04` → banks A–D.
   * Multi dumps (`11`) go to the Multi bank or the loose list (above). Unknown messages (e.g. the Global dump `12`) are skipped with a count, never misread as singles.
   * A file can hold several banks (`multis.mid` is a Total dump with edit buffers, A, B, multis and globals).
   * Data bytes ≥ 0x80 are never stored or exported, for singles and multis (skipped with a count).
7. **Sending banks overwrites RAM irrevocably** (manual p. 177). The same applies to the Multi bank: the app confirms, asks for a backup (all 128 multis received and saved to a file), skips flagged multis by default and paces at 300 ms of silence after each dump (§3.7). Confirm before sending A/B bank dumps, and offer a backup first (request A and B). **Flagged ⚠ patches and multis** (wrong checksum, or migrated from an old library) are left out of bank sends and of bank file exports by default, for singles and multis alike; the user can choose "⚠ as stored" (sent or written with the checksum still wrong, §3.2) or, for file exports only, "⚠ fix checksum".
8. **Bundled third-party material:** the manual, addenda, OS files and factory sounds are © Access Music. They stay in this repository only because it is **private** (Q4). Don't publish them.
9. **Note audition** (owner request, 24 Nov): the editor can play notes on the connected Virus to test patch changes without a keyboard. Details in §5.9.

10. **Settings persistence** (G3): the app remembers MIDI port names, synth channel, device ID, edited part, audition settings, last tab (including the Multi tab) and bank, import/export folders (shared by the Librarian and the Multi tab; the one used last is saved) and window geometry in a versioned `settings.json` in the data directory (next to the library). Loading tolerates unknown or invalid fields (defaults), a corrupt file is renamed to `.bak` with a visible notice, and a saved port that no longer exists is not connected under that name: a port with the same base name (Windows renames, §3.7) is used if present, otherwise auto-detect stands in (a Virus-looking port, else the only real port, via `virus_midi::auto_pick_port`; the CLI has its own stricter `select_port`), and the saved name is kept so the app reconnects when the port returns. The "fetch before store" choice and a "Not now" answer to the legacy-library prompt are saved too. Hold/latch is never persisted. Our own JSON file was chosen over eframe's `persistence` for per-field validation and testability.
11. **File dialogs and import/export** (U5) run off the UI thread (`dialogs.rs`): one task at a time with a busy indicator, on the Librarian and the Multi tab. File reads and writes happen on the worker; all database work stays on the UI thread.
12. **Storing the editor patch** (R12): other controllers can change the synth behind the editor's back (§3.7). The Store dialog has "Fetch from synth first", available only with an output connected and no bank send running (the disabled checkbox says which): it requests the edit buffer, adopts it through the normal fetch path (replaced unsaved edits stay undoable), then stores it in the chosen slot. The 10 s timeout starts when the request actually goes out (it may first wait for a quiet line, §3.7); a request still queued when the store is given up is dropped. A timeout or a wrong checksum stores nothing. The overwrite confirmation (RV-H1) applies as before.
13. **The ⚠ flag travels with the sound** (CR-H1). A patch or multi that came from a dump with a wrong checksum, or from a migrated old library, stays flagged when it is stored anywhere: into another slot, from a loose edit-buffer dump, or from the editor after loading it (`library::EditorFlag`, `MultiSession::load(.., flagged)`). Only "✔ Mark as checked…" clears the flag, after an explanation and a confirmation (`MARK_CHECKED_EXPLANATION`): per patch, per bank, and per multi on the Multi tab (`mark_checked`, `mark_bank_checked`, `mark_multi_checked`). The stored bytes are never changed by it.
14. **One library, several programs:** the GUI and `virus get-bank --to-library` may open the same library. SQLite waits up to 2 s for a lock (busy timeout) before reporting an error.
15. **Adopting a legacy library** (RV-M3, CR-M5): the "Copy and upgrade" prompt for an old `virus_library.db` only replaces an **empty** library (no patches, no multis and no user banks). The current library is first copied to `<library>.pre-adopt.bak` (`.2`, `.3`… if one exists). "Not now" is remembered for that file.

### 5.9 Note audition

**Why:** the owner edits sounds with no keyboard attached and needs to hear each change immediately. The Virus' own *Audition* (UP+DOWN on the panel, manual p. 90) plays only C3; the editor should do more.

**MIDI facts (manual pp. 54–58, 90, 168–170; addendum):**
* **Single mode:** the Virus plays notes received on the **Global Channel** (C124, default 1).
* **Multi / Multi-Single mode:** each part plays notes on its own **Part MIDI Channel** (C34; multi dump bytes 64–79). Audition must target the channel of the **part being edited**. If several parts share that channel, they all sound (layering), and the part's **Low/High Key** (C35/C36) and **Transpose** (C37) still apply.
* Notes are plain channel messages: Note On `9c nn vv`, Note Off `8c nn 40` (vv = 0 is not used for note off, so that it's unambiguous). Notes 0–127; the Virus calls note 60 **C3**.
* **Velocity matters:** Page B **B47–B61** (Velocity → Osc shapes, PW, FM, filter env amounts, resonance, Amp, Pan) make velocity part of what is being tested.
* **Hold pedal** = CC64 (A64, performance controller); value ≥ 64 holds, < 64 releases. Not stored in the patch.
* **All Notes Off** = CC123 (A123); **All Sound Off** = CC120 (also cuts release tails). Both value 0.
* The **arpeggiator** (B1 Arp Mode ≠ Off, B4 Arp Hold) plays the held notes, so holding/latching notes is how arpeggiator patches are tested.

**Requirements:**
1. **On-screen piano keyboard** (at least 2 octaves, scrollable by octave). Click to play, drag across keys to glide from note to note (old note off before new note on). Velocity comes from the vertical click position on the key (top = soft, bottom = loud) or, if the user prefers, a fixed **velocity slider** (1–127). Held keys are highlighted.
2. **Computer keyboard playing**: `A W S E D F T G Y H U J K` = C, C♯, D … C one octave up; `Z` / `X` = octave down / up. Active only when **no text field has focus**, and key repeat must not retrigger notes.
3. **Octave shift** (buttons and Z/X), range so that every note stays in 0–127.
4. **Hold / latch toggle**: keys stay on after release until pressed again or hold is turned off (for pads and the arpeggiator). Optionally send CC64 instead of latching in the editor.
5. **Audition button**: one click plays a configurable note or chord (default C3, velocity 100) for a configurable length (default 1 s), like the Virus' own Audition.
6. **Test phrases** (optional): single note, chord, bass line, played at a set tempo. A phrase is a list of (start ms, note, velocity, length ms) relative to its start; it can be stopped at any time.
7. **Panic button**: note offs for all tracked notes, then **CC123 All Notes Off** and **CC120 All Sound Off** on the target channel (and CC64 = 0 if the editor sent a hold). The same button sits in the app header (any tab) and, in Multi mode, also covers every enabled part's channel; the CLI has `virus panic [--all-channels]`.
8. **No stuck notes:** every note on is tracked per channel and gets a note off on **key release, octave change, channel change, part change, MIDI disconnect, window focus loss and app exit**. Hold/latch is released by the same triggers except octave change.
9. Notes go out through `virus_midi::MidiCommand::Send` (immediately, never queued behind a slow bank batch); the **Tx LED** flashes through the existing `Sent` event.
10. The feature is available on **every tab** (a collapsible strip above the MIDI bar) and greys out when no MIDI output is connected.

**Implementation:** pure builders (`note_on`, `note_off`, `control_change`, `all_notes_off`, `all_sound_off`), `NoteTracker` and `PhrasePlayer` live in `virus-midi` (`notes` module) and are unit-tested. The egui piano widget and the audition state (computer-key mapping, settings) live in `virus-app` (`widgets/piano.rs`, `audition.rs`). In Multi mode the target is the edited part's channel from the fetched Multi (`audition::auto_channel` with `MultiSession::edited_part_channel`, TODO N9); until a Multi has been fetched, part *n* is assumed to listen on channel *n*. The strip also has a manual channel override.

### 5.10 Look and feel (owner requirement, 24 Nov)
The UI should look like the Virus Rack's front panel: **crimson and black**. A second, optional theme follows the **Virus TI** (dark gunmetal, light blue highlights, red LEDs, grey LCD); the header's 🎨 menu switches between them and the choice is saved in `settings.json` (`ui.theme`: `rack` or `ti`). Rack palette, sampled from a photo of the unit:

| Role | Colour | Where |
|---|---|---|
| Panel red | `#C42343` (crimson, the photo's average) | Header bar, section title strips, selected tab, held piano key |
| Arc red | `#E03A5C` (the same hue, lighter: the panel red is only 2.9:1 on control black) | Knob arcs, filter curve |
| Accent text | `#FF6A84` | Red text on black backgrounds |
| Panel red, dark | `#8A152E` | Hover states, header and section outlines |
| Chassis black | `#141414` | Window and panel backgrounds |
| Control black | `#1E1E1E` / `#2A2A2A` | Knob bodies, buttons, combo boxes |
| Legend | white on red (the hardware's legends; black on this red is only 3.7:1), `#E8E8E8` on black | Labels; text on red must stay readable (WCAG AA) |
| LED amber | `#FFB000` | Rx/Tx and tab LEDs, envelope curves |
| LCD | dark slate `#1C2A33` with light text `#CFE3EA` | Patch name / value readouts |

Keep all colours in one theme module so they can be adjusted in one place. **Implemented** in `crates/virus-app/src/theme.rs` as one `Palette` per theme (`RACK`, `TI`) plus the colours they share; a unit test checks WCAG AA contrast (≥ 4.5:1 for text, ≥ 3:1 for graphics) for every colour pair in both themes, so retuning can't silently break readability. Tabs carry LEDs in a black bezel (amber directly on red is only 2.3:1). The header starts with a blocky, slanted "VIRUS" wordmark (white on a black plate, drawn with shapes) followed by the app name in spaced uppercase monospace, like the LCD's start-up screen.

### 5.11 Editor comforts (24 Nov)
* **Undo/redo:** a history of byte-level changes (offset, before, after) to the edited patch, at most 200 steps. A continuous knob drag is one step (ended by pointer release or a 1 s pause). Load and Init start a new history.
* **Compare:** switches the synth and the editor between the edited patch and its **original**, which is where it was loaded from (library slot, file or synth fetch). Only the differing bytes are sent. Editing is disabled while comparing, and leaving Compare restores the exact edited bytes.
* **What undo, redo and Compare send** (`history::apply_step`, CR-M3): parameter changes (`70`/`71`) only when the step touches at most 16 bytes and all of them are documented sound parameters. Otherwise (e.g. undoing a Fetch or an incoming dump, which can differ in performance-controller bytes such as A64 Hold Pedal, or in undocumented bytes) they send **one edit-buffer dump** with the synth's current A0 kept, never a burst of parameter changes to performance or undocumented numbers.
* **Diff:** lists the differences between the edited patch and its original or any library patch. Named parameters show formatted values; undocumented bytes show as raw offsets.
* **Sections:** copy, paste or init all documented sound parameters of one `ParamDef.section`. Never touches A0, the name, the categories, performance controllers or undocumented bytes (§5.1).
* **Two states are always visible:**
  * **edited:** differs from the original;
  * **on synth:** the synth's edit buffer is believed to hold the editor's bytes. It becomes false after Load, Init, or a part/device change, until Send or Fetch.
* Replacing an edited patch asks first: Replace, Store first, or Cancel. The wording depends on the action (after Fetch the edits stay undoable). "Select on synth" is sent only once the replace has actually happened. An incoming edit-buffer dump over an edited patch is kept as one undo step.

---

## 6. Ground-Truth Test Data

> ⚠️ **The factory `.mid` files are damaged.** An old LF→CR text conversion turned every `0x0A` byte into `0x0D`; the files contain no `0x0A` at all. Effects:
> * some SMF SysEx lengths (`82 0A` → `82 0D`) are wrong;
> * Bank A's program 10 appears as 13;
> * checksums fail on 20 dumps in Bank A, 32 in Bank B and 170 of 418 in `multis.mid` (87 of its 129 multi dumps); multi 10 reads as `mm = 13`.
>
> Parsers must resynchronise on `F7`. **Clean copies of Banks A and B** (256/256 recovered with certainty from the undamaged SoundDiver `.lib`, cross-checked against the `.mid` dumps) are in `testdata/factory/Bank_A.syx` and `Bank_B.syx`; see `testdata/factory/README.md`. Strict golden tests (`crates/virus-syx/tests/clean_factory.rs`) require byte-identical round trips on them. `multis.mid` is a different soundset, so it isn't a source for the Rack banks.

| File | Contents |
|---|---|
| `Factory soundset/OSR_FactoryPatches_Bank_A.mid` | SMF with 128 single dumps, `dd=10`, `bb=01`. Prog 0 = `OvertureCK`, 1 = `Smoog   OK`, 2 = `JunoArp JS` (matches the patch list). A0 = 6 in all. 20 bad checksums (damaged) |
| `Factory soundset/OSR_FactoryPatches_Bank_B.mid` | 128 single dumps, `bb=02`. 32 bad checksums (damaged) |
| `Factory soundset/multis.mid` | Total dump: 33 single dumps with `bb=00` (edit buffers), 128 × `bb=01`, 128 × `bb=02`, 1 multi edit buffer, 128 multis, 1 command-`12` message. Damaged the same way |
| `Factory soundset/Virus_rack_factory_patches.lib` | **Emagic SoundDiver library** (`FORM…ESSL`), not SysEx. Apparently undamaged |
| `OSR15 Patch list.pdf` | Names for banks A–H |

Required tests: import → export is byte-identical for both banks; names and categories decode as listed; checksums verify.

---

## 7. Open Questions

| # | Question | How to resolve |
|---|---|---|
| Q1 | Raw value → name mapping for assign destinations (0–114+) and definable destinations | **Live test** (tool ready): `virus probe-destinations --channel 11` (also `--slot lfo1`, `--slot definable1`), then `virus probe-table` to generate the name table. CLI runbook step 10 |
| Q2 | How banks E–H (OS 1.5+ ROM) are addressed in dumps and requests (`bb` 05–08?) and whether they can be requested | Live test: `virus get-bank E --out-file probe_E.syx --timeout-ms 2000 -v --channel 11`. Runbook step 12 |
| Q3 | Full value lists for A49 (up to 30 seen), A112 (up to 29), A102, B39 | Live test: `virus probe-values A49 --channel 11` (likewise A112, A102, B39). Runbook step 11 |
| ~~Q4~~ | ~~Can the Access material stay in git?~~ | **Decided 24 Nov:** the repository is private and will stay private, so the material stays |
| ~~Q5~~ | ~~Support other Virus models?~~ | **Decided 24 Nov:** Virus Rack (classic) only for now |

---

## 8. Status & Roadmap

Detailed tasks are in [TODO.md](TODO.md).

| Phase | State |
|---|---|
| 1. Domain crate (`virus-syx`) | ✅ Rebuilt 24 Nov: raw-byte `Patch`, parameter table from §4, lossless enums, SysEx parse/encode per §3, file reader. All 256 factory patches round-trip byte-identically |
| 2. Git setup | ✅ Lockfile committed; binary attributes for MIDI files |
| 3. MIDI engine (`virus-midi`) | ✅ Reworked 24 Nov as a plain transport; interpretation (§5.5) moved to the app |
| 4. Librarian DB (`virus-db`) | ✅ Reworked 24 Nov: byte-exact storage, schema v3 + migrations (v1→v2→v3), bank-aware import/export (§5.6), Multi bank storage |
| 5. GUI (`virus-app`) | ✅ Rebuilt 24 Nov on the new crates (generated editor, Assign panel, 128-slot librarian, synth transfer, editor comforts). Starts and renders on the GPU-less dev VM via the wgpu/WARP fallback (TODO G1); ⚠️ not yet used with the synth (G2, E7, N8) |
| 6. Done 24 Nov | Note audition (TODO N items); **Multi editor and librarian** (F4/F5/F9, §4.4) including the Multi bank send, with the audition on the part's real channel (N9) |
| 6b. CLI (`virus-cli`) | ✅ Built 24 Nov and **used with the synth the same day** (TODO G2 results): fetch, param changes, bank backups, Global request, selftest 5/5. Found the timing rule (§3.7) |
| 7. Needs the synth | Rest of the CLI runbook: audible checks, destination names (Q1), value lists (Q3), banks E–H (Q2), C32 (F7), Multi addressing, bank send. Then the GUI live checks |
| 8. Later | Send a patch to a bank slot (F2), synth globals (F3), pass imports through the synth (F6), banks E–H (F8), controller-dump sync, arrangement/total dump import |

---

## 9. Changelog
* **2026-11-24 (late evening, code review 2; uncommitted on `main` until the owner commits)**
  * Second code review of `cc7cd97` (report outside the repo: `Downloads\VirusEditor-review-2026-11-24.md`; TODO "🔍 Code review 2"). All findings fixed except H11 (`cargo fmt`).
  * Data safety: the ⚠ flag survives Store and is cleared only by "Mark as checked" (now for multis too) (§5 rule 13); Multi-tab imports ask before overwriting (rule 6); Multi bank file export skips flagged multis by default (rule 7); 7-bit guards for multis; legacy adopt only replaces an empty library and keeps `.pre-adopt.bak` (rule 15); 2 s SQLite busy timeout (rule 14); Librarian "Clear slot…" and "Delete bank…"; Multi delete/overwrite confirmations.
  * Live MIDI: bank-send gap = silence after each dump, one constant `virus_midi::DUMP_GAP_MS` (≈49 s per bank); `BatchFinished`; port polling every 2 s and reopening a dead output; fetch-before-store timeout from when the request goes out (§3.7, rule 12).
  * Editor: undo/redo/Compare send one dump for large or non-sound steps (§5.11); one C32 helper for Select on synth and the Multi Part Bank row (§3.5).
  * CLI: `--allow-bad-checksum` keeps the checksum wrong (§3.2); quiet waits give up after max(10 × quiet, 20 s); complete recovery hints; CI builds and uploads `virus.exe`, actions pinned.
  * 439 workspace tests, clippy clean.
* **2026-11-24 (late afternoon)**
  * **First live session** (laptop + mioXC, CLI): Virus on channel 11, Device ID 1; fetch, param changes, bank A/B backups and the Global request (`35` → `12`, §3.6) work; selftest 5/5. Found that the Virus ignores requests that come too soon (§3.7): quiet gaps added to the CLI and the app (G4); bank sends paced at 300 ms.
  * Follow-ups: Compare/replace fixes (E6), "mark as checked" (R9), multis in the main file import and the Multi bank send (F9), tab LEDs and watermark (V2), CLI stream diagnostics.
  * CLI: `set` for Page C (with guarded globals) and raw `send-hex`, to test the C32 question (§3.5).
  * Studio notes (§3.7); bank-send confirmations remind you to stop the sequencer transport.
  * 308 workspace tests, clippy clean, CI green on Windows and Ubuntu. Added `AGENTS.md` for AI agents picking up the work.
  * Studio robustness (R10–R12): the app reconnects to a renamed port (Windows' `2- …` prefix) and returns to the saved name when it's back, with the same auto-pick as the CLI (shared in `virus-midi`); Panic in the header and `virus panic`; "Fetch from synth first" in the Store dialog. 322 workspace tests.
* **2026-11-24 (evening)**
  * Settings persistence (G3), off-thread file dialogs and import/export (U5) and the optional SysEx "Select on synth" (F7, C32 bank value to be verified on hardware).
* **2026-11-24 (late morning)**
  * The GUI now starts on the dev VM: if OpenGL 2.0+ is missing, it falls back to wgpu on Direct3D 12 WARP (TODO G1, §1.5).
* **2026-11-24 (late morning, `rework/multi`)**
  * Multi programs (F4/F5): `virus-syx` `Multi` + `MULTI_PARAMS` with the live Page C mapping (§4.4); golden test on all 129 multi dumps of `multis.mid`; `virus-db` schema v3 (`multis` table, transactional v2→v3); Multi tab in the app.
  * Note audition follows the edited part's MIDI channel from the fetched Multi (N9).
  * `rework/integration` merged in (theme, CI, review, note audition); 166 workspace tests, clippy clean.
* **2026-11-24 (late morning)**
  * New crate `virus-cli` (binary `virus`), on branch `rework/cli`. Commands:
    * `ports`, `monitor`, `fetch`, `send`, `set`;
    * `get-bank` (optionally into the library);
    * `put-bank` (RAM A/B only; `--yes`; automatic backup first; 300 ms gaps (was 150 before the live timing, §3.7); `--verify`);
    * `probe-destinations` (Q1), `probe-values` (Q3), `probe-table`;
    * `selftest` (G2).
  * The probes back up and restore the edit buffer, read each value back to find where ranges end, and save JSON after every step.
  * The library crates are unchanged; 39 new tests.
* **2026-11-24 (afternoon)**
  * Library crates reworked in parallel (`virus-syx`, `virus-midi`, `virus-db`) and merged into `rework/integration`.
  * Factory `.mid` files found to be LF→CR damaged; clean Banks A/B recovered from the SoundDiver `.lib` into `testdata/factory`.
  * App rebuilt on the new crates (98 workspace tests, clippy clean with `-D warnings`, release build OK).
  * New requirement: note audition, so you can play the synth from the editor (in progress).
* **2026-11-24**
  * Owner request: **note audition** (§5.9): play notes on the synth from the editor (piano, computer keys, Audition button, hold, panic, no stuck notes).
  * Code review: see TODO.md.
  * Re-read the manual, both addenda, the patch list and the OS 2.00 readme.
  * Rewrote §1, §3–§8 from those sources, and checked them against the factory dumps.
  * Corrected: the parameter map (previously unverified), the bank numbers, the CC/PolyPressure defaults and the matrix model.
  * Added: editor rules, ground-truth data and open questions.
* **2026-11-23**
  * Workspace and crates `virus-syx`, `virus-midi`, `virus-db` and `virus-app` created.
  * GitHub remote linked.


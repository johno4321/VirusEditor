# Access Virus Rack Editor & Librarian — Specification

## 1. Project Overview

### 1.1 Goal
A native desktop **sound editor and patch librarian** for the **Access Virus Rack (classic), OS 2.00**, written in Rust. It must:

1. **Edit** the synth's current sound live over MIDI, with every change heard immediately.
2. **Fetch and send** single sounds (and later multis) between the synth and the computer.
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
  * Multi-crate Cargo workspace
  * Pure domain crate (`virus-syx`) structured cleanly with a `domain` module
  * Dedicated MIDI I/O thread (`midir` in `virus-midi`) communicating via lock-free channels
  * Local patch database & indexing (`rusqlite`)
  * Native immediate-mode GUI (`egui` / `eframe`)

---

## 2. Architecture & Crate Layout

```text
VirusEditor\
├── Cargo.toml                  # Workspace definition
├── SPEC.md                     # Specification (this file)
├── TODO.md                     # Known issues and planned work
├── README.md
├── OSR1_Manual_International.pdf # Virus Rack manual
├── VirusOSR200_INT\            # OS 2.00 update: addenda (parameter tables), patch list, factory soundset
└── crates/
    ├── virus-syx/              # Core Domain: SysEx, 256-byte Patch, Checksums, Enums, Bipolar types
    │   ├── src/
    │   │   ├── lib.rs          # Crate root entrypoint (exports pub mod domain)
    │   │   └── domain/         # Domain model layer
    │   │       ├── mod.rs      # Module declarations and re-exports
    │   │       ├── checksum.rs # 7-bit checksum calculator & validator
    │   │       ├── types.rs    # Bipolar7 (-64..+63), Unipolar7, Osc/Filter/LFO/Matrix enums
    │   │       ├── sysex.rs    # Framing, header validation, dump & param encoders/parsers
    │   │       ├── page_a.rs   # Page A typed view (currently WRONG offsets, see §4 and TODO P1)
    │   │       ├── page_b.rs   # Page B typed view (currently WRONG offsets, see §4 and TODO P1)
    │   │       └── patch.rs    # 256-byte container combining Page A + B with syx export/import
    │   └── Cargo.toml
    ├── virus-midi/             # Hardware I/O: midir callbacks, port handling, SysEx stream buffering
    │   ├── src/
    │   │   ├── lib.rs          # Public MIDI crate API
    │   │   ├── stream.rs       # SysExStreamAssembler (filters real-time 0xF8..FF, reconstructs dumps)
    │   │   ├── events.rs       # MidiCommand and MidiEvent channel definitions
    │   │   ├── ports.rs        # Device scanner & "Virus" port auto-detection
    │   │   └── engine.rs       # MidiService background thread & non-blocking UI bridge
    │   └── Cargo.toml
    ├── virus-db/               # Librarian DB: SQLite schema, full-text search, categories, bank imports
    │   ├── src/
    │   │   ├── lib.rs          # Public crate API
    │   │   ├── schema.rs       # DDL definitions (banks, patches, indexes)
    │   │   ├── models.rs       # 17 Sound Categories, BankRecord, PatchRecord, PatchFilter
    │   │   └── db.rs           # PatchDatabase connection, CRUD, queries, SysEx bank import/export
    │   └── Cargo.toml
    └── virus-app/              # Native GUI: eframe/egui desktop app
        ├── src/
        │   ├── main.rs         # Entrypoint & window viewport config
        │   ├── app.rs          # VirusApp struct, event loop & tab routing
        │   ├── widgets/        # Custom widgets
        │   │   ├── mod.rs      # Widget exports
        │   │   ├── knob.rs     # Unipolar7 & Bipolar7 rotary knobs with detents
        │   │   └── curves.rs   # Dual filter frequency response & ADSR envelope curve painters
        │   └── views/          # Panels
        │       ├── mod.rs      # View exports
        │       ├── synth_panel.rs # Oscillators, Filters, Envelopes, LFOs, EQ & FX
        │       ├── matrix_panel.rs # 6-slot Modulation Matrix & Arpeggiator editor
        │       ├── librarian_panel.rs # Bank browser, search, categories, syx import/export
        │       └── midi_bar.rs # Ports, Device ID, activity LEDs, hardware buffer fetch
        └── Cargo.toml
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
* `cs` = (dd + 10 + bb + ss + Σdata) AND 7F. The manual calls the checksum "optional": the Virus **accepts a wrong checksum** but shows an error. The editor must always send a correct checksum. On import it should report a mismatch rather than silently drop the dump.

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

### 3.6 Observed but undocumented
* `multis.mid` contains one `F0 00 20 33 01 10 12 01 …` message, probably a **Global dump** (command `12`). It isn't documented, so keep it raw and pass it through untouched.

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
| 0 | *internal:* **Sound Version Number** (`6` in all factory OS 2.0 dumps) | never change (§5.2) |
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
* **240–255** part state bitfield: bit 0 enable, 1 MIDI volume enable, 2 hold enable, 4 internal, 5 priority, 6 program change enable.

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
5. **Live editing:** send SysEx parameter change `70`/`71` to part `40` (Single mode) or the selected part. Don't use CC/PolyPressure for output; PolyPressure is off by default.
   * **Incoming CC/PolyPressure:** accept only on the synth's channel, and only for sound parameters (class `a`/`b`), never performance controllers.
   * **Incoming SysEx parameter changes:** apply them to the part they address.
6. **Bulk files:** import must honour `bb`.
   * `00` → edit-buffer dump. Never put it into a bank slot by its part number.
   * `01–04` → banks A–D.
   * Multi dumps (`11`) and unknown messages are preserved or skipped, never misread as singles.
   * A file can hold several banks (`multis.mid` is a Total dump with edit buffers, A, B, multis and globals).
7. **Sending banks overwrites RAM irrevocably** (manual p. 177). Confirm before sending A/B bank dumps, and offer a backup first (request A and B).
8. **Bundled third-party material:** the manual, addenda, OS files and factory sounds are © Access Music. They stay in this repository only because it is **private** (Q4). Don't publish them.

---

## 6. Ground-Truth Test Data

> ⚠️ **The factory `.mid` files are damaged.** An old LF→CR text conversion turned every `0x0A` byte into `0x0D`; the files contain no `0x0A` at all. Effects:
> * some SMF SysEx lengths (`82 0A` → `82 0D`) are wrong;
> * Bank A's program 10 appears as 13;
> * checksums fail on 20 dumps in Bank A, 32 in Bank B and 170 of 418 in `multis.mid`.
>
> Parsers must resynchronise on `F7`. Golden tests must expect these counts, or use the recovered banks in `testdata/factory/` (being recovered from the SoundDiver `.lib`, which still contains `0x0A` bytes).

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
| Q1 | Raw value → name mapping for assign destinations (0–114+) and definable destinations | **Live test** once the MIDI cable arrives: set values on the synth and request a dump |
| Q2 | How banks E–H (OS 1.5+ ROM) are addressed in dumps and requests (`bb` 05–08?) and whether they can be requested | Live test |
| Q3 | Full value lists for A49 (up to 30 seen), A112 (up to 29), A102, B39 | Live test |
| ~~Q4~~ | ~~Can the Access material stay in git?~~ | **Decided 24 Nov:** the repository is private and will stay private, so the material stays |
| ~~Q5~~ | ~~Support other Virus models?~~ | **Decided 24 Nov:** Virus Rack (classic) only for now |

---

## 8. Status & Roadmap

Detailed tasks are in [TODO.md](TODO.md).

| Phase | State |
|---|---|
| 1. Domain crate (`virus-syx`) | ⚠️ Framing, checksum and encoders are correct. **Parameter map wrong** (§4 not followed): name, cutoff, envelopes, LFOs, filters and matrix are all at the wrong offsets. The "6-slot mod matrix" doesn't exist on the Virus (it has Assign 1/2/3 plus LFO1/2 assign). Decoding normalises bytes, which breaks rule §5.1 |
| 2. Git setup | ✅ |
| 3. MIDI engine (`virus-midi`) | ✅ Thread, channels, assembler. ⚠️ incoming CC handling breaks rule §5.5 |
| 4. Librarian DB (`virus-db`) | ⚠️ Export bank numbers wrong (§3.2); import ignores `bb` (§5.6); categories and names kept outside the patch (§5.3) |
| 5. GUI (`virus-app`) | ⚠️ Works, but edits the wrong parameters and has data-loss bugs |
| 6. **Next: correct patch model** | Raw-byte patch with a parameter table generated from §4; factory golden tests |
| 7. Later | Multi editing (§4.4), Page C globals, bank request/backup, controller-dump sync, arrangement and total dump import |

---

## 9. Changelog
* **2026-11-24**
  * Code review: see TODO.md.
  * Re-read the manual, both addenda, the patch list and the OS 2.00 readme.
  * Rewrote §1, §3–§8 from those sources, and checked them against the factory dumps.
  * Corrected: the parameter map (previously unverified), the bank numbers, the CC/PolyPressure defaults and the matrix model.
  * Added: editor rules, ground-truth data and open questions.
* **2026-11-23**
  * Workspace and crates `virus-syx`, `virus-midi`, `virus-db` and `virus-app` created.
  * GitHub remote linked.


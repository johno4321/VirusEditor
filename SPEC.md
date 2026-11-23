# Access Virus Rack / B Editor & Librarian — Specification & Roadmap

## 1. Project Overview
A native, zero-GC hardware synthesizer editor, patch librarian, and real-time controller for the **Access Virus B / Access Virus Rack / Classic / Indigo** series written in Rust.

* **Repository Location:** `D:\VirusEditor`
* **GitHub Repository:** [https://github.com/johno4321/VirusEditor](https://github.com/johno4321/VirusEditor)
* **Reference Documentation:** `OSR1_Manual_International.pdf` (Access Virus Rack User Manual, Chapters 16–20)
* **Target Hardware Architecture:**
  * Access Virus Rack (OSR1: 16 voices, 16-part multi-timbral)
  * Memory: 512 Single Programs (Banks A & B in RAM, Banks C & D in Flash ROM), 128 Multi Programs
* **MIDI Transport Protocols:**
  * **System Exclusive (SysEx):** Complete parameter access (Pages A, B, C) and bulk dumps
  * **MIDI Control Change (CC):** Real-time automation of Page A parameters
  * **MIDI Polyphonic Pressure:** Real-time automation of Page B parameters
* **Core Architecture:**
  * Multi-crate Cargo workspace
  * Pure domain crate (`virus-syx`) structured cleanly with a `domain` module
  * Dedicated MIDI I/O thread (`midir` in `virus-midi`) communicating via lock-free channels
  * Local patch database & indexing (`rusqlite`)
  * Native immediate-mode GUI (`egui` / `eframe`)

---

## 2. Architecture & Crate Layout

```text
D:\VirusEditor\
├── Cargo.toml                  # Workspace definition
├── SPEC.md                     # Living specification & task tracker (this file)
├── .gitignore                  # Rust & build ignores
├── OSR1_Manual_International.pdf # Official Virus Rack Manual
└── crates/
    ├── virus-syx/              # Core Domain: SysEx, 256-byte Patch, Checksums, Enums, Bipolar types
    │   ├── src/
    │   │   ├── lib.rs          # Crate root entrypoint (exports pub mod domain)
    │   │   └── domain/         # Domain model layer
    │   │       ├── mod.rs      # Module declarations and re-exports
    │   │       ├── checksum.rs # 7-bit checksum calculator & validator
    │   │       ├── types.rs    # Bipolar7 (-64..+63), Unipolar7, Osc/Filter/LFO/Matrix enums
    │   │       ├── sysex.rs    # Framing, header validation, dump & param encoders/parsers
    │   │       ├── page_a.rs   # Sound engine parameters (Osc, Filters, Envelopes, LFOs, Name)
    │   │       ├── page_b.rs   # Modulation matrix (6 slots), Arp, EQ, Phaser, FX
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
    └── virus-app/              # UI layer: egui/eframe desktop app with rotary knobs, curves & matrix
```

---

## 3. Verified SysEx Protocol Specification (Manual Chapter 20)

### 3.1 Header & Addressing
* **SysEx Framing:** Starts with `0xF0`, ends with `0xF7`
* **Manufacturer ID:** `0x00, 0x20, 0x33` (Access Music Electronics)
* **Product ID:** `0x01` (Virus family: A, B, C, Rack, Classic)
* **Device ID (`dd`):** `0x00..0x0F` (Individual Devices 1–16), `0x10` (Omni / All Devices)

### 3.2 Real-Time Parameter Change (`0x70`, `0x71`, `0x72`)
Format:
```text
F0 00 20 33 01 <dd> <7x> <pp> <nn> <vv> F7
```
* **Command `<7x>`:**
  * `0x70`: Page A (Sound parameters 0..127)
  * `0x71`: Page B (Extended sound / FX / Matrix parameters 0..127)
  * `0x72`: Page C (Multi and Global parameters)
* **Part Number `<pp>`:**
  * `0x40`: Single Edit Buffer (in Single Mode)
  * `0x00`..`0x0F`: Multi Part 1 to 16
* **Parameter Number `<nn>`:** `0x00`..`0x7F` (0..127)
* **Parameter Value `<vv>`:** `0x00`..`0x7F` (0..127)

### 3.3 Single Program Dump (`0x10`) & Verified Checksum Formula
Format:
```text
F0 00 20 33 01 <dd> 10 <bb> <ss> [256 data bytes] <cs> F7
```
* **Bank `<bb>`:** `0x00` (Single Edit Buffer), `0x01`..`0x04` (Banks A..D)
* **Program `<ss>`:** `0x00`..`0x7F` (Programs 0..127; or Part Number `0x00..0x0F` / `0x40` when `<bb> == 0x00`)
* **Data Payload:** 256 bytes total (Page A [128 bytes] + Page B [128 bytes])
* **Checksum (`cs`):**
  $$\text{Checksum} = \left(\text{DeviceID} + 0\text{x}10 + \text{Bank} + \text{Program} + \sum_{i=0}^{255} \text{data}[i]\right) \ \& \ 0\text{x}7\text{F}$$
  *(Directly covers bytes 5 through 264 of the SysEx frame).*

### 3.4 Request Commands
* **Single Program Request (`0x30`):** `F0 00 20 33 01 <dd> 30 <bb> <ss> F7`
* **Multi Program Request (`0x31`):** `F0 00 20 33 01 <dd> 31 <bb> <mm> F7`
* **Single Bank Request (`0x32`):** `F0 00 20 33 01 <dd> 32 <bb> F7`
* **Multi Bank Request (`0x33`):** `F0 00 20 33 01 <dd> 33 <bb> F7`
* **Arrangement Request (`0x34`):** `F0 00 20 33 01 <dd> 34 F7`
* **Global Request (`0x35`):** `F0 00 20 33 01 <dd> 35 F7`
* **Total RAM Dump Request (`0x36`):** `F0 00 20 33 01 <dd> 36 F7`
* **Controller Dump Request (`0x37`):** `F0 00 20 33 01 <dd> 37 00 <ss> F7`

---

## 4. Current Status & Progress

- [x] **Phase 1: Domain Crate (`virus-syx`)**
  - [x] Project established in `D:\VirusEditor`
  - [x] Analyzed official manual (`OSR1_Manual_International.pdf`)
  - [x] Verified checksum calculation (`dd + 0x10 + bb + ss + data`)
  - [x] Bipolar7 (`-64..=+63`) and Unipolar7 (`0..=127`) types
  - [x] SysEx framing, header validation, dump & parameter encoders (Page A `0x70`, Page B `0x71`, Page C `0x72`, Part `0x40`)
  - [x] Page A parameter mapping & bitfield decoding
  - [x] Page B parameter mapping (Arp, 6-slot Mod Matrix, EQ, Phaser)
  - [x] Full 256-byte `Patch` model with roundtrip serialization & ASCII name support
  - [x] Code organized into clean `domain` module with root `lib.rs` entry point
- [x] **Phase 2: Git & GitHub Setup**
  - [x] Git repository initialized in `D:\VirusEditor`
  - [x] Linked to [https://github.com/johno4321/VirusEditor](https://github.com/johno4321/VirusEditor)
  - [x] Initial commits pushed to `main` branch
- [x] **Phase 3: MIDI Engine (`virus-midi`)**
  - [x] Workspace member `crates/virus-midi`
  - [x] `midir` wrapper and continuous port polling/reconnection
  - [x] SysEx stream assembler (`stream.rs`) filtering real-time bytes (`0xF8..=0xFF`) and handling chunked packets
  - [x] Asynchronous command & event channels (`crossbeam-channel`)
  - [x] Page A (MIDI CC) and Page B (PolyPressure) incoming message decoding
  - [x] Thread-isolated background worker loop (`engine.rs`)
- [ ] **Phase 4: Database & Librarian (`virus-db`)**
  - [ ] SQLite schema for presets, banks, author, tags, and raw syx BLOBs
  - [ ] Full-text search and category filtering
  - [ ] Standard `.syx` and `.mid` file import/export
- [ ] **Phase 5: GUI Frontend (`virus-app`)**
  - [ ] `eframe` / `egui` app skeleton
  - [ ] Custom rotary knob widget (with bipolar center detent)
  - [ ] Graphical Filter Frequency Response visualizer
  - [ ] ADSR Envelope draggable curve widget
  - [ ] Modulation Matrix grid UI
  - [ ] Preset Librarian table and search bar

---

## 5. Changelog / Work Log
* **2026-11-23**:
  * Initialized workspace in `D:\VirusEditor`.
  * Verified SysEx implementation against official Access Virus Rack manual (`OSR1_Manual_International.pdf`).
  * Reorganized `virus-syx` with dedicated `domain` module (`src/domain/`) and top-level `src/lib.rs`.
  * Linked remote repository to GitHub (`johno4321/VirusEditor`).
  * Implemented `virus-midi` crate: robust SysEx stream assembler, port scanner, and threaded background event loop.

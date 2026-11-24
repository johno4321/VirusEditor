# Instructions for AI coding agents

This file is for any AI agent (Copilot, Gemini, Claude, …) picking up work on VirusEditor: a Rust editor and librarian for the **Access Virus Rack (classic), OS 2.00**.

## Read first, in this order

1. [TODO.md](TODO.md) **"Start here (handoff)"**: the current state, work in flight, and what to do next.
2. [SPEC.md](SPEC.md): the protocol (§3), the parameter map (§4) and the editor rules (§5). **§3.7 explains the live timing rules; read it before touching any MIDI request code.**
3. [crates/virus-cli/README.md](crates/virus-cli/README.md): the live-session runbook.
4. [README.md](README.md): features, layout and how to run.

## Working with the owner (John)

* **He approves every change.** Do the work (investigate, edit, test), but:
  * don't `git commit`, `push`, `merge`, `rebase`, `reset` or `stash`, and don't create or delete branches or tags, unless he asks for that specific action in his current message;
  * don't stage (`git add`) unless asked.
  * When the work is done, show the changed files, a diff summary and a suggested commit message, then wait.
* For any multi-step task, show a Markdown checklist (`- [ ]` / `- [x]`) and update it as you go.
* Explain what you checked, what you found and why you decided what you did. End with: the files changed, how you verified them, and what's left open.
* Ask before destructive actions, and before significant design decisions or when the scope is unclear.
* If you use git worktrees, put them in a sibling folder `VirusEditor.worktrees\<name>`.
* Keep docs current in the same change: tick items in TODO.md (status icons ⬜ 🔄 ✅ ⏸️), and update SPEC.md when behaviour or protocol knowledge changes. Before you finish, update TODO's "Start here" so the next agent can pick up.
* Commit messages are a short imperative summary, often prefixed by area (`CLI: …`, `App: …`, `Docs: …`). See `git log --oneline`.

## Commands (Windows, PowerShell)

```powershell
cargo clippy --workspace --all-targets --locked -- -D warnings   # must be clean (CI runs this)
cargo test --workspace --locked                                   # 322 tests on 24 Nov; must all pass
cargo build --release -p virus-app -p virus-cli                  # GUI and the `virus` CLI
cargo run -p virus-app --release
.\target\release\virus.exe --help
```

* Rust is pinned in `rust-toolchain.toml` (1.100.0; keep it in sync with `.github/workflows/ci.yml`). Windows needs the VS Build Tools C++ workload; Linux needs `libasound2-dev`.
* Behind a proxy that breaks certificate revocation checks: `[http] check-revoke = false` in `%USERPROFILE%\.cargo\config.toml`, or set `CARGO_HTTP_CHECK_REVOKE=false`.
* CI (GitHub Actions) runs clippy, tests and a release build on Windows and Ubuntu. `cargo fmt` isn't applied yet (TODO H11), so don't reformat files you aren't changing.
* **Editing docs from PowerShell 5.1:** `Get-Content`/`Set-Content` mangle UTF-8 (emoji, ✅, →) and add a BOM. Use your editor tools, or .NET `[IO.File]` with `UTF8Encoding($false)`.

## The synth and the hardware

* The Virus is connected to the **owner's laptop** through an iConnectivity mioXC (ports `mioXC MIDI IN` / `mioXC MIDI OUT`). **The Virus is on MIDI channel 11, Device ID 1.** Always pass `--channel 11` to the CLI.
* An agent on another machine can't reach the synth. Prepare the exact commands, explain what to look for, and let the owner run them and paste the output. Record the results in TODO G2 ("Live test results").
* A MIDI port can only be open in one program at a time on Windows: close the GUI before using the CLI, and the other way round.
* **The Virus ignores requests that arrive too soon after other traffic** (SPEC §3.7). Wait for 500 ms of silence, or 2 s before bank requests. The CLI (`wait_quiet`) and the app (`PacedSend`) already do this; keep it that way. Bank sends are paced at 300 ms per dump.
* The studio merges a sequencer into the Virus MIDI IN (SPEC §3.7 "Owner's studio"). The owner stops the transport before bank sends and dump requests.

## Safety rules (never break these)

* **Byte-exact:** a patch is 256 raw bytes, and that's the only source of truth. Never normalise, re-encode or drop undocumented bytes (33 offsets carry data). Unknown enum values are kept (`Unknown(u8)`, shown as `#n`).
* **Never write A0** (Sound Version).
* **Sending a bank overwrites the synth's RAM irrevocably.** Only with the owner's explicit go-ahead, after a backup (`virus get-bank A --out-file backup_A.syx`), with the transport stopped. Banks C/D are ROM, so never send to them.
* Wrong checksums are reported and skipped by default, never silently "fixed".
* Disruptive Page C globals (device ID, channels, dump/memory settings, play mode) need `--force` in the CLI for a reason. Don't bypass that.
* The Access material (manual, addenda, factory sounds) may stay only because the repo is **private**. Never publish it.

## Things that are easy to get wrong

* The factory `.mid` files in `VirusOSR200_INT/…` are **damaged** (LF→CR). Use `testdata/factory/Bank_A.syx` and `Bank_B.syx` as ground truth (SPEC §6). Don't modify `testdata`.
* Assign/definable **destination names are unverified** (Q1). The UI shows raw numbers with greyed hints until the live probe results are in.
* The SysEx bank byte `bb` is A=1 … D=4. The C32 "Select on synth" bank value is assumed 0-based (A=0); this is still unverified (TODO F7).
* Parameter offsets: data byte `n` = A`n`, byte `128+n` = B`n`. The single table `virus_syx::SINGLE_PARAMS` drives the editor, MIDI and decoding; change it there, not in the UI.
* Tests for new behaviour go next to the code (`#[cfg(test)]`), or in `crates/virus-syx/tests/` for factory-data tests. UI logic lives in testable modules (`session.rs`, `library.rs`, `comforts.rs`, …), not in `views/`.

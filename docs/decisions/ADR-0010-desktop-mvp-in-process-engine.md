# ADR-0010: Desktop MVP runs the engine in-process behind a service layer

## Status

Accepted (M6).

## Context

The specification separates the desktop (unprivileged) from a privileged
daemon (`phoinixd`) that owns device access, and wants a session database
for scan persistence. Both are substantial pieces. The first desktop
milestone needs a working, testable application quickly, without locking
the core into GUI dependencies.

## Decision

1. **A service layer first.** `phoinix-session` is the only thing a
   front-end talks to: typed DTOs, background scans with events and
   cancellation, sessions, recovery and previews. It has no GUI
   dependency and is tested end to end on the fixtures.
2. **In-process engine for the MVP.** The Tauri shell calls the service
   layer directly. Physical-disk scans therefore need an elevated
   application process; the UI explains this. The daemon can be added
   later behind the same DTOs: every command already has the shape of a
   restricted IPC message (no arbitrary file reads).
3. **JSON session files** (`.phx`, metadata and evidence only) instead
   of SQLite for now: no native dependency, human-readable, trivially
   portable between the CLI and the desktop. SQLite remains the plan when
   sessions grow to hundreds of thousands of candidates.
4. **No decoding in the process.** Previews hand image bytes to the
   webview (its sandboxed decoder), show validated UTF-8 text, or a hex
   dump. Nothing parses hostile content for display.
5. **Separate Cargo workspace** for the Tauri crate, so the core keeps
   building and testing without WebKitGTK/WebView2 toolchains; CI has a
   dedicated desktop job.

## Consequences

- The desktop is a thin layer; behaviour lives in tested Rust and small
  pure TypeScript modules.
- Elevation is the user's responsibility until `phoinixd` exists.
- Session files can be large for big volumes; that is the trigger for
  the SQLite migration.

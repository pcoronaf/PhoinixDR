# Desktop application (`apps/desktop`)

The desktop is a Tauri 2 shell over a GUI-independent service layer. It
contains no recovery logic: every button maps to a typed command of
`phoinix-session`, and every long operation reports through events.

```text
apps/desktop/src            React + TypeScript + Vite front-end
        │  invoke("start_scan", { request })      typed commands
        │  listen("scan-event")                   typed events
apps/desktop/src-tauri      Tauri 2 shell: commands.rs maps 1:1 onto…
        │
crates/phoinix-session      Workspace: inspect, scans, sessions, recovery, previews
        │
engines and writers         phoinix-fs-*, phoinix-carve, phoinix-recovery
```

## Service layer (`phoinix-session`)

| API | what it does |
|---|---|
| `Workspace::devices` | block devices via `phoinix-device` |
| `Workspace::inspect` | partition table plus the filesystem of every volume (`SourceInfo`) |
| `Workspace::start_scan` | background thread; `ScanEvent`s on a channel; cancellable; partial results kept |
| `ScanSession` | the candidates with their full evidence; saved as JSON `.phx` (metadata only, never content) |
| `Workspace::recover` | `phoinix-recovery` over a freshly opened volume; destination safety; `RecoverEvent`s |
| `Workspace::preview` | reconstructed stream → image bytes (decoded by the webview), validated text, or a hex dump |

The DTOs in `dto.rs` are the IPC contract; `apps/desktop/src/types.ts`
mirrors them field by field. `crates/phoinix-session/tests/service.rs`
exercises the whole layer on the fixtures without a GUI: quick and deep
scans with their event streams, cancellation, session round trips,
recovery of metadata and carved candidates, previews and destination
checks.

## Commands and events

| command | payload | result |
|---|---|---|
| `app_info` | – | version, author, disclaimer, sessions directory, device access, whether the process is elevated, platform |
| `relaunch_elevated` | – | starts an elevated copy through the system prompt (UAC / polkit); whether this instance exits |
| `set_engine_log` | `enabled` | switches the live engine log; `engine-log` events (batches of `LogLine`) while on |
| `list_devices` | – | `DeviceInfo[]` |
| `inspect_source` | `path` | `SourceInfo` |
| `find_partitions` | `path`, `verify?` | `PartitionCandidate[]`; `search-event` while running |
| `start_scan` | `ScanRequest` | starts; `scan-event` (`ScanEvent`) then `scan-complete` |
| `cancel_scan` | – | whether a scan was running |
| `list_sessions` / `load_session` / `current_session` | – / `path` | `SessionSummary` |
| `candidates` | – | `CandidateSummary[]` of the current session |
| `candidate_detail` | `id` | the full `RecoveryCandidate` (evidence, reasons, validation) |
| `preview_candidate` | `id` | `Preview` |
| `check_destination` | `destination` | `DestinationInfo` (same disk, overwrites image, dangerous) |
| `recover` | `RecoverRequest` | `RecoverItem[]`; `recover-event` while running |

Sessions are saved automatically after every scan under the platform app
data directory (`app_info.sessions_dir`).

## Screens

1. **Home**: physical disk, removable device or disk image; recent
   sessions.
2. **Source**: devices with size, bus, medium, accessibility. When a
   device is not accessible, a notice offers **Restart as administrator**
   (`relaunch_elevated`).
3. **Scan**: volume, "Search for lost partitions" (structure search with
   progress; candidates with status, confidence and repair; a chosen
   candidate is scanned as a virtual mount), Quick Scan / Deep Scan (deep
   is forced when no filesystem is recognised), deep-scan options (whole
   volume, file types), content examination.
4. **Scanning**: phase, progress (records; bytes for carving), candidates
   so far, cancel. **Advanced** adds the equivalent `phoinix scan` command
   line and the live engine log (see below), both with copy buttons.
5. **Results**: folder tree, table (name, health badge with confidence on
   hover, size, type, modified, original location, carved tag), search,
   health / source / type filters, sortable columns, multi-select; detail
   panel with the evidence reasons, structure validation and a preview
   tab. **Advanced** (top bar) adds object references, extents, allocation
   state, timestamps and storage, the `phoinix explain` / `phoinix recover`
   command lines for the selected file, and *Copy scan log*.
6. **Recover**: destination picker with the safety check (a destination on
   the source disk is refused unless the expert override is ticked; the
   source image can never be overwritten), folder tree / timestamps / SHA-256
   options, per-file results.

## Privileges

The MVP runs the engine in the application process. Physical disks need
an elevated process (Administrator on Windows, root or a `disk` group on
Linux); disk images do not. The Home screen and the device picker say so
when devices cannot be opened and offer **Restart as administrator**: the
backend (`elevate.rs`, no `unsafe`) starts the current executable through
`Start-Process -Verb RunAs` on Windows (UAC prompt; this instance exits once
the elevated copy runs) or `pkexec` on Linux (this instance stays, since
pkexec cannot report the outcome before the user answers), and `app_info`
reports whether the process is already elevated (`fltmc.exe` exit status,
or the effective uid). The privileged helper (`phoinixd`) of the specification, which
would let the GUI stay unprivileged, is future work; the service layer is
already the boundary it would sit behind.

## Engine log (Advanced mode)

The engine reports what it does through `tracing`; the command line prints
it with `-v`/`-vv`. The desktop installs a second `tracing` layer
(`enginelog.rs`) beside the stderr formatter. It accepts records from
`phoinix*` targets at `debug` or above, only while the Advanced switch is
on (one atomic load per record when off), renders the message and fields
to one line, and hands them to a bounded queue; a thread batches them
(at most every 80 ms or 256 lines) into `engine-log` events. When the
queue is full, records are dropped rather than slowing the engine. The
front-end keeps the last 2000 lines, shows them on the scanning page, and
offers *Copy log*. The session layer logs the scan lifecycle (request,
opened volume, phases, counts, outcome) so the log is structured on every
filesystem. Nothing is written to disk, and the engine never logs
recovered content.

The front-end also renders the command line equivalent to the current
scan and to the selected candidate (`lib/cli.ts`), from the same
`ScanRequest` and candidate reference the commands use, so a user can
reproduce a desktop result with `phoinix`.

## Building and running

```bash
cd apps/desktop
npm ci
npm run tauri dev       # development window (starts Vite and the Rust shell)
npm run tauri build     # installers / bundles under src-tauri/target/release/bundle
```

Prerequisites: Node 22, Rust stable, and on Linux the WebKitGTK
development packages (`libwebkit2gtk-4.1-dev libgtk-3-dev
libayatana-appindicator3-dev librsvg2-dev`); Windows needs the WebView2
runtime (present on Windows 10/11) and the MSVC build tools.

`npm run dev` alone serves the front-end in a browser with a demo data set
(`src/demo.ts`), which is enough for layout work. `npm run typecheck`,
`npm test` (vitest on the pure filter/tree logic) and `npm run build` are
what CI runs, together with `cargo clippy` on `src-tauri`.

The Tauri crate is its own Cargo workspace (`apps/desktop/src-tauri`,
excluded from the root workspace) so that the recovery core builds and
tests without a GUI toolchain.

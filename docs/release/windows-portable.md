# Windows portable release

## Requirement

> **REL-001.** The standard Windows portable release SHALL be distributed as
> a single executable and SHALL require no installation or separately
> installed PHOINIX dependencies. It may rely on operating-system components
> included with supported Windows versions, including WebView2.

Supported Windows versions are Windows 10 (21H2 or later) and Windows 11,
64-bit. Both ship the WebView2 Evergreen Runtime as an operating-system
component.

## How the requirement is met

| artefact | content | dependencies |
|---|---|---|
| `PhoinixDR-windows-x64-portable.exe` | the desktop application: the Tauri shell with the React front-end embedded at compile time and the whole recovery engine linked in | WebView2 (part of Windows), the Visual C++ runtime DLLs that ship with Windows |
| `phoinix-windows-x64.exe` | the command-line application | none beyond Windows itself |

- The front-end assets are compiled into the executable
  (`tauri::generate_context!` embeds `frontendDist`); the executable opens
  no sibling files and needs no `resources` directory. `bundle.resources`
  stays empty in `tauri.conf.json`.
- Nothing is installed. The executable can be run from a download folder,
  a USB stick or a network share. It writes only what the user asks it to
  write: recovered files to the chosen destination, reports to the chosen
  path, and scan sessions to the user's local application-data directory
  (`%LOCALAPPDATA%\org.phoinixdr.desktop`), which is created on first use
  and can be deleted at any time.
- No PHOINIX library, service, driver or runtime is required beside the
  executable. Every filesystem engine and image-container reader is native
  Rust code linked into the binary (ADR-0004, ADR-0013).
- WebView2 is the only runtime the desktop executable needs. When it is
  absent (a Windows 10 installation older than 21H2 with updates blocked),
  the executable reports the missing runtime and points to Microsoft's
  Evergreen Bootstrapper; the optional MSI/NSIS installers, which are not
  the standard portable release, download it automatically
  (`webviewInstallMode: downloadBootstrapper`).
- Reading physical disks needs the same privilege Windows requires from
  any tool that opens `\\.\PhysicalDriveN`: run the executable as
  administrator. Disk images need no elevation.

## Verification

The release workflow (`.github/workflows/release.yml`) builds both
executables on `windows-latest`, runs `phoinix.exe --version` and
`phoinix.exe inspect` on a fixture, and fails if the desktop build produced
anything beside the single executable in its output directory. The
published `SHA256SUMS.txt` lets users verify what they downloaded:

```powershell
Get-FileHash .\PhoinixDR-windows-x64-portable.exe -Algorithm SHA256
```

## Other platforms

Linux releases are a tarball with the two executables; the desktop binary
needs the distribution's WebKitGTK 4.1 packages, which is the equivalent of
the WebView2 dependency. They are convenience builds: Windows is the
platform the portable requirement is written for.

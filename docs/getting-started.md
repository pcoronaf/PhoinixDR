# Getting started

## Download (Windows)

1. Open the [latest release](https://github.com/pcoronaf/PhoinixDR/releases/latest).
2. Download `PhoinixDR-<version>-windows-x64-portable.exe` (desktop
   application; the file name carries the release number, for example
   `PhoinixDR-0.1.2-windows-x64-portable.exe`) and,
   if you want the command line, `phoinix-windows-x64.exe`.
3. Optionally check the download against `SHA256SUMS.txt`:

   ```powershell
   Get-FileHash .\PhoinixDR-<version>-windows-x64-portable.exe -Algorithm SHA256
   ```

4. Run it. Nothing is installed; the executable needs only Windows 10
   (21H2 or later) or Windows 11, whose WebView2 runtime it uses. To scan
   physical disks, run it as administrator. See
   [Windows portable release](release/windows-portable.md).

Windows SmartScreen may warn about an unsigned executable from a new
publisher; the SHA-256 in the release lets you confirm it is the file
published here.

## Download (Linux)

Download `phoinix-linux-x64.tar.gz`, unpack it and run `./phoinix` or
`./phoinix-desktop`. The desktop binary needs the distribution's
WebKitGTK 4.1 packages (`libwebkit2gtk-4.1-0`, `libgtk-3-0`,
`libayatana-appindicator3-1`). Reading devices needs `sudo`.

## Build from source

PhoinixDR is a Cargo workspace on stable Rust (edition 2024, Rust 1.85 or
newer). The desktop application is a Tauri 2 shell with a React/TypeScript
front-end and needs Node.js 22.

### Windows

1. Install [Rust](https://rustup.rs) (the `x86_64-pc-windows-msvc` toolchain)
   and the *Desktop development with C++* workload of Visual Studio Build
   Tools, which provides the MSVC linker.
2. Install [Node.js 22](https://nodejs.org) (LTS).
3. Install the [WebView2 runtime](https://developer.microsoft.com/microsoft-edge/webview2/)
   if `Get-AppxPackage *WebView2*` or Windows 10 21H2 does not already
   provide it.
4. Build:

   ```powershell
   git clone https://github.com/pcoronaf/PhoinixDR.git
   cd PhoinixDR
   cargo build --release                 # target\release\phoinix.exe
   cd apps\desktop
   npm ci
   npx tauri build --no-bundle           # production build of the desktop app
   # apps\desktop\src-tauri\target\release\phoinix-desktop.exe (portable)
   npm run tauri dev                     # development window with hot reload
   ```

   Build the desktop application through the Tauri CLI, as above. A plain
   `cargo build --release` inside `src-tauri` produces a *development*
   binary that expects the Vite dev server on `localhost:1420` and shows
   "localhost refused to connect" when run on its own; the CLI enables the
   production feature that embeds the front-end. `npx tauri build` without
   `--no-bundle` additionally produces MSI/NSIS installers under
   `src-tauri\target\release\bundle`; they are optional.

### Linux (Debian/Ubuntu)

```bash
sudo apt install build-essential curl pkg-config libssl-dev \
    libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev
curl https://sh.rustup.rs -sSf | sh
git clone https://github.com/pcoronaf/PhoinixDR.git
cd PhoinixDR
cargo build --release                    # target/release/phoinix
cd apps/desktop && npm ci && npx tauri build --no-bundle
# apps/desktop/src-tauri/target/release/phoinix-desktop
```

### Tests

```bash
cargo test --workspace        # unit and integration tests on the fixture corpora
cd apps/desktop && npm test   # front-end tests
```

The fixtures under `tests/fixtures` are committed; the scripts under
`tests/generated` rebuild them (they need mkntfs, mkfs.fat, mkfs.exfat,
mke2fs, ewfacquire and qemu-img, and root for loop mounts).

## Two ways to recover

PhoinixDR can work on the device itself or on an image of it. Pick one
before you start:

1. **Directly from the device, in one step.** Start PhoinixDR *as
   administrator* (Windows: right-click, *Run as administrator*; Linux:
   `sudo`), choose *Physical disk* or *Removable device*, scan and
   recover. Fastest; PhoinixDR only ever reads from the device.
2. **From a disk image.** First make an image of the device with an
   imaging tool (FTK Imager or Arsenal Image Mounter on Windows, `dd`
   or `ewfacquire` on Linux) and then open the image file in PhoinixDR
   with *Disk image*. PhoinixDR itself needs no elevation for this; the
   imaging tool does, since it reads the same raw device. This is the
   recommended path for a failing drive, for anything you may need to
   examine again, and for forensic work (E01 hashes are verified and
   reported).

Both paths give the same results on a healthy device. In both, recover to
a different disk than the one you are recovering from.

## First recovery

```bash
phoinix inspect stick.img            # what is on it
phoinix scan stick.img               # deleted files with recovery health
phoinix explain stick.img 64         # why a file scores what it scores
phoinix recover stick.img 64 --output ~/recovered
```

Or open the desktop application, choose *Disk image*, pick the file, keep
*Quick Scan*, and press *Recover* on the rows you want. The
[desktop guide](user-guide/desktop.md) and the
[command-line guide](user-guide/cli.md) cover every option.

## Where to go next

- [FAQ](faq.md): why a file shows 0 %, why a USB stick needs administrator
  rights, what "allocated" means.
- [Health model](recovery/health-model.md): how likelihood and confidence
  are computed.
- [Architecture](architecture/overview.md) and the
  [decision records](decisions/README.md).

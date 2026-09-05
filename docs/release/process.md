# Release process

A release is one run of the *Release* workflow
(`.github/workflows/release.yml`). It builds the Windows portable
executable and the command-line tool on `windows-latest`, the Linux tarball
on `ubuntu-latest`, checks them (see
[Windows portable release](windows-portable.md)), writes `SHA256SUMS.txt`
and publishes a GitHub Release with generated notes.

## Preconditions the workflow enforces

- The workspace version in `Cargo.toml` (which every crate, the desktop
  application and `package.json` share) is the version being released.
- `CHANGELOG.md` has a `## <version>` section.
- The tag `v<version>` does not exist yet, unless an existing tag is being
  rebuilt on purpose.
- The desktop executable embeds its front-end, carries the version in its
  Windows version resource and has nothing beside it in the build output.

## Releasing

There are two ways to start a release; both produce the same result.

**Run the workflow on `main`.** In GitHub, *Actions → Release → Run
workflow*, branch `main`, leave the *tag* field empty. The workflow reads
the version from `Cargo.toml`, builds, creates the tag `v<version>` on the
head commit and publishes the release. This is the path that needs nothing
but a click (or an API call), so an assistant working on the repository
can trigger it after bumping the version and the changelog.

**Push a tag.** `git tag v0.1.2 origin/main && git push origin v0.1.2`
does the same from a terminal; the workflow then checks that the tag
matches `Cargo.toml`.

To rebuild and republish an existing release (for example after fixing
the workflow itself), run the workflow with the *tag* field set to that
tag; the release assets are replaced.

## Preparing a release

1. Bump the version in `Cargo.toml` (`[workspace.package]`),
   `apps/desktop/src-tauri/Cargo.toml`, `apps/desktop/src-tauri/tauri.conf.json`
   and `apps/desktop/package.json` (with `package-lock.json`), and refresh
   both `Cargo.lock` files (`cargo build -p phoinix-cli` and
   `cargo metadata --manifest-path apps/desktop/src-tauri/Cargo.toml`).
2. Add a `## <version>` section to `CHANGELOG.md`.
3. Make sure CI is green on the commit, then start the release as above.

## Published files

| file | content |
|---|---|
| `PhoinixDR-<version>-windows-x64-portable.exe` | desktop application, single executable |
| `phoinix-windows-x64.exe` | command-line application |
| `phoinix-linux-x64.tar.gz` | both executables for Linux x86-64 |
| `SHA256SUMS.txt` | checksums of the files above |

The website's download buttons resolve the current desktop file name
through the GitHub releases API; the other names are stable so that
`releases/latest/download/<file>` links keep working.

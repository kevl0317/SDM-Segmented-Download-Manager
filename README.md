# SDM — Segmented Download Manager

![Rust](https://img.shields.io/badge/rust-stable-orange)
![License](https://img.shields.io/badge/license-MIT-green)
![Platform](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-lightgrey)

A fast download manager. SDM splits each file into byte-range **segments** and
fetches them over several connections at once, with pause/resume and dynamic
work-stealing so no connection sits idle. It's a single native app written in
Rust: a clean desktop window, a catch popup for links from your clipboard and
browser, and a tray icon that keeps it running in the background.

## Features

- **Segmented downloads** over HTTP and HTTPS, 8 connections per file
- **Pause & resume**, including across restarts (progress is checkpointed to a sidecar)
- **Work-stealing** — when a connection finishes, it splits the largest remaining
  segment and helps finish it, so one slow route no longer drags out the tail
- **Modern, minimal window** with dark and light themes; downloads are compact
  rows that expand into a card with a live per-segment bar while they run
- **Catches download links** you copy (event-driven, no polling) and, with the
  optional Chrome/Edge extension, links you click — a small popup asks before
  anything downloads
- **Runs in the system tray** — closing the window keeps SDM running, so
  downloads and catching continue; one SDM at a time
- **Native menu bar** — File, Downloads, View, Options, Help
- **Headless CLI** (`sdm`) with a text progress bar, built from source

## Install

Download the latest **`SDM-Setup-<version>.exe`** from the
[Releases](https://github.com/kevl0317/SDM-Segmented-Download-Manager/releases)
page and run it. It installs SDM per-user (no admin) on 64-bit Windows 10 or 11,
with a Start-Menu shortcut and an uninstaller, and (unless you untick it) the
browser host the extension needs. Installing over 1.x upgrades it in place;
your download list and settings carry over.

Releases will be signed by the [SignPath Foundation](https://signpath.org/),
which provides free code signing for open-source projects, once it accepts SDM.
Until then the installer is unsigned, so Windows SmartScreen may warn on first
run (More info → Run anyway). See the [Code signing policy](CODE_SIGNING_POLICY.md).

## Using SDM

- **Download a link:** paste it into the box at the top (Ctrl+N focuses it) and
  press Download. Files go to your Downloads folder.
- **Catch popup:** when SDM catches a link, a small window shows it with the
  file name, which you can edit. Enter downloads, Esc cancels. The main window
  comes forward only once you download.
- **Copied links:** Options → Catch copied links. By default only links that
  look like downloadable files are caught; Catch all link types drops that filter.
- **Clicked links:** load the browser extension; see [`extension/`](extension/).
- **Manage a download:** click its row to expand it, then Pause / Resume, Open
  (its folder) or Remove. Remove takes the entry off the list; with
  Options → Remove also deletes the file, it deletes the file too (the button
  then reads Delete).
- **Tray:** closing the window hides SDM to the tray. Double-click the tray icon
  to reopen it; right-click for Open SDM / Quit. Turn this off under
  Options → Keep running in the tray when closed.
- **View:** Show completed downloads, Always on top, and Theme (Dark / Light).

Settings and the download list live in `%UserProfile%\.sdm`.

`SDM.exe --register-host` re-registers the browser host, `--unregister-host`
removes it, and `--quit` closes a running SDM (the uninstaller uses it).

## Build it yourself

Needs a stable Rust toolchain (MSVC on Windows). The Windows SDK, if installed,
embeds the app icon and version info; [Inno Setup 6](https://jrsoftware.org/isinfo.php)
builds the installer.

```bash
cargo build --release -p sdm-gui   # -> target/release/sdm-gui.exe (installed as SDM.exe)
cargo build --release -p sdm-cli   # -> target/release/sdm.exe (headless CLI)
iscc SDM.iss                       # -> dist/SDM-Setup-<version>.exe (needs the sdm-gui build)
cargo test --workspace
```

`scripts\build-installer.ps1` does the app and the installer in one step;
`-SignCommand "<signtool command>"` signs a local build with a certificate of
your own.

CLI: `sdm <url> [-o/--out <dir>] [-n/--connections <N>] [-f/--filename <name>]`.
Ctrl+C stops the download.

The desktop app targets Windows. The engine and CLI are plain Rust and should
build on macOS and Linux, but only Windows is tested so far.

### Release builds and signing

Release builds come from [GitHub Actions](.github/workflows/build.yml): pushing
a version tag (`v2.0.2`, matching `SDM.iss`) builds SDM on GitHub's runners and
attaches the installer to that tag's release (a draft, if there isn't one yet).
Pull requests and `main` get the tests and an unsigned installer as a run
artifact.

Signing goes through the [SignPath Foundation](https://signpath.org/), free for
open-source projects; see the [code signing policy](CODE_SIGNING_POLICY.md).
Once the project is approved, the SignPath project needs the GitHub.com trusted
build system, the artifact configurations in [`.github/signpath`](.github/signpath)
(slugs `app` and `installer`), a release signing policy with manual approval,
and a CI user. The repository then needs the secret `SIGNPATH_API_TOKEN` (that
CI user's token) and the variables `SIGNPATH_ORGANIZATION_ID`,
`SIGNPATH_PROJECT_SLUG` and `SIGNPATH_SIGNING_POLICY_SLUG`; from then on, tag
builds sign SDM.exe and the installer, each after you approve it in SignPath.

A signed installer can still get a SmartScreen warning at first. SmartScreen
builds reputation for the signing identity as people install releases;
Microsoft says that takes weeks and hundreds of installs.

## How it works

1. **Probe** — a ranged `GET bytes=0-0` reveals the total size, whether the server
   honors ranges, and the filename.
2. **Plan** — split `[0, size)` into contiguous byte-range segments.
3. **Fetch** — parallel connections (async, on tokio), each writing its range at
   the right file offset.
4. **Work-steal** — an idle connection splits the segment with the most bytes
   left and takes half.
5. **Persist** — progress is checkpointed to a `.sdm` sidecar next to the
   `.sdmdownload` working file; pause stops cleanly and resume continues where
   each segment left off.
6. **Finalize** — the finished file gets its real name.

If a server won't honor ranges or won't reveal the size, SDM falls back to a single
stream.

## Design notes

SDM is a straightforward HTTP client: it uses only the standard `Range` request
mechanism. It installs no drivers, does not intercept system or browser traffic,
and is not affiliated with any commercial download manager. The browser extension
hands links over through the standard native-messaging API, and the app listens
only on a token-guarded loopback socket.

## Project layout

| Path | Role |
|------|------|
| `crates/sdm-core` | download engine (probe, segmentation, work-stealing, resume), queue persistence, loopback IPC, link and filename helpers |
| `crates/sdm-gui` | the desktop app (iced): window, catch popup, tray, menus, clipboard catching; also the browser's native-messaging host |
| `crates/sdm-cli` | headless CLI (`sdm`) |
| `extension/` | Chrome/Edge extension that catches clicked download links |
| `SDM.iss` | Inno Setup installer script |

## Contributing

Issues and pull requests are welcome. `cargo test --workspace` runs the tests for
the engine (segment planning, work-stealing), removing downloads, the IPC wire
format and the link helpers.

## License

[MIT](LICENSE) © 2026 Kevin Liu

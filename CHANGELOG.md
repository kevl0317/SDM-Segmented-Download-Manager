# Changelog

## Unreleased

### Changed
- **The installer asks before connecting the browser extension.** Registering
  the native-messaging host the Chrome/Edge extension needs is now a
  setup option (on by default) instead of happening silently.
- **Release builds come from GitHub Actions**, ready to be signed through the
  SignPath Foundation once the project is approved (see the code signing
  policy).
- The app's version info now matches the installer's (product name "SDM").

## v2.0.1 — 2026-10-28

### Fixed
- **A white line under the dark menu bar** after switching from the light theme.
- **The download card's buttons** could be squeezed into slivers by a long
  message. They now keep their size, and the details stay on one line.
- **Download errors read plainly**, e.g. "401 Unauthorized: the link may have
  expired or need a sign-in", instead of the raw error with the whole link in
  it. A download the server refuses (401, 403, 404…) now fails right away
  instead of retrying first.

## v2.0.0 — 2026-10-28

A complete rewrite in Rust: one native executable with no Python runtime, and a
5 MB installer. Segmented downloads, pause/resume, clipboard and browser
catching, and the tray work as in 1.3.0; what's different is below.

### Added
- **Dark and light themes** (View → Theme). The title bar and the menu bar
  switch along with the window.
- **Compact download rows.** A download is one slim row; it expands into a card
  with the live per-segment bar while it runs, or when you click it.
- **Native menu bar** — File, Downloads, View, Options, Help — and an About window.
- **One SDM at a time.** Launching SDM while it's running in the tray brings
  back its window instead of starting a second copy.
- **Options → "Remove also deletes the file".** Off by default, so Remove only
  takes the entry off the list; turn it on to delete the downloaded file (or a
  partial download's data) too.

### Changed
- **The catch popup comes to the front once** and steps aside when you switch to
  another window, instead of staying on top. Long links fit on one line.
- **Uninstalling closes a running SDM first**, so no files are left behind.
- **Installing over 1.x upgrades it in place**; the download list and settings
  carry over.
- **Downloads are always saved to your Downloads folder**; the popup has no
  "Save to" field yet.

### Removed
- **FTP downloads** — 2.0.0 downloads over HTTP and HTTPS.
- **The `sdm.pyz` zipapp.** The headless CLI now builds from source
  (`cargo build --release -p sdm-cli`).

## v1.3.0 — 2026-10-28

### Added
- **Browser integration (Chrome/Edge).** An optional Manifest V3 extension
  catches links you click (or right-click → "Download with SDM") and hands them
  to SDM via a native-messaging host (SDM.exe itself, launched by the browser),
  forwarding cookies, User-Agent and referrer so authenticated downloads work.
  It reaches the running app over a token-guarded loopback socket, starting SDM
  if needed. `SDM.exe --register-host` wires up the host; the installer does it
  automatically. See `extension/`.

### Changed
- The clipboard / browser catch popup now centers on the primary screen.

## v1.2.1 — 2026-10-27

### Changed
- **Caught-link popup is standalone now (like IDM).** Copying a download link
  shows only a small popup on top of everything; the main window stays where it
  is (even hidden in the tray). The full GUI comes to the front only after you
  click Download.

### Fixed
- **Installer shows "SDM" in Add/Remove Programs** (not "SDM 1.2.0") — the
  version stays in its own column.

## v1.2.0 — 2026-10-27

### Added
- **System tray / run in background (Windows).** Closing the window now hides
  SDM to the notification area instead of quitting, so downloads and clipboard
  monitoring keep running. Double-click the tray icon to reopen; right-click for
  Open / Quit. Toggle it under File → "Keep running in the tray when closed";
  quit for real from File → Exit or the tray menu.

### Fixed
- **A caught clipboard link now brings the window to the front.** Previously the
  Add dialog could open behind other windows; now SDM is raised to the
  foreground (and restored from the tray) when it catches a link.

## v1.1.1 — 2026-10-27

### Changed
- **Clipboard monitoring is now event-driven on Windows.** Instead of polling,
  a message-only window subscribes with `AddClipboardFormatListener`, so Windows
  notifies SDM the instant the clipboard changes and the clipboard is read only
  on a real change (the modern equivalent of the clipboard-viewer chain). Falls
  back to a light poll on non-Windows or if the listener can't be set up.

## v1.1.0 — 2026-10-27

### Added
- **Clipboard monitoring** — with it enabled (File → "Monitor clipboard for
  links"), copying a download link pops up the Add dialog with the URL filled
  in. Filters to downloadable file types by default; "Monitor all link types"
  drops the filter. Links SDM itself copies are ignored, and it only runs when
  you turn it on.
- **Windows installer** — `SDM-Setup` (built with Inno Setup) installs SDM
  per-user with a Start-Menu shortcut, an optional desktop shortcut, and an
  uninstaller — no admin required. The portable `SDM.exe` and the `sdm.pyz`
  zipapp are still offered too.

## v1.0.0 — 2026-10-27

First public release.

### Features
- **Segmented downloads** over HTTP/HTTPS/FTP with a configurable number of connections
- **Pause & resume**, including across app restarts (progress is checkpointed to a sidecar)
- **Dynamic work-stealing** — a finished connection splits the largest remaining segment
  and helps finish it, so one slow route no longer drags out the tail
- **DPI-aware Tkinter GUI** — download queue, live per-segment progress view, speed/ETA,
  right-click menu, and clipboard URL capture
- **Headless CLI** with a text progress bar
- **Packaging** — a standalone Windows `SDM.exe`, a dependency-free `sdm.pyz` zipapp,
  and a double-click launcher
- **Zero third-party runtime dependencies** (Python standard library only)

### Notes
- `SDM.exe` is unsigned, so Windows SmartScreen may warn on first run
  (More info → Run anyway). The onefile build self-extracts on first launch,
  so the first start is a little slower than later ones.

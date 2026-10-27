# Changelog

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

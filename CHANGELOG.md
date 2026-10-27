# Changelog

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

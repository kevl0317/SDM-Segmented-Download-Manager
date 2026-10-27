# SDM — Segmented Download Manager

![Python](https://img.shields.io/badge/python-3.9%2B-blue)
![License](https://img.shields.io/badge/license-MIT-green)
![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey)
![Dependencies](https://img.shields.io/badge/dependencies-none%20(stdlib)-brightgreen)

A fast, dependency-free download manager. SDM splits each file into byte-range
**segments** and fetches them over several connections at once, with pause/resume
and dynamic work-stealing so no connection sits idle. It ships with a DPI-aware
desktop GUI and a headless CLI.

**Pure Python standard library — no pip installs, no native dependencies.**

## Features

- **Segmented downloads** over HTTP/HTTPS/FTP with a configurable number of connections
- **Pause & resume**, including across app restarts (progress is checkpointed to a sidecar)
- **Work-stealing** — when a connection finishes, it splits the largest remaining
  segment and helps finish it, so one slow route no longer drags out the tail
- **DPI-aware Tkinter GUI** — download queue, a live per-segment progress view,
  speed/ETA, right-click menu, and clipboard URL capture
- **Clipboard monitoring** — optionally offer to download a link the moment you
  copy it (opt-in; filters to downloadable file types by default)
- **Headless CLI** with a text progress bar
- **Single-file packaging** — a `.pyz` zipapp, or a standalone Windows `.exe`
- **Zero third-party dependencies**

## Quick start

Requires Python 3.9+ with Tkinter (standard on python.org and conda builds).

```bash
git clone https://github.com/kevl0317/SDM.git
cd SDM

python -m sdm                                   # desktop GUI
python -m sdm "https://example.com/big.iso" -n 8    # CLI, 8 connections
```

CLI options: `-o/--out <dir>`, `-n/--connections <N>`, `-f/--filename <name>`.

## Run a packaged build

```bash
python dist/sdm.pyz          # GUI, from the zipapp (any Python 3.9+ with Tkinter)
```

On Windows you can also run the standalone `SDM.exe` (needs no Python installed),
or double-click `SDM.bat`, which builds and launches the app. Or install it with
**`SDM-Setup.exe`** from the [Releases](https://github.com/kevl0317/SDM/releases)
page — a per-user install (no admin) with a Start-Menu shortcut and an uninstaller.

## Build it yourself

```bash
python build.py        # -> dist/sdm.pyz   (stdlib zipapp, no dependencies)
build_exe.bat          # -> dist/SDM.exe   (Windows standalone, via PyInstaller)
iscc SDM.iss           # -> dist/SDM-Setup-1.1.1.exe  (Windows installer, via Inno Setup)
```

## How it works

1. **Probe** — a ranged `GET bytes=0-0` reveals the total size, whether the server
   honors ranges, and the filename.
2. **Plan** — split `[0, size)` into contiguous byte-range segments.
3. **Fetch** — a pool of worker threads, each writing its range at the right file offset.
4. **Work-steal** — idle workers split and help the segment with the most bytes left.
5. **Persist** — progress is checkpointed; pause stops cleanly and resume continues
   from where each segment left off.
6. **Finalize** — segments reassemble into the finished file.

If a server won't honor ranges or won't reveal the size, SDM falls back to a single
stream. See [`sdm/README.md`](sdm/README.md) for the detailed design.

## Design notes

SDM is a straightforward HTTP client: it uses only the standard `Range` request
mechanism. It installs no drivers, does not intercept system or browser traffic,
and is not affiliated with any commercial download manager.

## Project layout

| Path | Role |
|------|------|
| `sdm/engine.py` | download engine — probe, segmentation, work-stealing, resume |
| `sdm/manager.py` | job list + queue persistence |
| `sdm/gui.py` | DPI-aware Tkinter GUI |
| `sdm/cli.py` | headless CLI |
| `sdm/__main__.py` | entry point (GUI by default, CLI when given a URL) |
| `build.py`, `run_app.py`, `build_exe.bat`, `SDM.iss`, `SDM.bat` | packaging, installer & launchers |

## Contributing

Issues and pull requests are welcome. The codebase is small, stdlib-only, and
has a self-contained test for the engine (segmented download, pause/resume,
work-stealing, and the no-range fallback).

## License

[MIT](LICENSE) © 2026 Kevin Liu

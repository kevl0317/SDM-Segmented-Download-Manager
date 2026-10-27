# SDM — Segmented Download Manager

A small, **original** download manager written to demonstrate the legitimately
interesting core of a tool in this genre: **parallel, segmented HTTP(S)/FTP
downloading with pause/resume**, plus a DPI-aware desktop GUI. Pure Python
standard library — no pip installs, no native code.

This is **not** a clone of any commercial product, and it deliberately does
**not** include the invasive mechanisms such products use (kernel traffic
redirection, browser download-hijacking, AV-evasion lists). It only uses the
HTTP `Range` mechanism any client is entitled to use.

## Run

```bash
# Desktop GUI
python -m sdm

# Headless, single URL
python -m sdm "https://example.com/big.iso" -o "C:\Users\me\Downloads" -n 8
```

(Use the Python that has Tkinter — standard on Windows python.org / conda builds.)

## Package it

Two options, from the project root:

```bash
# 1) Zero-dependency zipapp (stdlib). Produces dist/sdm.pyz
python build.py
python dist/sdm.pyz            # runs the GUI with any Python 3.9+ + Tkinter

# 2) Standalone Windows .exe (no Python needed to run the result)
build_exe.bat                 # installs PyInstaller, emits dist/SDM.exe
```

`SDM.bat` is a double-click launcher: it builds `sdm.pyz` on first run and opens
the GUI with `pythonw` (no console window).

## How it works

1. **Probe** — one ranged `GET bytes=0-0`. The response tells us the total size
   (`Content-Range`), whether the server honors ranges (`206` / `Accept-Ranges`),
   and the filename (`Content-Disposition`, else the URL).
2. **Plan** — split `[0, size)` into *N* contiguous byte-range segments (capped so
   each segment is ≥ 1 MiB; small files stay single-connection).
3. **Fetch** — a pool of worker threads, each with its **own file handle**,
   `seek`ing to its offset and writing its range. A per-segment cap guards
   against servers that ignore the range end.
4. **Work-stealing** — when a worker finishes its segment, it splits the back
   half of the segment with the most bytes left and helps finish it. Idle
   connections keep working until the whole file is done, so one slow route
   no longer drags out the tail. (Overlap at a split point is harmless — the
   same file offsets receive the same bytes.)
5. **Persist** — progress is written to a `<file>.sdmdownload.sdm` JSON sidecar.
   **Pause** sets a stop event; workers flush and exit. **Resume** reopens the
   pre-allocated file (no truncation) and each segment continues from
   `start + downloaded`.
6. **Finalize** — when every segment is complete, the working file is renamed to
   the final name (uniquified on collision) and the sidecar is deleted.

If the server won't do ranges or won't reveal the size, it falls back to a
single stream (no resume).

## Layout

| File | Role |
|---|---|
| `sdm/engine.py` | `Download` / `Segment` — probe, planning, work-stealing workers, resume, finalize |
| `sdm/manager.py` | `Manager` — job list + queue persistence (`~/.sdm/queue.json`) |
| `sdm/gui.py` | DPI-aware Tkinter UI: queue table, per-segment canvas, pause/resume/remove |
| `sdm/clipboard.py` | event-driven clipboard link monitor (auto-popup when you copy a download URL) |
| `sdm/tray.py` | Windows system-tray icon so SDM can run in the background after the window is closed |
| `sdm/cli.py` | headless download with a text progress bar |
| `sdm/__main__.py` | entry point (GUI by default, CLI when given a URL) |
| `build.py` | builds `dist/sdm.pyz` (stdlib zipapp) |
| `run_app.py`, `build_exe.bat` | PyInstaller entry + build script for a standalone `.exe` |
| `SDM.bat` | double-click launcher |

## Performance notes

- **Work-stealing** (above) is the main throughput win: all connections stay
  busy to the end instead of idling once their own segment finishes.
- Reads are 256 KiB — fewer syscalls than tiny reads, still small enough that
  **Pause** reacts promptly.
- Threads + blocking `urllib` are the right tool here: downloads are I/O-bound
  and the GIL is released during socket and disk I/O.

## Known limits

- Resume depends on the server honoring `Range` on the resumed request.
- No bandwidth throttling, scheduler, or checksum verification yet.
- Cross-restart resume relies on the sidecar + partial file still being present.

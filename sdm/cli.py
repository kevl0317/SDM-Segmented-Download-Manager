"""Headless CLI: download a URL with a text progress bar."""
from __future__ import annotations

import argparse
import sys
import time

from .engine import Download, Status
from .manager import default_download_dir


def human_bytes(n) -> str:
    if n is None:
        return "?"
    n = float(n)
    for unit in ("B", "KB", "MB", "GB", "TB"):
        if n < 1024 or unit == "TB":
            return f"{n:.1f}{unit}" if unit != "B" else f"{int(n)}B"
        n /= 1024
    return f"{n:.1f}TB"


def human_time(s) -> str:
    if s is None:
        return "--:--"
    s = int(s)
    h, s = divmod(s, 3600)
    m, s = divmod(s, 60)
    return f"{h}:{m:02d}:{s:02d}" if h else f"{m:02d}:{s:02d}"


def _render(d: Download) -> None:
    pct = d.progress()
    done = human_bytes(d.downloaded())
    total = human_bytes(d.size)
    speed = human_bytes(d.speed) + "/s"
    if pct is None:
        bar = f"{done} ({d.connections} conns, size unknown)"
    else:
        width = 30
        filled = int(width * pct)
        bar = "[" + "#" * filled + "-" * (width - filled) + f"] {pct*100:5.1f}%"
    line = f"\r{bar}  {done}/{total}  {speed:>12}  ETA {human_time(d.eta())}   "
    sys.stdout.write(line)
    sys.stdout.flush()


def main(argv=None) -> int:
    p = argparse.ArgumentParser(prog="sdm", description="Segmented download manager (CLI).")
    p.add_argument("url", help="URL to download")
    p.add_argument("-o", "--out", default=default_download_dir(), help="destination directory")
    p.add_argument("-n", "--connections", type=int, default=8, help="parallel connections")
    p.add_argument("-f", "--filename", default=None, help="override output filename")
    args = p.parse_args(argv)

    d = Download(args.url, args.out, args.filename, args.connections)
    print(f"URL: {args.url}")
    d.start()

    try:
        while d.is_active() or d.status is Status.RUNNING:
            if d.status is Status.RUNNING:
                _render(d)
            if not d.is_active():
                break
            time.sleep(0.25)
    except KeyboardInterrupt:
        print("\nPausing...")
        d.pause()
        d.wait(10)
        print(f"Paused at {human_bytes(d.downloaded())}. Re-run to resume.")
        return 130

    d.wait()
    _render(d)
    print()
    if d.status is Status.COMPLETE:
        print(f"Done -> {d.final_path}")
        print(f"Ranges: {'yes' if d.supports_ranges else 'no'}  "
              f"Segments: {len(d.segments) or 1}  Size: {human_bytes(d.size)}")
        return 0
    print(f"Failed: {d.error or d.status.value}")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())

"""Segmented download engine (stdlib only).

An original, clean-room download core: it splits a file into byte-range
segments, fetches them over several parallel HTTP(S) connections, writes each
segment at its correct offset, and supports pause / resume via a small JSON
sidecar. No kernel hooks, no browser/traffic interception -- just the Range
mechanism that any HTTP client is entitled to use.
"""
from __future__ import annotations

import json
import os
import re
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from collections import deque
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from enum import Enum
from typing import Optional

CHUNK = 256 * 1024            # read size; big enough to cut syscalls, small enough to stay pause-responsive
MIN_SEGMENT = 1 << 20          # don't bother parallelizing below 1 MiB/segment
SPLIT_MIN = 2 << 20           # a finished worker only steals a remainder this large or bigger
MAX_RETRIES = 5
WORKING_SUFFIX = ".sdmdownload"
STATE_SUFFIX = ".sdm"
# A browser-like UA maximizes server compatibility; override per download.
DEFAULT_UA = ("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
              "(KHTML, like Gecko) Chrome/124.0 Safari/537.36 SDM/1.0")


class Status(str, Enum):
    QUEUED = "queued"
    PROBING = "probing"
    RUNNING = "running"
    PAUSED = "paused"
    COMPLETE = "complete"
    ERROR = "error"


@dataclass
class Segment:
    index: int
    start: int
    end: int            # inclusive
    downloaded: int = 0
    active: bool = False   # a worker is currently fetching this segment

    @property
    def length(self) -> int:
        return self.end - self.start + 1

    @property
    def done(self) -> bool:
        return self.downloaded >= self.length


def default_headers(ua: str = DEFAULT_UA) -> dict:
    # Accept-Encoding: identity is essential -- a gzip'd range would break
    # byte offsets. We want raw bytes at known positions.
    return {"User-Agent": ua, "Accept": "*/*", "Accept-Encoding": "identity"}


def sanitize_filename(name: str) -> str:
    name = urllib.parse.unquote(name or "").strip().strip(".")
    name = re.sub(r'[\\/:*?"<>|\x00-\x1f]', "_", name)
    return name or "download"


def _parse_content_disposition(value: str) -> Optional[str]:
    if not value:
        return None
    m = re.search(r"filename\*\s*=\s*[^']*''([^;]+)", value, re.I)
    if m:
        return sanitize_filename(m.group(1))
    m = re.search(r'filename\s*=\s*"([^"]+)"', value, re.I)
    if m:
        return sanitize_filename(m.group(1))
    m = re.search(r"filename\s*=\s*([^;]+)", value, re.I)
    if m:
        return sanitize_filename(m.group(1))
    return None


def filename_from_url(url: str) -> str:
    path = urllib.parse.urlparse(url).path
    base = os.path.basename(path)
    return sanitize_filename(base) if base else "download"


class Download:
    """One download job. Owns its own worker threads while running."""

    def __init__(self, url: str, dest_dir: str, filename: Optional[str] = None,
                 connections: int = 8, headers: Optional[dict] = None,
                 timeout: float = 30.0):
        self.url = url
        self.dest_dir = os.path.abspath(dest_dir)
        self.filename = sanitize_filename(filename) if filename else None
        self.connections = max(1, int(connections))
        self.headers = headers or default_headers()
        self.timeout = timeout

        self.size: Optional[int] = None
        self.supports_ranges = False
        self.segments: list[Segment] = []
        self.status = Status.QUEUED
        self.error: Optional[str] = None

        self.speed = 0.0                     # bytes/sec, updated by sampler
        self._samples: deque = deque(maxlen=8)
        self._stop = threading.Event()
        self._lock = threading.RLock()
        self._orchestrator: Optional[threading.Thread] = None
        self._stream_downloaded = 0          # single-connection fallback counter
        self._probed = False                 # probe/plan done once, kept across resumes

    # ---- paths -----------------------------------------------------------
    @property
    def _resolved_name(self) -> str:
        return self.filename or filename_from_url(self.url)

    @property
    def final_path(self) -> str:
        return os.path.join(self.dest_dir, self._resolved_name)

    @property
    def working_path(self) -> str:
        return self.final_path + WORKING_SUFFIX

    @property
    def state_path(self) -> str:
        return self.working_path + STATE_SUFFIX

    # ---- progress --------------------------------------------------------
    def downloaded(self) -> int:
        with self._lock:
            if self.segments:
                return sum(s.downloaded for s in self.segments)
            return self._stream_downloaded

    def progress(self) -> Optional[float]:
        if not self.size:
            return None
        return min(1.0, self.downloaded() / self.size)

    def eta(self) -> Optional[float]:
        if not self.size or self.speed <= 0:
            return None
        remaining = self.size - self.downloaded()
        return remaining / self.speed if remaining > 0 else 0.0

    def is_active(self) -> bool:
        return self.status in (Status.QUEUED, Status.PROBING, Status.RUNNING)

    # ---- control ---------------------------------------------------------
    def start(self) -> None:
        with self._lock:
            if self.status is Status.COMPLETE:
                return
            if self._orchestrator is not None and self._orchestrator.is_alive():
                return
            self._stop.clear()
            self.error = None
            self.status = Status.QUEUED
            self._orchestrator = threading.Thread(target=self._run, daemon=True)
            self._orchestrator.start()

    def pause(self) -> None:
        self._stop.set()

    def wait(self, timeout: Optional[float] = None) -> None:
        t = self._orchestrator
        if t:
            t.join(timeout)

    # ---- orchestration ---------------------------------------------------
    def _run(self) -> None:
        try:
            if not self._probed:
                self.status = Status.PROBING
                self._probe()
                self._plan_segments()
                self._probed = True

            if self.size == 0:                       # empty file edge case
                self._preallocate()
                self._finalize()
                return

            self.status = Status.RUNNING
            sampler = threading.Thread(target=self._sample_speed, daemon=True)
            sampler.start()

            if self.supports_ranges and self.size and self.segments:
                self._preallocate()
                with self._lock:
                    for s in self.segments:
                        s.active = False             # clear stale flags before (re)starting
                if any(not s.done for s in self.segments):
                    with ThreadPoolExecutor(max_workers=self.connections) as ex:
                        futures = [ex.submit(self._run_worker) for _ in range(self.connections)]
                        for f in futures:
                            f.result()               # surface worker crashes
            else:
                self._single_stream()

            # Decide final state.
            if self.status is Status.ERROR:
                self._save_state()
            elif self._stop.is_set():
                self.status = Status.PAUSED
                self._save_state()
            elif self._all_done():
                self._finalize()
            else:
                self.status = Status.PAUSED
                self._save_state()
        except Exception as exc:                     # noqa: BLE001 - report any failure
            self.error = f"{type(exc).__name__}: {exc}"
            self.status = Status.ERROR
            self._save_state()
        finally:
            self.speed = 0.0

    def _all_done(self) -> bool:
        if self.segments:
            return all(s.done for s in self.segments)
        return self.size is not None and self._stream_downloaded >= self.size

    def _fail(self, exc: Exception) -> None:
        with self._lock:
            if self.status is not Status.ERROR:
                self.error = f"{type(exc).__name__}: {exc}"
                self.status = Status.ERROR
        self._stop.set()

    # ---- network ---------------------------------------------------------
    def _open(self, extra_headers: Optional[dict] = None):
        headers = dict(self.headers)
        if extra_headers:
            headers.update(extra_headers)
        req = urllib.request.Request(self.url, headers=headers)
        return urllib.request.urlopen(req, timeout=self.timeout)

    def _probe(self) -> None:
        """One ranged GET to learn size, range support, and the filename."""
        resp = self._open({"Range": "bytes=0-0"})
        try:
            self.url = resp.geturl()                 # follow redirects once
            status = getattr(resp, "status", resp.getcode())
            headers = resp.headers
            if self.filename is None:
                cd = headers.get("Content-Disposition")
                self.filename = _parse_content_disposition(cd) or filename_from_url(self.url)

            cr = headers.get("Content-Range")
            if status == 206 and cr:
                self.supports_ranges = True
                m = re.search(r"/(\d+)\s*$", cr)
                if m:
                    self.size = int(m.group(1))
            else:
                self.supports_ranges = (headers.get("Accept-Ranges", "").lower() == "bytes")
                cl = headers.get("Content-Length")
                if status == 200 and cl is not None:
                    self.size = int(cl)
        finally:
            resp.close()

    def _plan_segments(self) -> None:
        if not (self.supports_ranges and self.size and self.size > 0):
            self.segments = []
            return
        n = max(1, min(self.connections, (self.size + MIN_SEGMENT - 1) // MIN_SEGMENT))
        base = self.size // n
        segments, offset = [], 0
        for i in range(n):
            start = offset
            end = (self.size - 1) if i == n - 1 else (start + base - 1)
            segments.append(Segment(i, start, end))
            offset = end + 1
        self.segments = segments

    def _preallocate(self) -> None:
        os.makedirs(self.dest_dir, exist_ok=True)
        p = self.working_path
        if os.path.exists(p) and self.size is not None and os.path.getsize(p) == self.size:
            return                                   # resume: keep existing bytes
        with open(p, "wb") as f:
            if self.size:
                f.truncate(self.size)

    def _claim_segment(self) -> Optional[Segment]:
        """Hand a worker its next unit of work, under the lock.

        First any segment nobody is downloading yet; otherwise steal the back
        half of the in-progress segment with the most bytes left, so idle
        connections keep helping until the whole file is done. Returns None
        when there is nothing left worth doing.
        """
        with self._lock:
            for s in self.segments:
                if not s.active and not s.done:
                    s.active = True
                    return s
            victim, best = None, 0
            for s in self.segments:
                if s.active and not s.done:
                    remaining = s.end - (s.start + s.downloaded) + 1
                    if remaining > best:
                        victim, best = s, remaining
            if victim is not None and best >= SPLIT_MIN:
                cursor = victim.start + victim.downloaded
                split_at = cursor + (victim.end - cursor + 1) // 2
                new = Segment(len(self.segments), split_at, victim.end, 0, active=True)
                victim.end = split_at - 1            # shrink the victim; its worker stops here
                self.segments.append(new)
                return new
            return None

    def _run_worker(self) -> None:
        """Pull segments (claimed or stolen) until the work is exhausted."""
        try:
            with open(self.working_path, "r+b") as f:
                while not self._stop.is_set():
                    seg = self._claim_segment()
                    if seg is None:
                        return
                    try:
                        self._download_segment(f, seg)
                    finally:
                        with self._lock:
                            seg.active = False
        except Exception as exc:                     # noqa: BLE001
            self._fail(exc)

    def _download_segment(self, f, seg: Segment) -> None:
        retries = 0
        while not self._stop.is_set():
            with self._lock:
                pos, end = seg.start + seg.downloaded, seg.end
            if pos > end:                            # complete (possibly after a steal shrank it)
                return
            try:
                resp = self._open({"Range": f"bytes={pos}-{end}"})
            except Exception:                        # noqa: BLE001
                retries += 1
                if retries > MAX_RETRIES:
                    raise
                time.sleep(min(2 ** retries, 10))
                continue
            try:
                f.seek(pos)
                while not self._stop.is_set():
                    chunk = resp.read(CHUNK)
                    if not chunk:
                        break
                    with self._lock:
                        remaining = seg.end - (seg.start + seg.downloaded) + 1
                    if remaining <= 0:               # a steal shrank this segment past here
                        break
                    if len(chunk) > remaining:       # server ignored the range end, or we were split
                        chunk = chunk[:remaining]
                    f.write(chunk)
                    with self._lock:
                        seg.downloaded += len(chunk)
                    if seg.start + seg.downloaded > seg.end:
                        break
            finally:
                resp.close()
            retries = 0

    def _single_stream(self) -> None:
        """Fallback when the server won't do ranges or won't reveal size."""
        os.makedirs(self.dest_dir, exist_ok=True)
        self._stream_downloaded = 0
        try:
            resp = self._open()
            with open(self.working_path, "wb") as f:
                while not self._stop.is_set():
                    chunk = resp.read(CHUNK)
                    if not chunk:
                        break
                    f.write(chunk)
                    with self._lock:
                        self._stream_downloaded += len(chunk)
            resp.close()
            if not self._stop.is_set():
                if self.size is None:
                    self.size = self._stream_downloaded
        except Exception as exc:                     # noqa: BLE001
            self._fail(exc)

    # ---- finish / persistence -------------------------------------------
    def _finalize(self) -> None:
        final = self.final_path
        if os.path.exists(final):
            root, ext = os.path.splitext(final)
            i = 1
            while os.path.exists(f"{root} ({i}){ext}"):
                i += 1
            final = f"{root} ({i}){ext}"
        os.replace(self.working_path, final)
        if os.path.exists(self.state_path):
            os.remove(self.state_path)
        self.status = Status.COMPLETE

    def _save_state(self) -> None:
        try:
            data = {
                "url": self.url,
                "dest_dir": self.dest_dir,
                "filename": self._resolved_name,
                "connections": self.connections,
                "size": self.size,
                "supports_ranges": self.supports_ranges,
                "segments": [[s.start, s.end, s.downloaded] for s in self.segments],
            }
            tmp = self.state_path + ".tmp"
            with open(tmp, "w", encoding="utf-8") as f:
                json.dump(data, f)
            os.replace(tmp, self.state_path)
        except OSError:
            pass

    def _sample_speed(self) -> None:
        self._samples.clear()
        while self.status is Status.RUNNING:
            self._samples.append((time.monotonic(), self.downloaded()))
            if len(self._samples) >= 2:
                (t0, b0), (t1, b1) = self._samples[0], self._samples[-1]
                dt = t1 - t0
                self.speed = (b1 - b0) / dt if dt > 0 else 0.0
            time.sleep(0.5)

    @classmethod
    def from_state(cls, state_path: str) -> "Download":
        with open(state_path, "r", encoding="utf-8") as f:
            data = json.load(f)
        d = cls(data["url"], data["dest_dir"], data["filename"],
                connections=data.get("connections", 8))
        d.size = data.get("size")
        d.supports_ranges = data.get("supports_ranges", False)
        d.segments = [Segment(i, s, e, dl)
                      for i, (s, e, dl) in enumerate(data.get("segments", []))]
        d._probed = True
        d.status = Status.COMPLETE if d._all_done() and not d.segments else Status.PAUSED
        if d.segments and all(s.done for s in d.segments):
            d.status = Status.PAUSED   # still needs finalize; resume will do it
        return d

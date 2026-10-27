"""Download manager: owns a list of Download jobs and persists the queue."""
from __future__ import annotations

import json
import os
from typing import Optional

from .engine import Download, Status


def default_download_dir() -> str:
    return os.path.join(os.path.expanduser("~"), "Downloads")


def config_dir() -> str:
    d = os.path.join(os.path.expanduser("~"), ".sdm")
    os.makedirs(d, exist_ok=True)
    return d


class Manager:
    def __init__(self, default_dir: Optional[str] = None):
        self.default_dir = default_dir or default_download_dir()
        self.queue_path = os.path.join(config_dir(), "queue.json")
        self.downloads: list[Download] = []

    def add(self, url: str, dest_dir: Optional[str] = None,
            filename: Optional[str] = None, connections: int = 8) -> Download:
        d = Download(url, dest_dir or self.default_dir, filename, connections)
        self.downloads.append(d)
        self.save_queue()
        return d

    def remove(self, d: Download, delete_files: bool = False) -> None:
        d.pause()
        if d in self.downloads:
            self.downloads.remove(d)
        if delete_files:
            for p in (d.working_path, d.state_path):
                try:
                    if os.path.exists(p):
                        os.remove(p)
                except OSError:
                    pass
        self.save_queue()

    def pause_all(self) -> None:
        for d in self.downloads:
            d.pause()

    def save_queue(self) -> None:
        entries = []
        for d in self.downloads:
            entries.append({
                "url": d.url,
                "dest_dir": d.dest_dir,
                "filename": d._resolved_name,
                "connections": d.connections,
                "status": d.status.value,
                "state_path": d.state_path,
                "final_path": d.final_path,
                "size": d.size,
            })
        try:
            tmp = self.queue_path + ".tmp"
            with open(tmp, "w", encoding="utf-8") as f:
                json.dump(entries, f, indent=1)
            os.replace(tmp, self.queue_path)
        except OSError:
            pass

    def load_queue(self) -> None:
        if not os.path.exists(self.queue_path):
            return
        try:
            with open(self.queue_path, "r", encoding="utf-8") as f:
                entries = json.load(f)
        except (OSError, ValueError):
            return
        for e in entries:
            d: Optional[Download] = None
            if e.get("state_path") and os.path.exists(e["state_path"]):
                try:
                    d = Download.from_state(e["state_path"])
                except (OSError, ValueError, KeyError):
                    d = None
            if d is None and e.get("status") == Status.COMPLETE.value \
                    and e.get("final_path") and os.path.exists(e["final_path"]):
                d = Download(e["url"], e["dest_dir"], e.get("filename"),
                             e.get("connections", 8))
                d.size = e.get("size") or (os.path.getsize(e["final_path"]))
                d.status = Status.COMPLETE
            if d is None:
                # Nothing on disk to resume; keep it as a fresh queued job.
                d = Download(e["url"], e["dest_dir"], e.get("filename"),
                             e.get("connections", 8))
                d.size = e.get("size")
            self.downloads.append(d)

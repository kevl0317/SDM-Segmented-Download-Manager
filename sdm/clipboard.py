"""Clipboard link monitor.

The classic "copy a download link and a prompt pops up" behavior, done
honestly: SDM polls the clipboard through Tk on a timer. No clipboard hooks,
no OS message interception -- just reading the clipboard the app already has
access to, and only when the user turns the feature on.

The URL test is a pure function and the monitor takes injectable clipboard /
scheduler callables, so both are testable without a display.
"""
from __future__ import annotations

from urllib.parse import urlparse

# Default set of "this looks like a file worth downloading" extensions, so the
# prompt doesn't fire on every ordinary link you copy. Users can switch to
# "all link types" to drop this filter.
DEFAULT_EXTENSIONS = {
    "zip", "rar", "7z", "gz", "tgz", "tar", "bz2", "xz", "z",
    "exe", "msi", "dmg", "pkg", "deb", "rpm", "appimage", "apk", "jar", "bin",
    "iso", "img",
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "mpg", "mpeg", "ts",
    "mp3", "flac", "wav", "aac", "ogg", "m4a", "opus", "wma",
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "epub", "mobi",
    "torrent",
}


def extract_download_url(text, extensions=DEFAULT_EXTENSIONS, monitor_all=False):
    """Return a clean download URL from clipboard text, or None.

    Accepts a single http(s)/ftp URL with no whitespace. Unless monitor_all is
    set, the URL's filename must end in a known downloadable extension.
    """
    if not text:
        return None
    t = text.strip().strip("<>\"'")
    if not t or any(c in t for c in " \t\r\n"):
        return None
    if len(t) > 4096 or not t.lower().startswith(("http://", "https://", "ftp://")):
        return None
    if monitor_all:
        return t
    name = urlparse(t).path.rsplit("/", 1)[-1]
    if "." not in name:
        return None
    ext = name.rsplit(".", 1)[-1].lower()
    return t if ext in {e.lower() for e in extensions} else None


class ClipboardMonitor:
    """Poll the clipboard and call on_url(url) when a new download link appears."""

    def __init__(self, widget, on_url, *, interval_ms=700, extensions=None,
                 monitor_all=False, get_clipboard=None, scheduler=None,
                 canceler=None):
        self.widget = widget
        self.on_url = on_url
        self.interval_ms = interval_ms
        self.extensions = set(extensions) if extensions else set(DEFAULT_EXTENSIONS)
        self.monitor_all = monitor_all
        self._get = get_clipboard or self._tk_clipboard
        self._after = scheduler or widget.after
        self._cancel = canceler or getattr(widget, "after_cancel", lambda job: None)
        self.enabled = False
        self._last = None
        self._job = None

    def _tk_clipboard(self):
        try:
            return self.widget.clipboard_get()
        except Exception:        # noqa: BLE001 - empty/non-text clipboard raises TclError
            return None

    def start(self):
        if self.enabled:
            return
        self.enabled = True
        self._last = self._get()         # ignore whatever is already on the clipboard
        self._schedule()

    def stop(self):
        self.enabled = False
        if self._job is not None:
            try:
                self._cancel(self._job)
            except Exception:    # noqa: BLE001
                pass
            self._job = None

    def ignore(self, value):
        """Treat value as already seen (e.g. a URL SDM itself just copied)."""
        self._last = value

    def _schedule(self):
        if self.enabled:
            self._job = self._after(self.interval_ms, self.poll)

    def poll(self):
        if not self.enabled:
            return
        value = self._get()
        if value is not None and value != self._last:
            self._last = value
            url = extract_download_url(value, self.extensions, self.monitor_all)
            if url:
                try:
                    self.on_url(url)
                except Exception:    # noqa: BLE001 - a handler error must not kill the poll loop
                    pass
        self._schedule()

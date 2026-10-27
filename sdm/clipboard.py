"""Clipboard link monitor.

The classic "copy a download link and a prompt pops up" behavior.

On Windows the clipboard is **not polled**. A listener thread owns a
message-only window subscribed with ``AddClipboardFormatListener``; Windows
delivers ``WM_CLIPBOARDUPDATE`` to it the instant the clipboard changes (the
modern equivalent of the clipboard-viewer chain older download managers use).
The thread only sets a flag -- it never touches Tk, which isn't safe to call
across threads -- and a tiny flag-check on the Tk side reads the clipboard
*only when it actually changed*. So the clipboard is read on real changes, not
on a timer.

Anywhere a native listener can't be set up (non-Windows, or on failure), it
falls back to a light timer poll. No clipboard hooks, no OS message
interception -- just reading the clipboard the app already has access to, and
only when the user turns the feature on.

The URL test is a pure function and the monitor takes injectable clipboard /
scheduler callables, so the logic is testable without a display.
"""
from __future__ import annotations

import sys
import threading
from urllib.parse import urlparse

# Default "this looks like a file worth downloading" extensions, so the prompt
# doesn't fire on every ordinary link. Users can switch to "all link types".
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


class _WinClipboardListener:
    """A daemon thread with a message-only window that calls on_update (from the
    thread) whenever WM_CLIPBOARDUPDATE arrives. The callback must be
    thread-safe and must not touch Tk."""

    WM_CLIPBOARDUPDATE = 0x031D
    WM_QUIT = 0x0012
    GWLP_WNDPROC = -4
    HWND_MESSAGE = -3

    def __init__(self, on_update):
        self.on_update = on_update
        self.ok = False
        self._thread = None
        self._thread_id = None
        self._hwnd = None
        self._wndproc = None            # keep a reference so it isn't GC'd
        self._ready = threading.Event()

    def start(self):
        self._thread = threading.Thread(target=self._run, name="sdm-clip", daemon=True)
        self._thread.start()
        self._ready.wait(5)

    def _run(self):
        import ctypes
        from ctypes import wintypes as wt

        user32 = ctypes.WinDLL("user32", use_last_error=True)
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        LRESULT = ctypes.c_ssize_t
        WNDPROC = ctypes.WINFUNCTYPE(LRESULT, wt.HWND, wt.UINT, wt.WPARAM, wt.LPARAM)

        user32.CreateWindowExW.restype = wt.HWND
        user32.CreateWindowExW.argtypes = [
            wt.DWORD, wt.LPCWSTR, wt.LPCWSTR, wt.DWORD,
            ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int,
            wt.HWND, wt.HMENU, wt.HINSTANCE, wt.LPVOID]
        user32.CallWindowProcW.restype = LRESULT
        user32.CallWindowProcW.argtypes = [ctypes.c_void_p, wt.HWND, wt.UINT, wt.WPARAM, wt.LPARAM]
        set_wlp = getattr(user32, "SetWindowLongPtrW", None) or user32.SetWindowLongW
        set_wlp.restype = ctypes.c_void_p
        set_wlp.argtypes = [wt.HWND, ctypes.c_int, ctypes.c_void_p]
        user32.AddClipboardFormatListener.restype = wt.BOOL
        user32.AddClipboardFormatListener.argtypes = [wt.HWND]
        user32.RemoveClipboardFormatListener.restype = wt.BOOL
        user32.RemoveClipboardFormatListener.argtypes = [wt.HWND]
        user32.GetMessageW.restype = ctypes.c_int
        user32.GetMessageW.argtypes = [ctypes.c_void_p, wt.HWND, wt.UINT, wt.UINT]
        user32.DestroyWindow.argtypes = [wt.HWND]
        kernel32.GetCurrentThreadId.restype = wt.DWORD

        old_proc = ctypes.c_void_p(0)

        def _proc(hwnd, msg, wparam, lparam):
            if msg == self.WM_CLIPBOARDUPDATE:
                try:
                    self.on_update()
                except Exception:   # noqa: BLE001
                    pass
                return 0
            return user32.CallWindowProcW(old_proc, hwnd, msg, wparam, lparam)

        self._wndproc = WNDPROC(_proc)
        self._thread_id = kernel32.GetCurrentThreadId()
        hwnd = None
        try:
            hwnd = user32.CreateWindowExW(0, "STATIC", "SDMClipListener", 0,
                                          0, 0, 0, 0, wt.HWND(self.HWND_MESSAGE),
                                          None, None, None)
            if hwnd:
                old_proc.value = set_wlp(hwnd, self.GWLP_WNDPROC,
                                         ctypes.cast(self._wndproc, ctypes.c_void_p))
                self.ok = bool(user32.AddClipboardFormatListener(hwnd))
                self._hwnd = hwnd
        except Exception:   # noqa: BLE001
            self.ok = False
        finally:
            self._ready.set()

        if not self.ok:
            return

        msg = wt.MSG()
        while user32.GetMessageW(ctypes.byref(msg), None, 0, 0) > 0:
            user32.TranslateMessage(ctypes.byref(msg))
            user32.DispatchMessageW(ctypes.byref(msg))
        try:
            user32.RemoveClipboardFormatListener(hwnd)
            user32.DestroyWindow(hwnd)
        except Exception:   # noqa: BLE001
            pass

    def stop(self):
        if self._thread_id:
            try:
                import ctypes
                ctypes.windll.user32.PostThreadMessageW(self._thread_id, self.WM_QUIT, 0, 0)
            except Exception:   # noqa: BLE001
                pass
        if self._thread:
            self._thread.join(2)


class ClipboardMonitor:
    """Watch the clipboard and call on_url(url) when a new download link appears.

    Event-driven on Windows (clipboard read only on real changes); falls back to
    polling elsewhere or on failure.
    """

    def __init__(self, widget, on_url, *, interval_ms=1000, signal_ms=200,
                 extensions=None, monitor_all=False, get_clipboard=None,
                 scheduler=None, canceler=None, use_native=True):
        self.widget = widget
        self.on_url = on_url
        self.interval_ms = interval_ms        # polling fallback period
        self.signal_ms = signal_ms            # native: how often the GUI checks the change flag
        self.extensions = set(extensions) if extensions else set(DEFAULT_EXTENSIONS)
        self.monitor_all = monitor_all
        self._get = get_clipboard or self._tk_clipboard
        self._after = scheduler or (widget.after if widget is not None else None)
        self._cancel = canceler or (getattr(widget, "after_cancel", None) if widget is not None else None)
        self.use_native = bool(use_native and sys.platform == "win32" and widget is not None)
        self.enabled = False
        self.event_driven = False
        self._last = None
        self._job = None
        self._native = None
        self._signal = threading.Event()      # set by the listener thread on a real change

    def _tk_clipboard(self):
        try:
            return self.widget.clipboard_get()
        except Exception:   # noqa: BLE001 - empty/non-text clipboard raises TclError
            return None

    def start(self):
        if self.enabled:
            return
        self.enabled = True
        self._last = self._get()         # ignore whatever is already on the clipboard
        if self.use_native and self._start_native():
            self.event_driven = True
            self._signal.clear()
            self._job = self._after(self.signal_ms, self._drain)
            return
        self.event_driven = False
        self._schedule()                 # polling fallback

    def _start_native(self):
        try:
            self._native = _WinClipboardListener(self._signal.set)
            self._native.start()
            if not self._native.ok:
                self._native.stop()
                self._native = None
                return False
            return True
        except Exception:   # noqa: BLE001
            self._native = None
            return False

    def _drain(self):
        """Native path: a cheap flag check — reads the clipboard only when the
        listener thread signalled a real change."""
        if not self.enabled or self._native is None:
            return
        if self._signal.is_set():
            self._signal.clear()
            self._handle_value(self._get())
        self._job = self._after(self.signal_ms, self._drain)

    def stop(self):
        self.enabled = False
        self.event_driven = False
        if self._native is not None:
            try:
                self._native.stop()
            except Exception:   # noqa: BLE001
                pass
            self._native = None
        if self._job is not None and self._cancel is not None:
            try:
                self._cancel(self._job)
            except Exception:   # noqa: BLE001
                pass
            self._job = None

    def ignore(self, value):
        """Treat value as already seen (e.g. a URL SDM itself just copied)."""
        self._last = value

    def _schedule(self):
        if self.enabled and self._native is None and self._after is not None:
            self._job = self._after(self.interval_ms, self.poll)

    def poll(self):
        """Timer tick for the polling fallback."""
        if not self.enabled or self._native is not None:
            return
        self._handle_value(self._get())
        self._schedule()

    def _handle_value(self, value):
        if value is not None and value != self._last:
            self._last = value
            url = extract_download_url(value, self.extensions, self.monitor_all)
            if url:
                try:
                    self.on_url(url)
                except Exception:   # noqa: BLE001 - a handler error must not kill monitoring
                    pass

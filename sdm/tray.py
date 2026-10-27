"""Windows system-tray icon (stdlib ctypes).

Lets SDM keep running in the notification area after its window is closed, so
background downloads and clipboard monitoring stay alive. A dedicated thread
owns a message-only window and the Shell_NotifyIcon icon and pumps its own
messages. The on_open / on_quit callbacks are invoked from that thread, so they
must be thread-safe -- in practice they only set a flag the GUI drains on the
Tk thread.
"""
from __future__ import annotations

import sys
import threading

TRAY_AVAILABLE = sys.platform == "win32"


class SystemTray:
    def __init__(self, tooltip="SDM", on_open=None, on_quit=None):
        self.tooltip = (tooltip or "SDM")[:120]
        self.on_open = on_open or (lambda: None)
        self.on_quit = on_quit or (lambda: None)
        self.ok = False
        self._thread = None
        self._thread_id = None
        self._ready = threading.Event()

    def start(self):
        if not TRAY_AVAILABLE:
            return False
        self._thread = threading.Thread(target=self._run, name="sdm-tray", daemon=True)
        self._thread.start()
        self._ready.wait(5)
        return self.ok

    def stop(self):
        if self._thread_id:
            try:
                import ctypes
                ctypes.windll.user32.PostThreadMessageW(self._thread_id, 0x0012, 0, 0)  # WM_QUIT
            except Exception:   # noqa: BLE001
                pass
        if self._thread:
            self._thread.join(2)

    def _run(self):
        import ctypes
        from ctypes import wintypes as wt

        user32 = ctypes.WinDLL("user32", use_last_error=True)
        shell32 = ctypes.WinDLL("shell32", use_last_error=True)
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        LRESULT = ctypes.c_ssize_t

        WM_APP = 0x8000
        TRAY_CB = WM_APP + 1
        WM_LBUTTONUP, WM_LBUTTONDBLCLK, WM_RBUTTONUP, WM_CONTEXTMENU = 0x0202, 0x0203, 0x0205, 0x007B
        GWLP_WNDPROC, HWND_MESSAGE = -4, -3
        NIM_ADD, NIM_DELETE = 0, 2
        NIF_MESSAGE, NIF_ICON, NIF_TIP = 0x1, 0x2, 0x4
        TPM_RETURNCMD, TPM_RIGHTBUTTON = 0x0100, 0x0002
        MF_STRING, MF_SEPARATOR = 0x0, 0x800
        ID_OPEN, ID_QUIT = 1, 2
        IDI_APPLICATION = 32512

        class GUID(ctypes.Structure):
            _fields_ = [("Data1", wt.DWORD), ("Data2", wt.WORD), ("Data3", wt.WORD),
                        ("Data4", ctypes.c_ubyte * 8)]

        class NID(ctypes.Structure):
            _fields_ = [("cbSize", wt.DWORD), ("hWnd", wt.HWND), ("uID", wt.UINT),
                        ("uFlags", wt.UINT), ("uCallbackMessage", wt.UINT), ("hIcon", wt.HICON),
                        ("szTip", wt.WCHAR * 128), ("dwState", wt.DWORD), ("dwStateMask", wt.DWORD),
                        ("szInfo", wt.WCHAR * 256), ("uVersion", wt.UINT),
                        ("szInfoTitle", wt.WCHAR * 64), ("dwInfoFlags", wt.DWORD),
                        ("guidItem", GUID), ("hBalloonIcon", wt.HICON)]

        class POINT(ctypes.Structure):
            _fields_ = [("x", wt.LONG), ("y", wt.LONG)]

        WNDPROC = ctypes.WINFUNCTYPE(LRESULT, wt.HWND, wt.UINT, wt.WPARAM, wt.LPARAM)
        user32.CreateWindowExW.restype = wt.HWND
        user32.CreateWindowExW.argtypes = [wt.DWORD, wt.LPCWSTR, wt.LPCWSTR, wt.DWORD,
                                           ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int,
                                           wt.HWND, wt.HMENU, wt.HINSTANCE, wt.LPVOID]
        user32.CallWindowProcW.restype = LRESULT
        user32.CallWindowProcW.argtypes = [ctypes.c_void_p, wt.HWND, wt.UINT, wt.WPARAM, wt.LPARAM]
        set_wlp = getattr(user32, "SetWindowLongPtrW", None) or user32.SetWindowLongW
        set_wlp.restype = ctypes.c_void_p
        set_wlp.argtypes = [wt.HWND, ctypes.c_int, ctypes.c_void_p]
        user32.GetMessageW.restype = ctypes.c_int
        user32.GetMessageW.argtypes = [ctypes.c_void_p, wt.HWND, wt.UINT, wt.UINT]
        user32.DestroyWindow.argtypes = [wt.HWND]
        user32.CreatePopupMenu.restype = wt.HMENU
        user32.AppendMenuW.argtypes = [wt.HMENU, wt.UINT, ctypes.c_size_t, wt.LPCWSTR]
        user32.TrackPopupMenu.restype = ctypes.c_int
        user32.TrackPopupMenu.argtypes = [wt.HMENU, wt.UINT, ctypes.c_int, ctypes.c_int,
                                          ctypes.c_int, wt.HWND, wt.LPVOID]
        user32.GetCursorPos.argtypes = [ctypes.POINTER(POINT)]
        user32.SetForegroundWindow.argtypes = [wt.HWND]
        user32.DestroyMenu.argtypes = [wt.HMENU]
        user32.LoadIconW.restype = wt.HICON
        user32.LoadIconW.argtypes = [wt.HINSTANCE, ctypes.c_void_p]
        user32.RegisterWindowMessageW.restype = wt.UINT
        user32.RegisterWindowMessageW.argtypes = [wt.LPCWSTR]
        shell32.Shell_NotifyIconW.restype = wt.BOOL
        shell32.Shell_NotifyIconW.argtypes = [wt.DWORD, ctypes.POINTER(NID)]
        shell32.ExtractIconW.restype = wt.HICON
        shell32.ExtractIconW.argtypes = [wt.HINSTANCE, wt.LPCWSTR, wt.UINT]
        kernel32.GetModuleHandleW.restype = wt.HMODULE
        kernel32.GetModuleHandleW.argtypes = [wt.LPCWSTR]
        kernel32.GetCurrentThreadId.restype = wt.DWORD

        taskbar_created = user32.RegisterWindowMessageW("TaskbarCreated")

        # Icon: the frozen exe carries SDM's icon; otherwise fall back to the default app icon.
        hicon = 0
        try:
            h = shell32.ExtractIconW(kernel32.GetModuleHandleW(None), sys.executable, 0)
            if h and h > 1:
                hicon = h
        except Exception:   # noqa: BLE001
            pass
        if not hicon:
            hicon = user32.LoadIconW(None, ctypes.c_void_p(IDI_APPLICATION))

        nid = NID()
        nid.cbSize = ctypes.sizeof(NID)
        nid.uID = 1
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP
        nid.uCallbackMessage = TRAY_CB
        nid.hIcon = hicon
        nid.szTip = self.tooltip

        old_proc = ctypes.c_void_p(0)

        def _show_menu(hwnd):
            hmenu = user32.CreatePopupMenu()
            user32.AppendMenuW(hmenu, MF_STRING, ID_OPEN, "Open SDM")
            user32.AppendMenuW(hmenu, MF_SEPARATOR, 0, None)
            user32.AppendMenuW(hmenu, MF_STRING, ID_QUIT, "Quit SDM")
            pt = POINT()
            user32.GetCursorPos(ctypes.byref(pt))
            user32.SetForegroundWindow(hwnd)   # so the menu dismisses on click-away
            cmd = user32.TrackPopupMenu(hmenu, TPM_RETURNCMD | TPM_RIGHTBUTTON,
                                        pt.x, pt.y, 0, hwnd, None)
            user32.DestroyMenu(hmenu)
            if cmd == ID_OPEN:
                self.on_open()
            elif cmd == ID_QUIT:
                self.on_quit()

        def _proc(hwnd, msg, wparam, lparam):
            if msg == TRAY_CB:
                event = lparam & 0xFFFF
                if event in (WM_LBUTTONUP, WM_LBUTTONDBLCLK):
                    try:
                        self.on_open()
                    except Exception:   # noqa: BLE001
                        pass
                elif event in (WM_RBUTTONUP, WM_CONTEXTMENU):
                    try:
                        _show_menu(hwnd)
                    except Exception:   # noqa: BLE001
                        pass
                return 0
            if msg == taskbar_created and nid.hWnd:
                shell32.Shell_NotifyIconW(NIM_ADD, ctypes.byref(nid))  # re-add if Explorer restarted
                return 0
            return user32.CallWindowProcW(old_proc, hwnd, msg, wparam, lparam)

        self._wndproc = WNDPROC(_proc)   # keep ref alive
        self._thread_id = kernel32.GetCurrentThreadId()
        hwnd = None
        try:
            hwnd = user32.CreateWindowExW(0, "STATIC", "SDMTray", 0, 0, 0, 0, 0,
                                          wt.HWND(HWND_MESSAGE), None, None, None)
            if hwnd:
                old_proc.value = set_wlp(hwnd, GWLP_WNDPROC,
                                         ctypes.cast(self._wndproc, ctypes.c_void_p))
                nid.hWnd = hwnd
                self.ok = bool(shell32.Shell_NotifyIconW(NIM_ADD, ctypes.byref(nid)))
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
            shell32.Shell_NotifyIconW(NIM_DELETE, ctypes.byref(nid))
            user32.DestroyWindow(hwnd)
        except Exception:   # noqa: BLE001
            pass

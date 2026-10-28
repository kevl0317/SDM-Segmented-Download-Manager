"""Tkinter desktop GUI for the segmented download manager.

Themed (ttk 'clam' + a cohesive palette), DPI-aware so it renders crisply on
high-resolution displays, with colour-coded statuses, a right-click menu,
selection-aware toolbar buttons, and a details pane whose canvas visualises
each parallel segment filling in real time.
"""
from __future__ import annotations

import json
import os
import queue
import subprocess
import sys
import threading
import tkinter as tk
from tkinter import filedialog, messagebox, ttk

from .cli import human_bytes, human_time
from .clipboard import ClipboardMonitor
from .engine import Download, Status
from .ipc import IpcServer
from .manager import Manager, config_dir
from .tray import SystemTray, TRAY_AVAILABLE

REFRESH_MS = 400
FONT_FAMILY = "Segoe UI" if sys.platform == "win32" else "TkDefaultFont"

# Set by main() once the real display DPI is known; 1.0 == 96 dpi (100%).
UI_SCALE = 1.0

PALETTE = {
    "bg": "#eef1f5",
    "surface": "#ffffff",
    "stripe": "#f4f6f9",
    "sel": "#d7e7ff",
    "accent": "#2d7ff9",
    "accent_dark": "#1b63d0",
    "text": "#1f2328",
    "muted": "#6b7280",
    "border": "#d4d8de",
    "track": "#e3e6ea",
    "success": "#1a7f37",
    "danger": "#cf222e",
    "paused": "#9a6700",
}

STATUS_TEXT = {
    Status.QUEUED: "Queued",
    Status.PROBING: "Connecting…",
    Status.RUNNING: "Downloading",
    Status.PAUSED: "Paused",
    Status.COMPLETE: "Complete",
    Status.ERROR: "Error",
}
STATUS_TAG = {
    Status.RUNNING: "running", Status.COMPLETE: "complete",
    Status.ERROR: "error", Status.PAUSED: "paused",
}

SETTINGS_PATH = os.path.join(config_dir(), "settings.json")


# ---- scaling helpers -----------------------------------------------------
def px(n: float) -> int:
    """Pixels scaled for the current display DPI."""
    return max(1, int(round(n * UI_SCALE)))


def fontspec(size_px: int, weight: str = "normal") -> tuple:
    """A font tuple sized in *pixels* (negative Tk size) so it is immune to
    Tk's point-based scaling and tracks UI_SCALE exactly."""
    spec = (FONT_FAMILY, -px(size_px))
    return spec + (weight,) if weight != "normal" else spec


def enable_hidpi() -> float:
    """Mark the process DPI-aware (Windows) so it is not bitmap-stretched,
    and return the display scale factor. Must run before the Tk root exists."""
    if sys.platform != "win32":
        return 1.0
    import ctypes
    try:
        ctypes.windll.shcore.SetProcessDpiAwareness(2)   # per-monitor-aware v2
    except Exception:  # noqa: BLE001 - older Windows
        try:
            ctypes.windll.user32.SetProcessDPIAware()
        except Exception:  # noqa: BLE001
            return 1.0
    try:
        dpi = ctypes.windll.user32.GetDpiForSystem()
    except Exception:  # noqa: BLE001
        dpi = 96
    return max(1.0, dpi / 96.0)


def primary_screen_size(widget) -> tuple:
    """Primary (main) monitor size in physical pixels."""
    if sys.platform == "win32":
        try:
            import ctypes
            u = ctypes.windll.user32
            w, h = int(u.GetSystemMetrics(0)), int(u.GetSystemMetrics(1))  # SM_CXSCREEN / SM_CYSCREEN
            if w > 0 and h > 0:
                return w, h
        except Exception:  # noqa: BLE001
            pass
    try:
        return widget.winfo_screenwidth(), widget.winfo_screenheight()
    except tk.TclError:
        return 1920, 1080


# ---- settings ------------------------------------------------------------
def load_settings() -> dict:
    try:
        with open(SETTINGS_PATH, "r", encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError):
        return {}


def save_settings(data: dict) -> None:
    try:
        with open(SETTINGS_PATH, "w", encoding="utf-8") as f:
            json.dump(data, f, indent=1)
    except OSError:
        pass


def apply_theme(root: tk.Misc) -> ttk.Style:
    style = ttk.Style(root)
    try:
        style.theme_use("clam")
    except tk.TclError:
        pass
    if sys.platform == "win32":
        # Scale point-based text (menus, native dialogs) to match the DPI.
        try:
            root.tk.call("tk", "scaling", UI_SCALE * 96.0 / 72.0)
        except tk.TclError:
            pass
    p = PALETTE
    body = fontspec(13)
    root.configure(bg=p["bg"])
    style.configure(".", background=p["bg"], foreground=p["text"], font=body)
    style.configure("TFrame", background=p["bg"])
    style.configure("Surface.TFrame", background=p["surface"])
    style.configure("Card.TFrame", background=p["surface"])
    style.configure("TLabel", background=p["bg"], foreground=p["text"], font=body)
    style.configure("Muted.TLabel", background=p["bg"], foreground=p["muted"], font=fontspec(12))
    style.configure("Detail.TLabel", background=p["surface"], foreground=p["text"], font=body)
    style.configure("DetailTitle.TLabel", background=p["surface"], foreground=p["text"],
                    font=fontspec(16, "bold"))
    style.configure("DetailMuted.TLabel", background=p["surface"], foreground=p["muted"],
                    font=fontspec(12))

    style.configure("TButton", padding=(px(14), px(8)), relief="flat",
                    background=p["track"], foreground=p["text"], font=body, borderwidth=0)
    style.map("TButton",
              background=[("active", "#d2d6dc"), ("disabled", p["bg"])],
              foreground=[("disabled", "#b2b6bc")])
    style.configure("Accent.TButton", background=p["accent"], foreground="#ffffff",
                    font=fontspec(13, "bold"))
    style.map("Accent.TButton",
              background=[("active", p["accent_dark"]), ("disabled", "#a8c6f6")],
              foreground=[("disabled", "#eef1f5")])
    style.configure("TSpinbox", arrowsize=px(13))
    style.configure("TEntry", padding=px(4))
    style.configure("Vertical.TScrollbar", background=p["bg"], troughcolor=p["bg"],
                    borderwidth=0, arrowsize=px(13))

    style.configure("Treeview", background=p["surface"], fieldbackground=p["surface"],
                    foreground=p["text"], rowheight=px(34), borderwidth=0, font=body)
    style.configure("Treeview.Heading", background=p["bg"], foreground=p["muted"],
                    relief="flat", padding=(px(10), px(8)), font=fontspec(11, "bold"))
    style.map("Treeview.Heading", background=[("active", p["track"])])
    style.map("Treeview",
              background=[("selected", p["sel"])],
              foreground=[("selected", p["text"])])
    return style


class SegmentBar(tk.Canvas):
    """Draws one block per segment, each filled by its download ratio."""

    def __init__(self, master, height_px: int = 30):
        super().__init__(master, height=px(height_px), highlightthickness=max(1, px(1)),
                         highlightbackground=PALETTE["border"], bg=PALETTE["surface"])
        self._d: Download | None = None
        self.bind("<Configure>", lambda e: self._redraw())

    def show(self, d: Download | None) -> None:
        self._d = d
        self._redraw()

    def _redraw(self) -> None:
        self.delete("all")
        w, h = self.winfo_width(), self.winfo_height()
        if w <= 1:
            return
        p = PALETTE
        d = self._d
        if d is None:
            self.create_text(w // 2, h // 2, text="Select a download to see its segments",
                             fill=p["muted"], font=fontspec(12))
            return
        pad, gap = px(4), max(1, px(2))
        segs = d.segments
        if segs and d.size:
            n = len(segs)
            avail = w - 2 * pad - gap * (n - 1)
            x = float(pad)
            for s in segs:
                sw = max(3.0, avail * (s.length / d.size))
                self.create_rectangle(x, pad, x + sw, h - pad, fill=p["track"], width=0)
                frac = (s.downloaded / s.length) if s.length else 0.0
                if frac > 0:
                    col = p["success"] if s.done else p["accent"]
                    self.create_rectangle(x, pad, x + sw * frac, h - pad, fill=col, width=0)
                x += sw + gap
        else:
            self.create_rectangle(pad, pad, w - pad, h - pad, fill=p["track"], width=0)
            prog = d.progress()
            if prog is not None:
                self.create_rectangle(pad, pad, pad + (w - 2 * pad) * prog, h - pad,
                                      fill=p["accent"], width=0)
            else:
                self.create_text(w // 2, h // 2, font=fontspec(12),
                                 text=f"{human_bytes(d.downloaded())} (single stream)",
                                 fill=p["muted"])


class AddDialog(tk.Toplevel):
    def __init__(self, master, default_dir: str, default_conns: int, clipboard_url: str = "",
                 standalone: bool = False, filename: str = ""):
        super().__init__(master)
        self.title("Download this link?" if standalone else "Add download")
        self.resizable(False, False)
        self.configure(bg=PALETTE["bg"])
        self.result = None
        self._standalone = standalone
        try:
            master_visible = bool(master.winfo_viewable())
        except tk.TclError:
            master_visible = False
        if not standalone or master_visible:
            self.transient(master)   # skip when popping over a hidden (tray) main window

        frm = ttk.Frame(self, padding=px(16))
        frm.grid(sticky="nsew")
        frm.columnconfigure(1, weight=1)

        ttk.Label(frm, text="URL").grid(row=0, column=0, sticky="w", pady=px(5), padx=(0, px(10)))
        self.url_var = tk.StringVar(value=clipboard_url)
        url_entry = ttk.Entry(frm, textvariable=self.url_var, width=62, font=fontspec(13))
        url_entry.grid(row=0, column=1, columnspan=2, sticky="ew", pady=px(5))

        ttk.Label(frm, text="Save to").grid(row=1, column=0, sticky="w", pady=px(5), padx=(0, px(10)))
        self.dir_var = tk.StringVar(value=default_dir)
        ttk.Entry(frm, textvariable=self.dir_var, font=fontspec(13)).grid(
            row=1, column=1, sticky="ew", pady=px(5))
        ttk.Button(frm, text="Browse…", command=self._browse).grid(row=1, column=2, padx=(px(8), 0))

        ttk.Label(frm, text="Filename").grid(row=2, column=0, sticky="w", pady=px(5), padx=(0, px(10)))
        self.name_var = tk.StringVar(value=filename)
        ttk.Entry(frm, textvariable=self.name_var, font=fontspec(13)).grid(
            row=2, column=1, columnspan=2, sticky="ew", pady=px(5))
        ttk.Label(frm, text="leave blank to name it from the server / URL",
                  style="Muted.TLabel").grid(row=3, column=1, columnspan=2, sticky="w")

        ttk.Label(frm, text="Connections").grid(row=4, column=0, sticky="w", pady=px(5), padx=(0, px(10)))
        self.conn_var = tk.IntVar(value=default_conns)
        ttk.Spinbox(frm, from_=1, to=32, textvariable=self.conn_var, width=6,
                    font=fontspec(13)).grid(row=4, column=1, sticky="w", pady=px(5))

        btns = ttk.Frame(frm)
        btns.grid(row=5, column=0, columnspan=3, sticky="e", pady=(px(14), 0))
        ttk.Button(btns, text="Cancel", command=self.destroy).pack(side="right", padx=px(6))
        ttk.Button(btns, text="Download", style="Accent.TButton", command=self._ok).pack(side="right")

        url_entry.focus_set()
        url_entry.selection_range(0, "end")
        self.bind("<Return>", lambda e: self._ok())
        self.bind("<Escape>", lambda e: self.destroy())
        self.update_idletasks()
        if standalone:
            self.bind("<Map>", self._center_on_map)   # center once shown; before map the WM ignores it
            try:
                self.attributes("-topmost", True)   # the catch popup floats above everything
                self.lift()
            except tk.TclError:
                pass
            self.after(60, self.focus_force)
        else:
            self._center(master)
        self.grab_set()

    def _center_on_map(self, event):
        if event.widget is self:
            self.unbind("<Map>")
            self._center_screen()

    def _center(self, master):
        try:
            x = master.winfo_rootx() + (master.winfo_width() - self.winfo_width()) // 2
            y = master.winfo_rooty() + (master.winfo_height() - self.winfo_height()) // 3
            self.geometry(f"+{max(0, x)}+{max(0, y)}")
        except tk.TclError:
            pass

    def _center_screen(self):
        try:
            self.update_idletasks()
            w = self.winfo_reqwidth() or self.winfo_width()
            h = self.winfo_reqheight() or self.winfo_height()
            sw, sh = primary_screen_size(self)
            x = max(0, (sw - w) // 2)
            y = max(0, (sh - h) // 2)
            self.geometry("%dx%d+%d+%d" % (w, h, x, y))   # full WxH+X+Y; "+x+y" alone isn't applied
        except tk.TclError:
            pass

    def _browse(self):
        d = filedialog.askdirectory(initialdir=self.dir_var.get() or os.getcwd(), parent=self)
        if d:
            self.dir_var.set(d)

    def _ok(self):
        url = self.url_var.get().strip()
        if not url.lower().startswith(("http://", "https://", "ftp://")):
            messagebox.showerror("Invalid URL", "Enter an http(s):// or ftp:// URL.", parent=self)
            return
        self.result = {
            "url": url,
            "dest_dir": self.dir_var.get().strip() or None,
            "filename": self.name_var.get().strip() or None,
            "connections": max(1, int(self.conn_var.get())),
        }
        self.destroy()


class App(ttk.Frame):
    def __init__(self, master, manager: Manager):
        super().__init__(master, padding=0)
        self.manager = manager
        self.settings = load_settings()
        self.grid(sticky="nsew")
        master.rowconfigure(0, weight=1)
        master.columnconfigure(0, weight=1)
        self.rowconfigure(1, weight=1)
        self.columnconfigure(0, weight=1)
        self._rows: dict[str, Download] = {}
        self._dialog_open = False
        self.mon_var = tk.BooleanVar(value=bool(self.settings.get("clipboard_monitor", True)))
        self.all_var = tk.BooleanVar(value=bool(self.settings.get("clipboard_all", False)))
        self.clip = ClipboardMonitor(self, self._on_clip_url, monitor_all=self.all_var.get())
        self.tray_var = tk.BooleanVar(value=bool(self.settings.get("close_to_tray", True)))
        self._tray_open_evt = threading.Event()
        self._tray_quit_evt = threading.Event()
        self._tray_after_id = None
        self._topmost_after = None
        self.tray = (SystemTray("SDM - Segmented Download Manager",
                                on_open=self._tray_open_evt.set, on_quit=self._tray_quit_evt.set)
                     if TRAY_AVAILABLE else None)
        self._ipc_queue = queue.Queue()
        self._ipc = IpcServer(on_request=self._ipc_queue.put)
        self._ipc_after_id = None

        self._build_toolbar()
        self._build_tree()
        self._build_details()
        self._build_statusbar()
        self._build_menu(master)

        self._after_id = None
        for d in self.manager.downloads:
            self._add_row(d)
        self._update_actions()
        self._after_id = self.after(REFRESH_MS, self._refresh)
        if self.mon_var.get():
            self.clip.start()
        if self.tray is not None and self.tray.start():
            self._tray_after_id = self.after(150, self._tray_drain)
        if self._ipc.start():
            self._ipc_after_id = self.after(150, self._ipc_drain)
        master.protocol("WM_DELETE_WINDOW", self._on_window_close)

    def stop(self) -> None:
        self.clip.stop()
        if self.tray is not None:
            try:
                self.tray.stop()
            except Exception:   # noqa: BLE001
                pass
        try:
            self._ipc.stop()
        except Exception:   # noqa: BLE001
            pass
        for attr in ("_after_id", "_tray_after_id", "_topmost_after", "_ipc_after_id"):
            job = getattr(self, attr, None)
            if job is not None:
                try:
                    self.after_cancel(job)
                except tk.TclError:
                    pass
                setattr(self, attr, None)

    # ---- construction ----------------------------------------------------
    def _build_toolbar(self):
        bar = ttk.Frame(self, padding=(px(12), px(10)))
        bar.grid(row=0, column=0, sticky="ew")
        self.btn_add = ttk.Button(bar, text="Add URL", style="Accent.TButton", command=self.on_add)
        self.btn_add.pack(side="left")
        self.btn_resume = ttk.Button(bar, text="Resume", command=self.on_resume)
        self.btn_resume.pack(side="left", padx=(px(8), 0))
        self.btn_pause = ttk.Button(bar, text="Pause", command=self.on_pause)
        self.btn_pause.pack(side="left", padx=px(8))
        self.btn_remove = ttk.Button(bar, text="Remove", command=self.on_remove)
        self.btn_remove.pack(side="left")
        self.btn_open = ttk.Button(bar, text="Open folder", command=self.on_open)
        self.btn_open.pack(side="left", padx=px(8))
        ttk.Button(bar, text="Clear completed", command=self.on_clear_completed).pack(side="right")

    def _build_tree(self):
        wrap = tk.Frame(self, background=PALETTE["border"], bd=0,
                        highlightthickness=max(1, px(1)), highlightbackground=PALETTE["border"])
        wrap.grid(row=1, column=0, sticky="nsew", padx=px(12))
        wrap.rowconfigure(0, weight=1)
        wrap.columnconfigure(0, weight=1)
        cols = ("name", "size", "progress", "speed", "eta", "status")
        self.tree = ttk.Treeview(wrap, columns=cols, show="headings", selectmode="browse")
        layout = {
            "name": ("Name", 300, "w"), "size": ("Size", 95, "e"),
            "progress": ("Progress", 190, "w"), "speed": ("Speed", 100, "e"),
            "eta": ("ETA", 75, "e"), "status": ("Status", 120, "w"),
        }
        for c, (txt, w, anchor) in layout.items():
            self.tree.heading(c, text=txt)
            self.tree.column(c, width=px(w), anchor=anchor, stretch=(c == "name"))
        self.tree.grid(row=0, column=0, sticky="nsew")
        vsb = ttk.Scrollbar(wrap, orient="vertical", command=self.tree.yview)
        self.tree.configure(yscrollcommand=vsb.set)
        vsb.grid(row=0, column=1, sticky="ns")

        self.tree.tag_configure("odd", background=PALETTE["stripe"])
        self.tree.tag_configure("even", background=PALETTE["surface"])
        self.tree.tag_configure("running", foreground=PALETTE["accent"])
        self.tree.tag_configure("complete", foreground=PALETTE["success"])
        self.tree.tag_configure("error", foreground=PALETTE["danger"])
        self.tree.tag_configure("paused", foreground=PALETTE["paused"])

        self.tree.bind("<<TreeviewSelect>>", lambda e: self._on_select())
        self.tree.bind("<Double-1>", lambda e: self.on_open())
        self.tree.bind("<Delete>", lambda e: self.on_remove())
        self.tree.bind("<Button-3>", self._popup)

        self.menu = tk.Menu(self, tearoff=0)
        self.menu.add_command(label="Resume", command=self.on_resume)
        self.menu.add_command(label="Pause", command=self.on_pause)
        self.menu.add_separator()
        self.menu.add_command(label="Open folder", command=self.on_open)
        self.menu.add_command(label="Copy URL", command=self.on_copy_url)
        self.menu.add_separator()
        self.menu.add_command(label="Remove", command=self.on_remove)

    def _build_details(self):
        outer = tk.Frame(self, background=PALETTE["border"],
                         highlightthickness=max(1, px(1)), highlightbackground=PALETTE["border"])
        outer.grid(row=2, column=0, sticky="ew", padx=px(12), pady=(px(10), 0))
        outer.columnconfigure(0, weight=1)
        box = ttk.Frame(outer, style="Card.TFrame", padding=px(14))
        box.grid(sticky="ew")
        box.columnconfigure(0, weight=1)
        self.d_title = ttk.Label(box, text="No download selected", style="DetailTitle.TLabel")
        self.d_title.grid(row=0, column=0, sticky="w")
        self.seg_bar = SegmentBar(box, height_px=28)
        self.seg_bar.grid(row=1, column=0, sticky="ew", pady=(px(10), px(10)))
        self.d_meta = ttk.Label(box, text="", style="DetailMuted.TLabel", justify="left")
        self.d_meta.grid(row=2, column=0, sticky="w")
        self.d_url = ttk.Label(box, text="", style="DetailMuted.TLabel", justify="left")
        self.d_url.grid(row=3, column=0, sticky="w", pady=(px(2), 0))

    def _build_statusbar(self):
        self.status_bar = ttk.Label(self, text="Ready", style="Muted.TLabel",
                                    padding=(px(14), px(8)))
        self.status_bar.grid(row=3, column=0, sticky="ew")

    def _build_menu(self, master):
        menubar = tk.Menu(master)
        filem = tk.Menu(menubar, tearoff=0)
        filem.add_command(label="Add URL…", command=self.on_add, accelerator="Ctrl+N")
        filem.add_separator()
        filem.add_command(label="Resume all", command=self.on_resume_all)
        filem.add_command(label="Pause all", command=self.manager.pause_all)
        filem.add_command(label="Clear completed", command=self.on_clear_completed)
        filem.add_separator()
        filem.add_checkbutton(label="Monitor clipboard for links",
                              variable=self.mon_var, command=self._toggle_monitor)
        filem.add_checkbutton(label="Monitor all link types (not just files)",
                              variable=self.all_var, command=self._toggle_all)
        if self.tray is not None:
            filem.add_checkbutton(label="Keep running in the tray when closed",
                                  variable=self.tray_var, command=self._toggle_close_to_tray)
        filem.add_separator()
        filem.add_command(label="Exit", command=self._real_quit)
        menubar.add_cascade(label="File", menu=filem)
        helpm = tk.Menu(menubar, tearoff=0)
        helpm.add_command(label="About", command=self._about)
        menubar.add_cascade(label="Help", menu=helpm)
        master.config(menu=menubar)
        master.bind("<Control-n>", lambda e: self.on_add())

    # ---- helpers ---------------------------------------------------------
    def _add_row(self, d: Download) -> str:
        iid = str(id(d))
        self.tree.insert("", "end", iid=iid, values=(d._resolved_name, "", "", "", "", ""))
        self._rows[iid] = d
        return iid

    def _selected(self) -> Download | None:
        sel = self.tree.selection()
        return self._rows.get(sel[0]) if sel else None

    def _progress_cell(self, d: Download) -> str:
        p = d.progress()
        if p is None:
            return "downloading…" if d.status is Status.RUNNING else "—"
        blocks = int(p * 16)
        return "█" * blocks + "░" * (16 - blocks) + f" {p * 100:4.1f}%"

    def _popup(self, event):
        iid = self.tree.identify_row(event.y)
        if iid:
            self.tree.selection_set(iid)
            self._on_select()
            self.menu.tk_popup(event.x_root, event.y_root)

    # ---- actions ---------------------------------------------------------
    def on_add(self):
        clip = ""
        try:
            c = self.clipboard_get().strip()
            if c.lower().startswith(("http://", "https://", "ftp://")) and len(c) < 2048:
                clip = c
        except tk.TclError:
            pass
        self._open_add(clip)

    def _open_add(self, prefill="", from_catch=False, extra=None):
        if self._dialog_open:
            return
        self._dialog_open = True
        try:
            filename = (extra or {}).get("filename") or ""
            dlg = AddDialog(self.master, self.settings.get("last_dir", self.manager.default_dir),
                            int(self.settings.get("connections", 8)), prefill,
                            standalone=from_catch, filename=filename)
            self.wait_window(dlg)
            if dlg.result:
                self.settings["last_dir"] = dlg.result["dest_dir"] or self.manager.default_dir
                self.settings["connections"] = dlg.result["connections"]
                save_settings(self.settings)
                self.clip.ignore(dlg.result["url"])    # don't re-prompt for what we just added
                if from_catch:
                    self._bring_to_front()   # raise the GUI only now that the user chose Download
                d = self.manager.add(**dlg.result, headers=self._build_headers(extra))
                iid = self._add_row(d)
                self.tree.selection_set(iid)
                d.start()
        finally:
            self._dialog_open = False

    @staticmethod
    def _build_headers(extra):
        """Turn a browser catch's cookies/UA/referrer into request headers."""
        if not extra:
            return None
        from .engine import default_headers
        h = default_headers()
        if extra.get("user_agent"):
            h["User-Agent"] = extra["user_agent"]
        if extra.get("cookies"):
            h["Cookie"] = extra["cookies"]
        if extra.get("referrer"):
            h["Referer"] = extra["referrer"]
        return h

    def _ipc_drain(self):
        """Process download requests handed over by the browser native host."""
        try:
            while True:
                self._handle_ipc(self._ipc_queue.get_nowait())
        except queue.Empty:
            pass
        self._ipc_after_id = self.after(150, self._ipc_drain)

    def _handle_ipc(self, msg):
        url = msg.get("url") if isinstance(msg, dict) else None
        if url:
            extra = {k: msg.get(k) for k in ("cookies", "user_agent", "referrer", "filename")}
            self._open_add(url, from_catch=True, extra=extra)

    def _on_clip_url(self, url):
        if self._dialog_open:
            return
        self._open_add(url, from_catch=True)   # popup only; the GUI stays put until Download

    def _toggle_monitor(self):
        on = self.mon_var.get()
        self.settings["clipboard_monitor"] = on
        save_settings(self.settings)
        self.clip.start() if on else self.clip.stop()

    def _toggle_all(self):
        self.clip.monitor_all = self.all_var.get()
        self.settings["clipboard_all"] = self.all_var.get()
        save_settings(self.settings)

    def _toggle_close_to_tray(self):
        self.settings["close_to_tray"] = self.tray_var.get()
        save_settings(self.settings)

    def _bring_to_front(self):
        """Force the window to the foreground (lift() alone can't on Windows)."""
        w = self.master
        try:
            w.deiconify()
            try:
                w.state("normal")           # un-minimize if iconified
            except tk.TclError:
                pass
            w.attributes("-topmost", True)  # topmost toggle = the reliable raise-above-others trick
            w.lift()
            w.focus_force()
            self._topmost_after = w.after(400, self._drop_topmost)
        except tk.TclError:
            pass

    def _drop_topmost(self):
        try:
            self.master.attributes("-topmost", False)
        except tk.TclError:
            pass

    def _on_window_close(self):
        """X / close: hide to the tray and keep running, if the tray is up."""
        if self.tray is not None and self.tray.ok and self.tray_var.get():
            try:
                self.master.withdraw()
                return
            except tk.TclError:
                pass
        self._real_quit()

    def _real_quit(self):
        self.stop()
        self.manager.pause_all()
        self.manager.save_queue()
        try:
            self.master.destroy()
        except tk.TclError:
            pass

    def _tray_drain(self):
        """Act on tray clicks (flags set from the tray thread) on the Tk thread."""
        if self._tray_quit_evt.is_set():
            self._real_quit()
            return
        if self._tray_open_evt.is_set():
            self._tray_open_evt.clear()
            self._bring_to_front()
        self._tray_after_id = self.after(150, self._tray_drain)

    def on_resume(self):
        d = self._selected()
        if d and d.status in (Status.PAUSED, Status.ERROR):
            d.start()

    def on_resume_all(self):
        for d in self.manager.downloads:
            if d.status in (Status.PAUSED, Status.ERROR):
                d.start()

    def on_pause(self):
        d = self._selected()
        if d and d.is_active():
            d.pause()

    def on_remove(self):
        d = self._selected()
        if not d:
            return
        delete = False
        if d.status is not Status.COMPLETE:
            delete = messagebox.askyesno(
                "Remove", f"Remove “{d._resolved_name}”?\n\nAlso delete the partial file?",
                parent=self.master)
        self.manager.remove(d, delete_files=delete)
        for iid, dd in list(self._rows.items()):
            if dd is d:
                self.tree.delete(iid)
                del self._rows[iid]
        self._on_select()

    def on_clear_completed(self):
        for iid, d in list(self._rows.items()):
            if d.status is Status.COMPLETE:
                self.manager.remove(d)
                self.tree.delete(iid)
                del self._rows[iid]
        self._on_select()

    def on_copy_url(self):
        d = self._selected()
        if d:
            self.clipboard_clear()
            self.clipboard_append(d.url)
            self.clip.ignore(d.url)

    def on_open(self):
        d = self._selected()
        if not d:
            return
        try:
            if sys.platform == "win32":
                if d.status is Status.COMPLETE and os.path.exists(d.final_path):
                    subprocess.run(["explorer", "/select,", os.path.normpath(d.final_path)])
                else:
                    os.startfile(d.dest_dir)  # noqa: S606
            elif sys.platform == "darwin":
                subprocess.run(["open", d.dest_dir])
            else:
                subprocess.run(["xdg-open", d.dest_dir])
        except OSError as exc:
            messagebox.showerror("Open folder", str(exc), parent=self.master)

    def _about(self):
        messagebox.showinfo(
            "About SDM",
            "SDM — Segmented Download Manager\n\n"
            "An original, stdlib-only parallel downloader with pause/resume.\n"
            "Uses the HTTP Range mechanism; no system-wide interception.",
            parent=self.master)

    # ---- selection + refresh --------------------------------------------
    def _on_select(self):
        d = self._selected()
        self._update_actions()
        self.seg_bar.show(d)
        if not d:
            self.d_title.config(text="No download selected")
            self.d_meta.config(text="")
            self.d_url.config(text="")
            return
        self.d_title.config(text=d._resolved_name)
        self._update_details(d)

    def _update_details(self, d: Download):
        conns = len(d.segments) or 1
        parts = [
            f"{STATUS_TEXT.get(d.status, d.status.value)}",
            f"{human_bytes(d.downloaded())} / {human_bytes(d.size)}",
            f"{conns} connection(s)",
            "resumable" if d.supports_ranges else "no range support",
        ]
        if d.status is Status.RUNNING:
            parts.insert(2, f"{human_bytes(d.speed)}/s")
            parts.insert(3, f"ETA {human_time(d.eta())}")
        if d.status is Status.ERROR and d.error:
            parts.append(f"— {d.error}")
        self.d_meta.config(text="   •   ".join(parts))
        self.d_url.config(text=f"{d.url}\n→ {d.final_path}")

    def _update_actions(self):
        d = self._selected()
        can_resume = bool(d and d.status in (Status.PAUSED, Status.ERROR))
        can_pause = bool(d and d.is_active())
        self.btn_resume.state(["!disabled"] if can_resume else ["disabled"])
        self.btn_pause.state(["!disabled"] if can_pause else ["disabled"])
        self.btn_remove.state(["!disabled"] if d else ["disabled"])
        self.btn_open.state(["!disabled"] if d else ["disabled"])

    def _refresh(self):
        active = 0
        total_speed = 0.0
        for iid, d in self._rows.items():
            if d.status is Status.RUNNING:
                active += 1
                total_speed += d.speed
            status = STATUS_TEXT.get(d.status, d.status.value)
            if d.status is Status.ERROR and d.error:
                status = "Error"
            tags = ("odd" if self.tree.index(iid) % 2 else "even",)
            if d.status in STATUS_TAG:
                tags = tags + (STATUS_TAG[d.status],)
            self.tree.item(iid, tags=tags, values=(
                d._resolved_name,
                human_bytes(d.size),
                self._progress_cell(d),
                (human_bytes(d.speed) + "/s") if d.status is Status.RUNNING else "",
                human_time(d.eta()) if d.status is Status.RUNNING else "",
                status,
            ))
        sel = self._selected()
        if sel:
            self.seg_bar.show(sel)
            self._update_details(sel)
        self._update_actions()
        self.manager.save_queue()
        clip_hint = "   —   clipboard monitor on" if self.clip.enabled else ""
        if active:
            self.status_bar.config(
                text=f"●  {active} active  —  {human_bytes(total_speed)}/s total{clip_hint}")
        else:
            done = sum(1 for d in self._rows.values() if d.status is Status.COMPLETE)
            self.status_bar.config(
                text=f"Ready  —  {len(self._rows)} in list, {done} complete{clip_hint}")
        self._after_id = self.after(REFRESH_MS, self._refresh)


def main(argv=None) -> int:
    global UI_SCALE
    UI_SCALE = enable_hidpi()
    manager = Manager()
    manager.load_queue()
    root = tk.Tk()
    root.title("SDM — Segmented Download Manager")
    root.geometry(f"{px(940)}x{px(600)}")
    root.minsize(px(720), px(460))
    apply_theme(root)
    app = App(root, manager)   # installs its own WM_DELETE_WINDOW handler (close-to-tray)
    root.mainloop()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

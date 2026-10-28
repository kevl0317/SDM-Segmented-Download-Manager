"""Native-messaging host.

The browser launches SDM (with the extension origin as an argument) to speak
the Chrome/Firefox native-messaging protocol over stdio. This reads those
messages and forwards each download request to the running SDM app over the
local IPC socket -- starting SDM if it isn't running. SDM routes here from
__main__ when launched with an extension-origin argument.

Protocol: each message is a little-endian uint32 length followed by UTF-8 JSON.
"""
from __future__ import annotations

import json
import os
import socket
import struct
import subprocess
import sys
import time

from . import ipc

MAX_IN = 1 << 22


def _binary_stdio():
    # Windows would otherwise translate newlines and corrupt the framing.
    if sys.platform == "win32":
        try:
            import msvcrt
            msvcrt.setmode(sys.stdin.fileno(), os.O_BINARY)
            msvcrt.setmode(sys.stdout.fileno(), os.O_BINARY)
        except Exception:   # noqa: BLE001
            pass


def _read_message():
    header = sys.stdin.buffer.read(4)
    if len(header) < 4:
        return None
    (length,) = struct.unpack("<I", header)
    if length <= 0 or length > MAX_IN:
        return None
    data = sys.stdin.buffer.read(length)
    if len(data) < length:
        return None
    return json.loads(data.decode("utf-8"))


def _send_message(obj) -> bool:
    try:
        data = json.dumps(obj).encode("utf-8")
        sys.stdout.buffer.write(struct.pack("<I", len(data)))
        sys.stdout.buffer.write(data)
        sys.stdout.buffer.flush()
        return True
    except OSError:
        return False


def _try_send(port, req) -> bool:
    with socket.create_connection(("127.0.0.1", port), timeout=5) as s:
        s.sendall((json.dumps(req) + "\n").encode("utf-8"))
        s.settimeout(5)
        s.recv(256)
    return True


def _launch_sdm():
    try:
        if getattr(sys, "frozen", False):
            subprocess.Popen([sys.executable], close_fds=True)         # SDM.exe -> GUI
        else:
            subprocess.Popen([sys.executable, "-m", "sdm"], close_fds=True)
    except OSError:
        pass


def _forward(req) -> bool:
    def send(port):
        # read the token fresh each try: a just-launched SDM writes a new one
        return _try_send(port, dict(req, token=ipc.read_token()))

    port = ipc.read_port()
    if port is not None:
        try:
            return send(port)
        except OSError:
            pass
    _launch_sdm()                       # not running -> start SDM, then retry
    for _ in range(50):                 # up to ~10s for it to come up
        time.sleep(0.2)
        port = ipc.read_port()
        if port is None:
            continue
        try:
            return send(port)
        except OSError:
            continue
    return False


EXTENSION_ID = "mbaoofinhcijeneijhaglnjonhikbojj"
HOST_NAME = "com.sdm.catch"
_NMH_KEYS = (
    r"Software\Google\Chrome\NativeMessagingHosts" "\\" + HOST_NAME,
    r"Software\Microsoft\Edge\NativeMessagingHosts" "\\" + HOST_NAME,
)


def _manifest_path():
    return os.path.join(ipc.config_dir(), HOST_NAME + ".json")


def register(exe_path=None) -> bool:
    """Register SDM as the native-messaging host for Chrome and Edge (per-user)."""
    if sys.platform != "win32":
        return False
    manifest = {
        "name": HOST_NAME,
        "description": "SDM native messaging host",
        "path": exe_path or sys.executable,
        "type": "stdio",
        "allowed_origins": ["chrome-extension://%s/" % EXTENSION_ID],
    }
    path = _manifest_path()
    try:
        with open(path, "w", encoding="utf-8") as f:
            json.dump(manifest, f, indent=2)
    except OSError:
        return False
    import winreg
    ok = False
    for base in _NMH_KEYS:
        try:
            with winreg.CreateKey(winreg.HKEY_CURRENT_USER, base) as k:
                winreg.SetValueEx(k, "", 0, winreg.REG_SZ, path)   # (Default) = manifest path
            ok = True
        except OSError:
            pass
    return ok


def unregister() -> bool:
    if sys.platform != "win32":
        return False
    import winreg
    for base in _NMH_KEYS:
        try:
            winreg.DeleteKey(winreg.HKEY_CURRENT_USER, base)
        except OSError:
            pass
    try:
        os.remove(_manifest_path())
    except OSError:
        pass
    return True


def main(argv=None) -> int:
    _binary_stdio()
    while True:
        try:
            msg = _read_message()
        except Exception:   # noqa: BLE001
            break
        if msg is None:
            break
        url = msg.get("url") if isinstance(msg, dict) else None
        if not url:
            if not _send_message({"ok": False, "error": "no url"}):
                break
            continue
        req = {
            "url": url,
            "filename": msg.get("filename"),
            "cookies": msg.get("cookies"),
            "user_agent": msg.get("userAgent") or msg.get("user_agent"),
            "referrer": msg.get("referrer"),
        }
        ok = _forward(req)
        if not _send_message({"ok": bool(ok)}):
            break
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

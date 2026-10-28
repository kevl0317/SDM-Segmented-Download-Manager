"""Local IPC listener so external helpers (the browser native-messaging host)
can hand download requests to the running SDM app.

A tiny newline-delimited-JSON TCP server bound to 127.0.0.1 on an ephemeral
port. The port and a per-run random token are written under ~/.sdm/ so the
native host (same user) can find them; every request must carry the token, and
the server only listens on loopback. Each request is one JSON object and
on_request(dict) is called from a worker thread -- marshal to the GUI thread
yourself.
"""
from __future__ import annotations

import json
import os
import secrets
import socket
import threading

from .manager import config_dir

PORT_FILE = os.path.join(config_dir(), "ipc.port")
TOKEN_FILE = os.path.join(config_dir(), "ipc.token")
MAX_MSG = 1 << 20


def read_token() -> str:
    try:
        with open(TOKEN_FILE, "r", encoding="utf-8") as f:
            return f.read().strip()
    except OSError:
        return ""


def read_port():
    try:
        with open(PORT_FILE, "r", encoding="utf-8") as f:
            return int(f.read().strip())
    except (OSError, ValueError):
        return None


class IpcServer:
    """Loopback JSON request server. Token-guarded; localhost only."""

    def __init__(self, on_request):
        self.on_request = on_request
        self.port = None
        self.token = secrets.token_hex(16)
        self._sock = None
        self._thread = None
        self._stop = False

    def start(self) -> bool:
        try:
            s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            s.bind(("127.0.0.1", 0))
            s.listen(16)
        except OSError:
            return False
        self._sock = s
        self.port = s.getsockname()[1]
        self._write(PORT_FILE, str(self.port))
        self._write(TOKEN_FILE, self.token)
        self._thread = threading.Thread(target=self._serve, name="sdm-ipc", daemon=True)
        self._thread.start()
        return True

    @staticmethod
    def _write(path, value):
        try:
            with open(path, "w", encoding="utf-8") as f:
                f.write(value)
        except OSError:
            pass

    def _serve(self):
        while not self._stop:
            try:
                conn, _ = self._sock.accept()
            except OSError:
                break
            threading.Thread(target=self._handle, args=(conn,), daemon=True).start()

    def _handle(self, conn):
        try:
            conn.settimeout(5)
            buf = b""
            while b"\n" not in buf and len(buf) < MAX_MSG:
                chunk = conn.recv(65536)
                if not chunk:
                    break
                buf += chunk
            msg = json.loads(buf.split(b"\n", 1)[0].decode("utf-8"))
            if not isinstance(msg, dict) or not msg.get("url"):
                conn.sendall(b'{"ok":false,"error":"bad request"}\n')
                return
            if msg.get("token") != self.token:
                conn.sendall(b'{"ok":false,"error":"bad token"}\n')
                return
            try:
                self.on_request(msg)
            except Exception:   # noqa: BLE001
                pass
            conn.sendall(b'{"ok":true}\n')
        except Exception:   # noqa: BLE001
            try:
                conn.sendall(b'{"ok":false}\n')
            except OSError:
                pass
        finally:
            try:
                conn.close()
            except OSError:
                pass

    def stop(self):
        self._stop = True
        try:
            if self._sock:
                self._sock.close()
        except OSError:
            pass
        for p in (PORT_FILE, TOKEN_FILE):
            try:
                if os.path.exists(p):
                    os.remove(p)
            except OSError:
                pass

"""SDM - a small, original segmented download manager (stdlib only).

Clean-room; implements HTTP Range-based parallel downloading with pause/resume.
No kernel drivers, no traffic interception, no browser download-hijacking.
"""
from .clipboard import ClipboardMonitor, extract_download_url
from .engine import Download, Segment, Status
from .manager import Manager, default_download_dir

__version__ = "1.2.1"
__all__ = ["Download", "Segment", "Status", "Manager", "default_download_dir",
           "ClipboardMonitor", "extract_download_url"]

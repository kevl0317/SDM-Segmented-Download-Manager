"""SDM - a small, original segmented download manager (stdlib only).

Clean-room; implements HTTP Range-based parallel downloading with pause/resume.
No kernel drivers, no traffic interception, no browser download-hijacking.
"""
from .engine import Download, Segment, Status
from .manager import Manager, default_download_dir

__version__ = "1.0.0"
__all__ = ["Download", "Segment", "Status", "Manager", "default_download_dir"]

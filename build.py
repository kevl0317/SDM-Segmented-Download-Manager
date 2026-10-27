"""Build a standalone, dependency-free sdm.pyz using the stdlib zipapp module.

    python build.py

Produces dist/sdm.pyz, runnable with any Python 3.9+ that has Tkinter:
    python dist/sdm.pyz          (GUI)
    python dist/sdm.pyz <url>    (CLI)
"""
import shutil
import tempfile
import zipapp
from pathlib import Path

ROOT = Path(__file__).resolve().parent
PKG = ROOT / "sdm"
DIST = ROOT / "dist"


def main() -> None:
    DIST.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        stage = Path(tmp)
        shutil.copytree(PKG, stage / "sdm",
                        ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))
        (stage / "__main__.py").write_text(
            "import sys\nfrom sdm.__main__ import main\nsys.exit(main())\n",
            encoding="utf-8")
        target = DIST / "sdm.pyz"
        zipapp.create_archive(stage, target=str(target), interpreter=None)
    size = target.stat().st_size
    print(f"built {target}  ({size:,} bytes)")
    print("run:  python dist/sdm.pyz         (GUI)")
    print("      python dist/sdm.pyz <url>   (CLI)")


if __name__ == "__main__":
    main()

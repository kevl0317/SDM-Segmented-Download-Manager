"""Entry script for PyInstaller (it bundles from a script, not `-m`)."""
import sys

from sdm.__main__ import main

if __name__ == "__main__":
    sys.exit(main())

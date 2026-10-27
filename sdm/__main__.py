"""Entry point.

  python -m sdm                 launch the GUI
  python -m sdm gui             launch the GUI
  python -m sdm <url> [opts]    headless CLI download
"""
import sys


def main() -> int:
    args = sys.argv[1:]
    if not args or args[0] == "gui":
        from .gui import main as gui_main
        return gui_main()
    from .cli import main as cli_main
    return cli_main(args)


if __name__ == "__main__":
    raise SystemExit(main())

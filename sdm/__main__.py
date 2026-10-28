"""Entry point.

  python -m sdm                 launch the GUI
  python -m sdm gui             launch the GUI
  python -m sdm <url> [opts]    headless CLI download
"""
import sys


def _is_native_host(args) -> bool:
    # The browser launches the host with the extension's origin as an argument.
    return any(a.startswith(("chrome-extension://", "moz-extension://")) or a == "--native-host"
               for a in args)


def main() -> int:
    args = sys.argv[1:]
    if args and args[0] in ("--register-host", "--unregister-host"):
        from .nativehost import register, unregister
        ok = register() if args[0] == "--register-host" else unregister()
        return 0 if ok else 1
    if _is_native_host(args):
        from .nativehost import main as host_main
        return host_main(args)
    if not args or args[0] == "gui":
        from .gui import main as gui_main
        return gui_main()
    from .cli import main as cli_main
    return cli_main(args)


if __name__ == "__main__":
    raise SystemExit(main())

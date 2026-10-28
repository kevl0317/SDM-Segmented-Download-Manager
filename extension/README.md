# SDM browser extension

Catches download links you **click** in a browser (or right-click → **Download
with SDM**) and hands them to the SDM app — the clean "browser hand-off" over
the standard WebExtension native-messaging API (no traffic interception).

Works in Chrome and Edge (Manifest V3).

## Install

1. **Install SDM** (`SDM-Setup-*.exe`). The installer registers the native host
   automatically. (To register manually, run `SDM.exe --register-host` once;
   `--unregister-host` removes it.)
2. **Load this folder** in your browser:
   - Chrome: `chrome://extensions` → enable **Developer mode** → **Load unpacked**
     → pick this `extension/` folder.
   - Edge: `edge://extensions` → **Developer mode** → **Load unpacked**.
   - The extension ID is fixed (`mbaoofinhcijeneijhaglnjonhikbojj`), so the native
     host already allows it — no ID copying needed.
3. **Use it:** click a download link, or right-click → **Download with SDM**. A
   small SDM popup appears centered on screen; click **Download**.

Click the toolbar icon to **pause/resume** auto-catching of browser downloads
(the context-menu item always works).

## How it works

The extension collects the URL plus the page's cookies, `User-Agent`, and
referrer, and sends them to the native host `com.sdm.catch` — which is `SDM.exe`
launched by the browser with the extension origin. The host forwards the request
to the already-running SDM over a **token-guarded loopback socket** (starting SDM
if needed); SDM shows the catch popup and downloads the file with those headers,
so authenticated/session downloads work.

Files: `manifest.json`, `background.js` (service worker), `icon*.png`.

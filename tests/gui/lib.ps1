# Driver for the SDM GUI end-to-end tests (run.ps1 dot-sources this file).
# Everything runs against a private profile; nothing touches the user's mouse,
# keyboard focus, or real SDM. Clicks are posted to the window under test.
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Runtime.WindowsRuntime
if (-not ("E2E" -as [type])) {
Add-Type @"
using System; using System.Text; using System.Collections.Generic; using System.Runtime.InteropServices;
public class E2E {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint f);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern IntPtr GetMenu(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetSubMenu(IntPtr m, int pos);
  [DllImport("user32.dll")] public static extern uint GetMenuItemID(IntPtr m, int pos);
  [DllImport("user32.dll")] public static extern int GetMenuItemCount(IntPtr m);
  [DllImport("user32.dll")] public static extern uint GetMenuState(IntPtr m, uint id, uint flags);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetMenuString(IntPtr m, uint id, StringBuilder s, int n, uint flags);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint f);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern int GetWindowLong(IntPtr h, int idx);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern uint RegisterWindowMessage(string s);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr LoadLibrary(string p);
  [DllImport("kernel32.dll", CharSet=CharSet.Ansi)] public static extern IntPtr GetProcAddress(IntPtr m, string n);
  [DllImport("user32.dll")] public static extern IntPtr SetWindowsHookEx(int id, IntPtr proc, IntPtr mod, uint tid);
  [DllImport("user32.dll")] public static extern bool UnhookWindowsHookEx(IntPtr h);

  public static List<IntPtr> Windows(uint pid) {
    var all = new List<IntPtr>();
    EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); if (p == pid) all.Add(h); return true; }, IntPtr.Zero);
    return all;
  }
  public static string Title(IntPtr h) { var sb = new StringBuilder(512); GetWindowText(h, sb, 512); return sb.ToString(); }
  public static string Class(IntPtr h) { var sb = new StringBuilder(256); GetClassName(h, sb, 256); return sb.ToString(); }
  // A visible window of this process by exact title.
  public static IntPtr Find(uint pid, string title) {
    foreach (var h in Windows(pid)) if (IsWindowVisible(h) && Title(h) == title) return h;
    return IntPtr.Zero;
  }
  // The main window, even hidden: it's the one with a menu bar.
  public static IntPtr FindMain(uint pid) {
    foreach (var h in Windows(pid)) if (GetMenu(h) != IntPtr.Zero && Title(h) == "SDM") return h;
    return IntPtr.Zero;
  }
  public static IntPtr InstallHook(string dll, IntPtr hwnd) {
    var mod = LoadLibrary(dll); var proc = GetProcAddress(mod, "hook"); uint pid; uint tid = GetWindowThreadProcessId(hwnd, out pid);
    return SetWindowsHookEx(3 /* WH_GETMESSAGE */, proc, mod, tid);
  }
  static IntPtr LP(int x, int y) { return (IntPtr)(((y & 0xFFFF) << 16) | (x & 0xFFFF)); }
  public static void Hover(IntPtr h, int x, int y) { PostMessage(h, 0x0200, IntPtr.Zero, LP(x, y)); }
  public static void Click(IntPtr h, int x, int y) {
    PostMessage(h, 0x0200, IntPtr.Zero, LP(x, y));
    PostMessage(h, 0x0201, (IntPtr)1, LP(x, y));
    PostMessage(h, 0x0202, IntPtr.Zero, LP(x, y));
  }
  // Wheel: screen coordinates, positive = up. One notch is 120.
  public static void Wheel(IntPtr h, int cx, int cy, int delta) {
    POINT p; p.X = cx; p.Y = cy; ClientToScreen(h, ref p);
    PostMessage(h, 0x0200, IntPtr.Zero, LP(cx, cy));
    PostMessage(h, 0x020A, (IntPtr)((delta & 0xFFFF) << 16), LP(p.X, p.Y));
  }
  // VK_F24 makes no character of its own; the WM_CHAR after it is the text.
  public static void Type(IntPtr h, string s) {
    foreach (char c in s) {
      PostMessage(h, 0x0100, (IntPtr)0x87, (IntPtr)0x00760001);
      PostMessage(h, 0x0102, (IntPtr)c, (IntPtr)0x00760001);
      PostMessage(h, 0x0101, (IntPtr)0x87, (IntPtr)unchecked((int)0xC0760001));
    }
  }
  public static void Key(IntPtr h, int vk) {
    PostMessage(h, 0x0100, (IntPtr)vk, (IntPtr)1);
    PostMessage(h, 0x0101, (IntPtr)vk, (IntPtr)unchecked((int)0xC0000001));
  }
  public static void Close(IntPtr h) { PostMessage(h, 0x0010, IntPtr.Zero, IntPtr.Zero); }
  public static uint MenuId(IntPtr h, int top, int sub, int subsub) {
    var m = GetSubMenu(GetMenu(h), top);
    if (subsub < 0) return GetMenuItemID(m, sub);
    return GetMenuItemID(GetSubMenu(m, sub), subsub);
  }
  public static bool MenuChecked(IntPtr h, int top, int sub, int subsub) {
    uint id = MenuId(h, top, sub, subsub);
    return (GetMenuState(GetMenu(h), id, 0) & 8) != 0;
  }
  public static void Command(IntPtr h, uint id) { PostMessage(h, 0x0111, (IntPtr)id, IntPtr.Zero); }
  public static string MenuText(IntPtr h, int top, int sub, int subsub) {
    var sb = new StringBuilder(256); GetMenuString(GetMenu(h), MenuId(h, top, sub, subsub), sb, 256, 0); return sb.ToString();
  }
  public static int[] Rect(IntPtr h) { RECT r; GetWindowRect(h, out r); return new[] { r.Left, r.Top, r.Right - r.Left, r.Bottom - r.Top }; }
  public static int[] Client(IntPtr h) { RECT r; GetClientRect(h, out r); return new[] { r.Right, r.Bottom }; }
  public static int[] ClientOrigin(IntPtr h) { RECT r; GetWindowRect(h, out r); POINT p; p.X = 0; p.Y = 0; ClientToScreen(h, ref p); return new[] { p.X - r.Left, p.Y - r.Top }; }
  public static bool Topmost(IntPtr h) { return (GetWindowLong(h, -20) & 0x8) != 0; }
  public static System.Drawing.Bitmap Capture(IntPtr h) {
    RECT r; GetWindowRect(h, out r);
    var bmp = new System.Drawing.Bitmap(r.Right - r.Left, r.Bottom - r.Top);
    using (var g = System.Drawing.Graphics.FromImage(bmp)) { var dc = g.GetHdc(); PrintWindow(h, dc, 2); g.ReleaseHdc(dc); }
    return bmp;
  }
}
"@ -ReferencedAssemblies System.Drawing
}
# See windows at their real size on any monitor (-4: per-monitor aware): assuming
# one scale, captures cut off part of a window on a scaled monitor.
[E2E]::SetThreadDpiAwarenessContext([IntPtr](-4)) | Out-Null


# ---- OCR: read what is on screen, and where ------------------------------
$null = [Windows.Media.Ocr.OcrEngine, Windows.Foundation, ContentType = WindowsRuntime]
$null = [Windows.Graphics.Imaging.BitmapDecoder, Windows.Foundation, ContentType = WindowsRuntime]
$null = [Windows.Graphics.Imaging.SoftwareBitmap, Windows.Foundation, ContentType = WindowsRuntime]
$null = [Windows.Storage.Streams.InMemoryRandomAccessStream, Windows.Storage.Streams, ContentType = WindowsRuntime]
$null = [Windows.Storage.Streams.DataWriter, Windows.Storage.Streams, ContentType = WindowsRuntime]
$script:asTaskOp = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' })[0]
function Await-Op($op, [type]$type) {
    $t = $script:asTaskOp.MakeGenericMethod($type).Invoke($null, @($op))
    $t.Wait(-1) | Out-Null
    $t.Result
}
$script:ocrEngine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()

# OCR a Bitmap at 2x; returns the lines, each with its words and client-relative boxes.
function Read-Bitmap([System.Drawing.Bitmap]$bmp) {
    $scale = 2
    $big = New-Object System.Drawing.Bitmap ($bmp.Width * $scale), ($bmp.Height * $scale)
    $g = [System.Drawing.Graphics]::FromImage($big)
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.DrawImage($bmp, 0, 0, $big.Width, $big.Height)
    $g.Dispose()
    $ms = New-Object System.IO.MemoryStream
    $big.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $big.Dispose()
    $bytes = $ms.ToArray()
    $ms.Dispose()
    $stream = New-Object Windows.Storage.Streams.InMemoryRandomAccessStream
    $writer = New-Object Windows.Storage.Streams.DataWriter ($stream.GetOutputStreamAt(0))
    $writer.WriteBytes($bytes)
    $null = Await-Op ($writer.StoreAsync()) ([uint32])
    $writer.DetachStream() | Out-Null
    $stream.Seek(0)
    $decoder = Await-Op ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder])
    $soft = Await-Op ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])
    $res = Await-Op ($script:ocrEngine.RecognizeAsync($soft)) ([Windows.Media.Ocr.OcrResult])
    foreach ($line in $res.Lines) {
        [pscustomobject]@{
            Text  = $line.Text
            Words = @($line.Words | ForEach-Object {
                [pscustomobject]@{
                    Text = $_.Text
                    X = [int]($_.BoundingRect.X / $scale); Y = [int]($_.BoundingRect.Y / $scale)
                    W = [int]($_.BoundingRect.Width / $scale); H = [int]($_.BoundingRect.Height / $scale)
                }
            })
        }
    }
}

# The window's client area as a bitmap, and its OCR with client coordinates.
function Get-ClientBitmap([IntPtr]$h) {
    $full = [E2E]::Capture($h)
    $o = [E2E]::ClientOrigin($h)
    $c = [E2E]::Client($h)
    $rect = New-Object System.Drawing.Rectangle $o[0], $o[1], ([Math]::Min($c[0], $full.Width - $o[0])), ([Math]::Min($c[1], $full.Height - $o[1]))
    $crop = $full.Clone($rect, $full.PixelFormat)
    $full.Dispose()
    $crop
}
function Read-Window([IntPtr]$h) {
    $b = Get-ClientBitmap $h
    Read-Bitmap $b
    $b.Dispose()
}
function Get-WindowText([IntPtr]$h) { (Read-Window $h | ForEach-Object { $_.Text }) -join "`n" }

# Click the first on-screen word equal to $word (optionally on the line containing $near).
function Click-Word([IntPtr]$h, [string]$word, [string]$near = "") {
    # Exact (with case) first; then ignoring what OCR tends to misread in names.
    $plain = { param($s) ($s -replace '[^A-Za-z0-9]', '').ToLowerInvariant() }
    $lines = @(Read-Window $h)
    foreach ($exact in $true, $false) {
        foreach ($line in $lines) {
            if ($near -and $line.Text -notlike "*$near*") { continue }
            foreach ($w in $line.Words) {
                $same = if ($exact) { $w.Text -ceq $word } else { (& $plain $w.Text) -eq (& $plain $word) }
                if ($same) {
                    [E2E]::Click($h, $w.X + [int]($w.W / 2), $w.Y + [int]($w.H / 2))
                    return $true
                }
            }
        }
    }
    $false
}

function Save-Shot([IntPtr]$h, [string]$name) {
    if (-not (Test-Path $script:shots)) { New-Item -ItemType Directory -Path $script:shots | Out-Null }
    $p = Join-Path $script:shots "$name.png"
    $b = [E2E]::Capture($h)
    $b.Save($p, [System.Drawing.Imaging.ImageFormat]::Png)
    $b.Dispose()
    $p
}

# ---- the app under test ----------------------------------------------------
function Start-Sdm([string]$cfg, [string]$dl, [string[]]$extra = @()) {
    New-Item -ItemType Directory -Force -Path $cfg, $dl | Out-Null
    $env:SDM_CONFIG_DIR = $cfg
    $env:SDM_DOWNLOAD_DIR = $dl
    $p = if ($extra.Count) { Start-Process -FilePath $script:exe -ArgumentList $extra -PassThru -WindowStyle Hidden } else { Start-Process -FilePath $script:exe -PassThru }
    $p
}
function Wait-Window([uint32]$procId, [string]$title, [int]$ms = 20000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) {
        $h = [E2E]::Find($procId, $title)
        if ($h -ne [IntPtr]::Zero) { return $h }
        Start-Sleep -Milliseconds 100
    }
    [IntPtr]::Zero
}
function Wait-Main([uint32]$procId, [int]$ms = 20000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) {
        $h = [E2E]::FindMain($procId)
        if ($h -ne [IntPtr]::Zero) { return $h }
        Start-Sleep -Milliseconds 100
    }
    [IntPtr]::Zero
}
# Poll until the window's text matches, or give up; returns the last text seen.
function Wait-Text([IntPtr]$h, [string]$pattern, [int]$ms = 15000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    do {
        $t = Get-WindowText $h
        if ($t -match $pattern) { return $t }
        Start-Sleep -Milliseconds 250
    } while ($sw.ElapsedMilliseconds -lt $ms)
    $t
}
# Hand the running test instance a catch over its loopback IPC, as the browser host does.
function Send-Ipc([string]$cfg, [hashtable]$body) {
    $port = [int](Get-Content (Join-Path $cfg "ipc-port") -Raw).Trim()
    $token = (Get-Content (Join-Path $cfg "ipc-token") -Raw).Trim()
    $body.token = $token
    $json = $body | ConvertTo-Json -Compress
    $c = New-Object System.Net.Sockets.TcpClient
    $c.Connect("127.0.0.1", $port)
    $s = $c.GetStream()
    $b = [Text.Encoding]::UTF8.GetBytes($json + "`n")
    $s.Write($b, 0, $b.Length)
    $s.ReadTimeout = 3000
    $buf = New-Object byte[] 64
    $ack = ""
    try { $n = $s.Read($buf, 0, 64); $ack = [Text.Encoding]::UTF8.GetString($buf, 0, $n) } catch {}
    $c.Close()
    $ack
}
function Stop-Sdm($p) {
    if ($p -and -not $p.HasExited) {
        $p.Kill()
        $p.WaitForExit(3000) | Out-Null
    }
}

# ---- results ---------------------------------------------------------------
$script:results = New-Object System.Collections.ArrayList
function Check([string]$area, [string]$name, [bool]$ok, [string]$detail = "") {
    $r = [pscustomobject]@{ Area = $area; Name = $name; Ok = $ok; Detail = $detail }
    [void]$script:results.Add($r)
    $tag = if ($ok) { "PASS" } else { "FAIL" }
    Write-Output ("[{0}] {1}: {2}{3}" -f $tag, $area, $name, $(if ($detail) { " -- $detail" } else { "" }))
}

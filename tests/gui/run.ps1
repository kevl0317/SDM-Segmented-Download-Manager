<#
.SYNOPSIS
End-to-end tests of the SDM app: every state, action and failure, on the real
binary, with assertions on what's on screen (read back by OCR) and on disk.

.DESCRIPTION
Builds the app, the CLI and the fault server, then drives the app with posted
window messages against a local server that misbehaves on request. Everything runs
on private profiles under target\gui-tests, and the mouse and keyboard stay yours,
but windows do come and go on screen, so it's best run while you're away. Close any
real SDM first: the clipboard section copies links, which it would catch too (your
clipboard's text is put back afterwards).

The default software renderer is what a capture can read in any session. It doesn't
draw the segment bars or the logo's arrow, so look at those on a normal GPU run.

Screenshots of each state go to target\gui-tests\shots; sheet.ps1 makes contact
sheets of them. Doesn't cover --register-host (it changes the real browser
registration), real tray clicks, or real drags between monitors.

.EXAMPLE
.\tests\gui\run.ps1
.EXAMPLE
.\tests\gui\run.ps1 -Only popup,disconnect
#>
param([string[]]$Only = @(), [string]$Backend = "tiny-skia")
$ErrorActionPreference = "Stop"
$Only = @($Only | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
$source = $PSScriptRoot
$repo = Split-Path -Parent (Split-Path -Parent $source)
$here = Join-Path $repo "target\gui-tests"
New-Item -ItemType Directory -Force (Join-Path $here "app") | Out-Null

# Native tools print progress on stderr, which Windows PowerShell can turn into
# errors; judge them by their exit code instead.
function Invoke-Tool([string]$what, [scriptblock]$run) {
    $saved = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try { & $run 2>&1 | ForEach-Object { "$_" } | Out-Null } finally { $ErrorActionPreference = $saved }
    if ($LASTEXITCODE -ne 0) { throw "$what failed (exit code $LASTEXITCODE)" }
}
Push-Location $repo
Invoke-Tool "cargo build" { cargo build -p sdm-gui -p sdm-cli -p sdm-testkit }
Pop-Location
Invoke-Tool "the test hook" { rustc --edition 2021 --crate-type cdylib -C opt-level=2 (Join-Path $source "hook.rs") -o (Join-Path $here "sdmhook.dll") }
Invoke-Tool "the explorer stub" { rustc --edition 2021 -C opt-level=1 (Join-Path $source "explorer_stub.rs") -o (Join-Path $here "app\explorer.exe") }
# Beside the stub: Windows looks for explorer.exe in the app's own folder first.
Copy-Item (Join-Path $repo "target\debug\sdm-gui.exe") (Join-Path $here "app\sdm-gui.exe") -Force
$script:exe = Join-Path $here "app\sdm-gui.exe"

. (Join-Path $source "lib.ps1")
$script:hookDll = Join-Path $here "sdmhook.dll"
$script:shots = Join-Path $here "shots"
function Clear-Dir([string]$path) {
    # By long-path name: a test file's name can be longer than Remove-Item can handle.
    if (Test-Path -LiteralPath $path) { [IO.Directory]::Delete("\\?\$path", $true) }
}
if (-not $Only.Count) { Clear-Dir $script:shots }
if ($Backend) { $env:ICED_BACKEND = $Backend }
$faultExe = Join-Path $repo "target\debug\sdm-faultserver.exe"
$openLog = Join-Path $here "open.log"
$env:SDM_TEST_OPEN_LOG = $openLog

# ---- helpers ---------------------------------------------------------------
function Start-Fault {
    $psi = New-Object Diagnostics.ProcessStartInfo
    $psi.FileName = $faultExe
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $proc = [Diagnostics.Process]::Start($psi)
    $port = [int]$proc.StandardOutput.ReadLine()
    # .NET may start its stdin with a byte-order mark; let an empty line take it.
    $proc.StandardInput.WriteLine("")
    [pscustomobject]@{ Proc = $proc; Port = $port }
}
function Fault([string]$cmd) { $script:server.Proc.StandardInput.WriteLine($cmd) }
function U([string]$pathAndQuery) { "http://127.0.0.1:$($script:server.Port)$pathAndQuery" }
function Verify-File([string]$path, [uint64]$seed, [uint64]$size) {
    $o = & $faultExe --verify $path $seed $size
    ($o -join " ")
}

function Open-App([string]$name, [switch]$KeepProfile, [string[]]$Args2 = @(), [hashtable]$Settings = $null) {
    $base = Join-Path $here "p\$name"
    if (-not $KeepProfile) { Clear-Dir $base }
    $cfg = Join-Path $base "cfg"; $dl = Join-Path $base "dl"
    New-Item -ItemType Directory -Force -Path $cfg, $dl | Out-Null
    if ($Settings) { ($Settings | ConvertTo-Json) | Out-File -Encoding ascii (Join-Path $cfg "settings.json") }
    $p = Start-Sdm $cfg $dl $Args2
    $h = Wait-Main $p.Id
    if ($h -eq [IntPtr]::Zero) { throw "no main window for $name" }
    [void][E2E]::InstallHook($script:hookDll, $h)
    Start-Sleep -Milliseconds 700
    [pscustomobject]@{ P = $p; H = $h; Cfg = $cfg; Dl = $dl; Name = $name }
}
function Close-App($a) { Stop-Sdm $a.P }
function Menu($h, [int]$top, [int]$sub, [int]$subsub = -1) { [E2E]::Command($h, [E2E]::MenuId($h, $top, $sub, $subsub)) }
function Add-Link($a, [string]$url) {
    Menu $a.H 0 0
    Start-Sleep -Milliseconds 300
    [E2E]::Type($a.H, $url)
    Start-Sleep -Milliseconds 300
    [E2E]::Key($a.H, 0x0D)
    Start-Sleep -Milliseconds 500
}
function Shot($a, [string]$name) { [void](Save-Shot $a.H $name) }
function Text($a) { Get-WindowText $a.H }
function Row-Click($a, [string]$word) {
    # Click the file name on its row: selects or folds it.
    $ok = Click-Word $a.H $word
    Start-Sleep -Milliseconds 500
    $ok
}
function Files($a) { @(Get-ChildItem $a.Dl -Force | ForEach-Object { $_.Name }) }
function Wait-Until([scriptblock]$cond, [int]$ms = 15000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) {
        if (& $cond) { return $true }
        Start-Sleep -Milliseconds 100
    }
    $false
}
function Popup-Of($a, [int]$ms = 8000) { Wait-Window $a.P.Id ("SDM " + [char]0x2014 + " New download") $ms }
function Catch-Link($a, [string]$url, [string]$filename = "", [string]$cookies = "", [string]$referrer = "", [string]$ua = "") {
    Send-Ipc $a.Cfg @{ url = $url; filename = $filename; cookies = $cookies; referrer = $referrer; userAgent = $ua }
}
function Gone($p, [int]$ms = 8000) { $p.WaitForExit($ms) }

function Section([string]$name, [scriptblock]$body) {
    if ($Only.Count -and $Only -notcontains $name) { return }
    Write-Output "=== $name ==="
    try { & $body } catch {
        Check $name "section ran to the end" $false ($_.Exception.Message + " @ " + $_.InvocationInfo.ScriptLineNumber)
    }
    Get-Process | Where-Object { $_.Path -eq $script:exe } | ForEach-Object { try { $_.Kill() } catch {} }
    Start-Sleep -Milliseconds 300
}

$script:server = Start-Fault
Write-Output "fault server on port $($script:server.Port); app: $($script:exe); backend: $Backend"

# ---- sections ---------------------------------------------------------------
Section "startup" {
    $a = Open-App "startup"
    $h = $a.H
    Check "startup" "main window is visible" ([E2E]::IsWindowVisible($h))
    $t = Text $a
    Check "startup" "empty state shows" ($t -match "Paste a link above") $t
    Check "startup" "link box shows its hint" ($t -match "Paste a link to download")
    Check "startup" "Download button has its label" ($t -match "Download")
    Shot $a "01-empty-dark"
    $c = [E2E]::Client($h)
    $k = [E2E]::GetDpiForWindow($h) / 96
    Check "startup" "opens at its normal size" ([Math]::Abs($c[0] - 760 * $k) -le 2 -and $c[1] -ge 590 * $k) ($c -join "x")
    $labels = @(
        @(0,0,"New download"), @(0,1,"Open downloads folder"), @(0,3,"Exit"),
        @(1,0,"Resume all"), @(1,1,"Pause all"), @(1,3,"Clear completed"),
        @(2,0,"Show completed downloads"), @(2,1,"Always on top"),
        @(3,0,"Catch copied links"), @(3,1,"Catch all link types"), @(3,3,"Keep running in the tray when closed"), @(3,4,"Remove also deletes the file"),
        @(4,0,"About SDM"), @(4,1,"Project page")
    )
    foreach ($l in $labels) {
        $text = ([E2E]::MenuText($h, $l[0], $l[1], -1)) -replace '&','' -replace "`t.*",''
        Check "startup" "menu item: $($l[2])" ($text -eq $l[2]) $text
    }
    Check "startup" "default: Show completed on" ([E2E]::MenuChecked($h,2,0,-1))
    Check "startup" "default: Always on top off" (-not [E2E]::MenuChecked($h,2,1,-1))
    Check "startup" "default: Dark theme" ([E2E]::MenuChecked($h,2,3,0) -and -not [E2E]::MenuChecked($h,2,3,1))
    Check "startup" "default: Catch copied links on" ([E2E]::MenuChecked($h,3,0,-1))
    Check "startup" "default: Catch all off" (-not [E2E]::MenuChecked($h,3,1,-1))
    Check "startup" "default: Keep in tray on" ([E2E]::MenuChecked($h,3,3,-1))
    Check "startup" "default: Remove keeps the file" (-not [E2E]::MenuChecked($h,3,4,-1))
    Close-App $a
}

Section "linkbox" {
    $a = Open-App "linkbox"
    $h = $a.H
    # Nothing to add
    [void](Click-Word $h "Download")
    Start-Sleep -Milliseconds 500
    Check "linkbox" "Download with an empty box adds nothing" ((Text $a) -match "Paste a link above")
    Menu $h 0 0; Start-Sleep -Milliseconds 200
    [E2E]::Type($h, "   "); [E2E]::Key($h, 0x0D); Start-Sleep -Milliseconds 400
    Check "linkbox" "Enter on only spaces adds nothing" ((Text $a) -match "Paste a link above")
    # clear the spaces
    foreach ($i in 1..3) { [E2E]::Key($h, 0x08) }
    # Not links
    Add-Link $a "not a link"
    $t = Wait-Text $h "Failed|look like" 5000
    Shot $a "02-garbage-link"
    [void](Row-Click $a "download")
    $t = Text $a
    Check "linkbox" "garbage: says it isn't a link" ($t -match "look like a link") $t
    Shot $a "02b-garbage-link-selected"
    Add-Link $a "ftp://example.com/pub/file.zip"
    $t = Wait-Text $h "file.zip" 5000
    [void](Row-Click $a "file.zip")
    $t = Text $a
    Check "linkbox" "ftp: says only http and https" ($t -match "http and https") $t
    Shot $a "02c-ftp-link"
    # A real one, by Enter
    $size = 3000000
    Add-Link $a (U "/alpha.zip?size=$size&seed=5")
    Check "linkbox" "Enter downloads a link" (Wait-Until { (Text $a) -match "alpha.zip" -and (Test-Path (Join-Path $a.Dl "alpha.zip")) })
    Check "linkbox" "file is byte-exact" ((Verify-File (Join-Path $a.Dl "alpha.zip") 5 $size) -eq "OK")
    # And by the button
    Menu $h 0 0; Start-Sleep -Milliseconds 200
    [E2E]::Type($h, (U "/bravo.zip?size=$size&seed=6")); Start-Sleep -Milliseconds 300
    [void](Click-Word $h "Download")
    Check "linkbox" "the Download button downloads a link" (Wait-Until { Test-Path (Join-Path $a.Dl "bravo.zip") })
    Check "linkbox" "file is byte-exact (button)" ((Verify-File (Join-Path $a.Dl "bravo.zip") 6 $size) -eq "OK")
    Start-Sleep -Milliseconds 600
    $t = Text $a
    Check "linkbox" "the box is empty again" ($t -match "Paste a link to download") $t
    Close-App $a
}

Section "states" {
    $a = Open-App "states"
    $h = $a.H
    # Connecting
    Add-Link $a (U "/charlie.bin?size=40000000&seed=9&rate=400000&delay_ms=2500&fault_probe=1")
    $t = Wait-Text $h "Connecting" 2500
    Check "states" "new download shows Connecting" ($t -match "Connecting") $t
    Shot $a "03-connecting"
    # Running
    $t = Wait-Text $h "[1-9][0-9.]*(KB|MB)/s" 12000
    Start-Sleep -Milliseconds 1000
    $t = Text $a
    Check "states" "running shows size and speed" ($t -match "MB / " -and $t -match "/s") $t
    Check "states" "running shows time left" ($t -match "left") $t
    Shot $a "04-running-unselected"
    Check "states" "no action buttons until selected" ($t -notmatch "Pause|Remove") $t
    # Select
    [void](Row-Click $a "charlie.bin")
    $t = Text $a
    Check "states" "selected running shows Pause, Open, Remove" ($t -match "Pause" -and $t -match "Open" -and $t -match "Remove") $t
    Shot $a "05-running-selected"
    # Fold
    [void](Row-Click $a "charlie.bin")
    $t = Text $a
    Check "states" "clicking again folds the buttons away" ($t -notmatch "Pause") $t
    [void](Row-Click $a "charlie.bin")
    # Pause
    Check "states" "Pause button can be clicked" (Click-Word $h "Pause")
    $t = Wait-Text $h "paused" 3000
    Check "states" "Pause takes effect fast" ($t -match "paused") $t
    Check "states" "paused offers Resume" ($t -match "Resume") $t
    Check "states" "paused has no speed" ($t -notmatch "/s") $t
    Shot $a "06-paused-selected"
    $sidecar = Join-Path $a.Dl "charlie.bin.sdmdownload.sdm"
    Check "states" "paused progress is saved beside the file" (Test-Path $sidecar)
    # Compact paused
    [void](Row-Click $a "charlie.bin")
    $t = Text $a
    Check "states" "paused row shows Paused" ($t -match "Paused") $t
    Shot $a "07-paused-compact"
    # Resume
    [void](Row-Click $a "charlie.bin")
    Check "states" "Resume button can be clicked" (Click-Word $h "Resume")
    $t = Wait-Text $h "/s" 5000
    Check "states" "resume runs again" ($t -match "/s") $t
    # Complete
    Check "states" "finishes" (Wait-Until { Test-Path (Join-Path $a.Dl "charlie.bin") } 60000)
    Start-Sleep -Milliseconds 800
    $t = Text $a
    Check "states" "finished shows Done and saved" ($t -match "Done" -and $t -match "saved") $t
    Check "states" "file is byte-exact after pause and resume" ((Verify-File (Join-Path $a.Dl "charlie.bin") 9 40000000) -eq "OK")
    Check "states" "no temp files left" (@(Files $a).Count -eq 1) ((Files $a) -join ",")
    Check "states" "selected finished offers no Pause or Resume" ($t -notmatch "Pause|Resume") $t
    Check "states" "selected finished offers Open and Remove" ($t -match "Open" -and $t -match "Remove") $t
    Check "states" "no header speed when idle" ($t -notmatch "/s") $t
    Shot $a "08-done-selected"
    [void](Row-Click $a "charlie.bin")
    Shot $a "09-done-compact"
    $t = Text $a
    Check "states" "finished row shows its size and Done" ($t -match "38.1MB" -and $t -match "Done") $t
    # Open
    Remove-Item $openLog -ErrorAction SilentlyContinue
    [void](Row-Click $a "charlie.bin")
    [void](Click-Word $h "Open")
    Check "states" "Open opens the download folder" (Wait-Until { (Test-Path $openLog) -and (Get-Content $openLog -Raw) -match [regex]::Escape($a.Dl) } 4000)
    # Remove keeps the file
    [void](Click-Word $h "Remove")
    Start-Sleep -Milliseconds 600
    $t = Text $a
    Check "states" "Remove takes it off the list" ($t -match "Paste a link above") $t
    Check "states" "Remove keeps the file by default" (Test-Path (Join-Path $a.Dl "charlie.bin"))
    Check "states" "Remove saved the queue" (-not ((Get-Content (Join-Path $a.Cfg "queue.json") -Raw) -match "charlie"))
    # Delete
    Menu $h 3 4; Start-Sleep -Milliseconds 300
    Check "states" "the option is ticked" ([E2E]::MenuChecked($h,3,4,-1))
    Add-Link $a (U "/delta.bin?size=2000000&seed=3")
    Check "states" "second file finishes" (Wait-Until { Test-Path (Join-Path $a.Dl "delta.bin") })
    Start-Sleep -Milliseconds 600
    [void](Row-Click $a "delta.bin")
    $t = Text $a
    Check "states" "with the option on the button says Delete" ($t -match "Delete" -and $t -notmatch "Remove") $t
    Shot $a "10-delete-button"
    [void](Click-Word $h "Delete")
    Start-Sleep -Milliseconds 800
    Check "states" "Delete takes the file too" (-not (Test-Path (Join-Path $a.Dl "delta.bin")))
    # Delete a partial
    Add-Link $a (U "/echo.bin?size=40000000&seed=4&rate=300000")
    [void](Wait-Text $h "/s" 5000)
    Start-Sleep -Milliseconds 600
    [void](Row-Click $a "echo.bin")
    [void](Click-Word $h "Delete")
    Check "states" "Delete on a running download cleans everything up" (Wait-Until { -not (Test-Path (Join-Path $a.Dl "echo.bin.sdmdownload")) -and -not (Test-Path (Join-Path $a.Dl "echo.bin.sdmdownload.sdm")) } 6000) ((Files $a) -join ",")
    Close-App $a
}

Section "failures" {
    $a = Open-App "failures"
    $h = $a.H
    $cases = @(
        @{ n = "nf.zip";    q = "status=404"; want = "no longer there"; label = "404" },
        @{ n = "auth.zip";  q = "status=401"; want = "sign-in"; label = "401" },
        @{ n = "deny.zip";  q = "status=403"; want = "sign-in"; label = "403" },
        @{ n = "gone.zip";  q = "status=410"; want = "no longer there"; label = "410" },
        @{ n = "oops.zip";  q = "status=500"; want = "500"; label = "500" },
        @{ n = "busy.zip";  q = "size=20000000&late_after=1&late_status=503"; want = "503"; label = "503 keeps coming" },
        @{ n = "huge.iso";  q = "size=9000000000000000"; want = "too big|disk space"; label = "file too big for the drive" },
        @{ n = "short.zip"; q = "size=5000000&ranges=0&drop_after=100000"; want = "connection was lost"; label = "stream cut short" }
    )
    foreach ($c in $cases) {
        Add-Link $a (U "/$($c.n)?$($c.q)")
        Start-Sleep -Milliseconds 400
        [void](Row-Click $a $c.n)
        # Selected, its card shows the reason once it has failed (some retry first).
        $t = Wait-Text $h "Resume" 60000
        Check "failures" "$($c.label): shows Failed" ($t -match "Failed") $c.n
        $t = Text $a
        Check "failures" "$($c.label): says why" ($t -match $c.want) $t
        Check "failures" "$($c.label): offers Resume" ($t -match "Resume") $t
        Check "failures" "$($c.label): message shows no link" ($t -notmatch "127\.0\.0\.1") $t
        Shot $a ("11-fail-" + ($c.label -replace '[^a-z0-9]','-'))
        [void](Row-Click $a $c.n)
    }
    Shot $a "12-fail-list"
    Check "failures" "refused downloads leave no files behind" (@(Files $a | Where-Object { $_ -notmatch "busy|short" }).Count -eq 0) ((Files $a) -join ",")
    Close-App $a
    # Nobody home
    $b = Open-App "failures2"
    $s2 = Start-Fault; $deadPort = $s2.Port; $s2.Proc.StandardInput.WriteLine("quit"); $s2.Proc.WaitForExit(3000) | Out-Null
    Add-Link $b "http://127.0.0.1:$deadPort/nobody.zip"
    Check "failures" "nothing listening: shows Failed" ((Wait-Text $b.H "Failed" 20000) -match "Failed")
    [void](Row-Click $b "nobody.zip")
    $t = Text $b
    Check "failures" "nothing listening: says so" ($t -match "reach the server") $t
    Shot $b "13-unreachable"
    # Resume of a dead link fails again, cleanly
    [void](Click-Word $b.H "Resume")
    $t = Wait-Text $b.H "Connecting" 1500
    $t = Wait-Text $b.H "reach the server" 20000
    Check "failures" "Resume on a dead link fails again cleanly" ($t -match "reach the server") $t
    Close-App $b
}

Section "disconnect" {
    # A short outage heals by itself
    $a = Open-App "disconnect"
    $h = $a.H
    $size = 30000000
    Add-Link $a (U "/heal.bin?size=$size&seed=21&rate=500000")
    [void](Wait-Text $h "/s" 6000); Start-Sleep -Seconds 2
    Fault "down"
    Start-Sleep -Seconds 6
    $t = Text $a
    Check "disconnect" "during an outage it says reconnecting" ($t -match "reconnecting") $t
    Shot $a "14-reconnecting"
    Check "disconnect" "during an outage it hasn't failed yet" ($t -notmatch "Failed") $t
    Fault "up"
    Check "disconnect" "after a short outage it finishes by itself" (Wait-Until { Test-Path (Join-Path $a.Dl "heal.bin") } 60000)
    Check "disconnect" "and the file is byte-exact" ((Verify-File (Join-Path $a.Dl "heal.bin") 21 $size) -eq "OK")
    Start-Sleep -Milliseconds 800
    Check "disconnect" "and it shows Done" ((Text $a) -match "Done")
    Close-App $a

    # A long outage fails with a reason, then Resume continues
    $a = Open-App "disconnect2"
    $h = $a.H
    Add-Link $a (U "/long.bin?size=$size&seed=22&rate=500000")
    [void](Wait-Text $h "/s" 6000); Start-Sleep -Seconds 2
    Fault "down"
    $t = Wait-Text $h "Failed" 90000
    Check "disconnect" "a long outage ends in Failed" ($t -match "Failed") $t
    [void](Row-Click $a "long.bin")
    $t = Text $a
    Check "disconnect" "a long outage says why" ($t -match "reach the server|connection") $t
    Shot $a "15-outage-failed"
    Check "disconnect" "progress survives on disk" (Test-Path (Join-Path $a.Dl "long.bin.sdmdownload.sdm"))
    Fault "up"
    [void](Click-Word $h "Resume")
    Check "disconnect" "Resume after the outage finishes" (Wait-Until { Test-Path (Join-Path $a.Dl "long.bin") } 60000)
    Check "disconnect" "and the file is byte-exact" ((Verify-File (Join-Path $a.Dl "long.bin") 22 $size) -eq "OK")
    Close-App $a

    # A dead link TCP can't see: Pause still works at once
    $a = Open-App "disconnect3"
    $h = $a.H
    Add-Link $a (U "/quiet.bin?size=$size&seed=23&rate=500000")
    [void](Wait-Text $h "/s" 6000); Start-Sleep -Seconds 2
    [void](Row-Click $a "quiet.bin")
    Fault "silent"
    Start-Sleep -Seconds 3
    Shot $a "16-silent-network"
    $sw = [Diagnostics.Stopwatch]::StartNew()
    [void](Click-Word $h "Pause")
    $t = Wait-Text $h "paused" 3000
    Check "disconnect" "Pause works on a silent connection" ($t -match "paused") ("after {0} ms: {1}" -f $sw.ElapsedMilliseconds, $t)
    Fault "down"; Start-Sleep -Milliseconds 300; Fault "up"
    [void](Click-Word $h "Resume")
    Check "disconnect" "Resume after the silence finishes" (Wait-Until { Test-Path (Join-Path $a.Dl "quiet.bin") } 60000)
    Check "disconnect" "and the file is byte-exact" ((Verify-File (Join-Path $a.Dl "quiet.bin") 23 $size) -eq "OK")
    Close-App $a

    # No ranges: a dropped stream fails, Resume starts over
    $a = Open-App "disconnect4"
    $h = $a.H
    Add-Link $a (U "/plain.bin?size=8000000&seed=24&ranges=0&drop_after=2000000&drop_times=1&rate=2000000")
    $t = Wait-Text $h "Failed" 15000
    Check "disconnect" "a dropped stream without ranges fails" ($t -match "Failed") $t
    [void](Row-Click $a "plain.bin")
    $t = Text $a
    Check "disconnect" "and says the connection was lost" ($t -match "connection was lost") $t
    [void](Click-Word $h "Resume")
    Check "disconnect" "Resume starts it over and finishes" (Wait-Until { Test-Path (Join-Path $a.Dl "plain.bin") } 30000)
    Check "disconnect" "and the file is byte-exact" ((Verify-File (Join-Path $a.Dl "plain.bin") 24 8000000) -eq "OK")
    Close-App $a

    # Pausing a stream says Resume starts over
    $a = Open-App "disconnect5"
    $h = $a.H
    Add-Link $a (U "/stream.bin?size=20000000&seed=25&ranges=0&rate=2000000")
    [void](Wait-Text $h "/s" 6000); Start-Sleep -Seconds 1
    [void](Row-Click $a "stream.bin")
    [void](Click-Word $h "Pause")
    $t = Wait-Text $h "paused" 3000
    Check "disconnect" "pausing a stream warns it starts over" ($t -match "starts over") $t
    Shot $a "17-paused-stream"
    [void](Click-Word $h "Resume")
    Check "disconnect" "and it finishes" (Wait-Until { Test-Path (Join-Path $a.Dl "stream.bin") } 30000)
    Check "disconnect" "and the file is byte-exact" ((Verify-File (Join-Path $a.Dl "stream.bin") 25 20000000) -eq "OK")
    Close-App $a

    # A server allowing two connections
    $a = Open-App "disconnect6"
    Add-Link $a (U "/limit.bin?size=30000000&seed=26&limit=2&limit_status=503&rate=4000000")
    Check "disconnect" "a server allowing 2 connections still works" (Wait-Until { Test-Path (Join-Path $a.Dl "limit.bin") } 60000)
    Check "disconnect" "and the file is byte-exact" ((Verify-File (Join-Path $a.Dl "limit.bin") 26 30000000) -eq "OK")
    Close-App $a

    # A file replaced during a pause
    $a = Open-App "disconnect7"
    $h = $a.H
    Add-Link $a (U "/swap.bin?size=30000000&seed=27&rate=500000&etag=1")
    [void](Wait-Text $h "/s" 6000); Start-Sleep -Seconds 2
    [void](Row-Click $a "swap.bin")
    [void](Click-Word $h "Pause")
    [void](Wait-Text $h "paused" 3000)
    Fault "replace"
    [void](Click-Word $h "Resume")
    Check "disconnect" "a file replaced during a pause restarts cleanly" (Wait-Until { Test-Path (Join-Path $a.Dl "swap.bin") } 60000)
    Check "disconnect" "and the result is entirely the new file" ((Verify-File (Join-Path $a.Dl "swap.bin") 28 30000000) -eq "OK")
    Close-App $a
}

Section "menus" {
    $a = Open-App "menus"
    $h = $a.H
    $slow = 30000000
    Add-Link $a (U "/one.bin?size=$slow&seed=31&rate=300000")
    Add-Link $a (U "/two.bin?size=$slow&seed=32&rate=300000")
    Add-Link $a (U "/three.bin?size=$slow&seed=33&rate=300000")
    [void](Wait-Text $h "/s" 6000); Start-Sleep -Seconds 1
    Shot $a "18-three-running"
    Menu $h 1 1
    $t = Wait-Text $h "(?s)Paused.*Paused.*Paused" 4000
    Check "menus" "Pause all pauses everything" ($t -match "(?s)Paused.*Paused.*Paused") $t
    Shot $a "19-all-paused"
    Menu $h 1 0
    Start-Sleep -Seconds 2
    $t = Text $a
    Check "menus" "Resume all resumes everything" ($t -notmatch "Paused" -and $t -match "/s") $t
    Menu $h 1 1
    [void](Wait-Text $h "(?s)Paused.*Paused.*Paused" 4000)

    Add-Link $a (U "/small.bin?size=500000&seed=34")
    [void](Wait-Text $h "Done" 8000)
    Start-Sleep -Milliseconds 500
    # Show completed
    Menu $h 2 0; Start-Sleep -Milliseconds 400
    $t = Text $a
    Check "menus" "Show completed off hides finished rows" ($t -notmatch "small.bin" -and $t -match "one.bin") $t
    Check "menus" "the setting is saved" ((Get-Content (Join-Path $a.Cfg "settings.json") -Raw) -match '"show_completed":\s*false')
    Menu $h 2 0; Start-Sleep -Milliseconds 400
    Check "menus" "Show completed on brings them back" ((Text $a) -match "small.bin")
    # Clear completed
    Menu $h 1 3; Start-Sleep -Milliseconds 400
    $t = Text $a
    Check "menus" "Clear completed removes finished rows only" ($t -notmatch "small.bin" -and $t -match "one.bin") $t
    Check "menus" "Clear completed keeps the file" (Test-Path (Join-Path $a.Dl "small.bin"))
    # Only finished, and hidden
    foreach ($n in "one.bin","two.bin","three.bin") {
        [void](Row-Click $a $n)
        [void](Click-Word $h "Remove")
        Start-Sleep -Milliseconds 400
    }
    Add-Link $a (U "/kept.bin?size=500000&seed=35")
    [void](Wait-Text $h "Done" 8000)
    Menu $h 2 0; Start-Sleep -Milliseconds 400
    $t = Text $a
    Check "menus" "only finished and hidden: says where they went" ($t -match "completed downloads are hidden") $t
    Shot $a "20-all-hidden"
    Menu $h 2 0; Start-Sleep -Milliseconds 300
    # Always on top
    Menu $h 2 1; Start-Sleep -Milliseconds 400
    Check "menus" "Always on top pins the window" ([E2E]::Topmost($h))
    Check "menus" "and ticks the menu" ([E2E]::MenuChecked($h,2,1,-1))
    Menu $h 2 1; Start-Sleep -Milliseconds 400
    Check "menus" "turning it off unpins" (-not [E2E]::Topmost($h))
    # Options save
    foreach ($o in @(@(3,0,"clipboard_monitor",$false), @(3,1,"clipboard_all",$true), @(3,3,"close_to_tray",$false), @(3,4,"delete_file_on_remove",$true))) {
        Menu $h $o[0] $o[1]; Start-Sleep -Milliseconds 300
        $json = Get-Content (Join-Path $a.Cfg "settings.json") -Raw
        $want = if ($o[3]) { "true" } else { "false" }
        Check "menus" "$($o[2]) toggles and saves" ($json -match ('"' + $o[2] + '":\s*' + $want)) $json
    }
    # Open folder & project page
    Remove-Item $openLog -ErrorAction SilentlyContinue
    Menu $h 0 1
    Check "menus" "Open downloads folder opens it" (Wait-Until { (Test-Path $openLog) -and (Get-Content $openLog -Raw) -match [regex]::Escape($a.Dl) } 4000)
    Remove-Item $openLog -ErrorAction SilentlyContinue
    Menu $h 4 1
    Check "menus" "Project page opens the repository" (Wait-Until { (Test-Path $openLog) -and (Get-Content $openLog -Raw) -match "github.com/kevl0317/SDM" } 4000)
    Close-App $a
}

Section "about" {
    $a = Open-App "about"
    $h = $a.H
    Menu $h 4 0
    $w = Wait-Window $a.P.Id "About SDM" 5000
    Check "about" "About opens" ($w -ne [IntPtr]::Zero)
    Start-Sleep -Milliseconds 700
    [void][E2E]::InstallHook($script:hookDll, $w)
    $t = Get-WindowText $w
    Check "about" "shows name, version, blurb" ($t -match "SDM" -and $t -match "Version 2\." -and $t -match "segmented download manager") $t
    Check "about" "shows Project page and Close" ($t -match "Project page" -and $t -match "Close") $t
    [void](Save-Shot $w "21-about-dark")
    Menu $h 4 0; Start-Sleep -Milliseconds 600
    $count = @([E2E]::Windows($a.P.Id) | Where-Object { [E2E]::IsWindowVisible($_) -and [E2E]::Title($_) -eq "About SDM" }).Count
    Check "about" "a second About doesn't open a second window" ($count -eq 1) "$count"
    Remove-Item $openLog -ErrorAction SilentlyContinue
    [void](Click-Word $w "Project")
    Check "about" "its Project page button works" (Wait-Until { (Test-Path $openLog) -and (Get-Content $openLog -Raw) -match "github.com" } 4000)
    [void](Click-Word $w "Close")
    Check "about" "Close closes it" (Wait-Until { [E2E]::Find($a.P.Id, "About SDM") -eq [IntPtr]::Zero } 3000)
    Menu $h 4 0
    $w = Wait-Window $a.P.Id "About SDM" 5000
    Start-Sleep -Milliseconds 500
    [E2E]::Key($w, 0x1B)
    Check "about" "Esc closes it" (Wait-Until { [E2E]::Find($a.P.Id, "About SDM") -eq [IntPtr]::Zero } 3000)
    Menu $h 4 0
    $w = Wait-Window $a.P.Id "About SDM" 5000
    Start-Sleep -Milliseconds 500
    [E2E]::Close($w)
    Check "about" "its X closes it" (Wait-Until { [E2E]::Find($a.P.Id, "About SDM") -eq [IntPtr]::Zero } 3000)
    Check "about" "SDM keeps running" (-not $a.P.HasExited)
    Close-App $a
}

Section "popup" {
    $a = Open-App "popup"
    $h = $a.H
    # Basics
    $ack = Catch-Link $a (U "/foxtrot.zip?size=2000000&seed=41")
    Check "popup" "a catch is acknowledged" ($ack -match '"ok":true') $ack
    $w = Popup-Of $a
    Check "popup" "a catch opens the popup" ($w -ne [IntPtr]::Zero)
    Start-Sleep -Milliseconds 800
    [void][E2E]::InstallHook($script:hookDll, $w)
    $t = Get-WindowText $w
    Check "popup" "shows heading, link and name" ($t -match "New download" -and $t -match "127\.0" -and $t -match "foxtrot.?zi") $t
    Check "popup" "buttons have their labels" ($t -match "Cancel" -and $t -match "Download") $t
    [void](Save-Shot $w "22-popup-dark")
    $c = [E2E]::Client($w)
    $k = [E2E]::GetDpiForWindow($w) / 96
    Check "popup" "is the size it should be" ([Math]::Abs($c[0] - 520 * $k) -le 2 -and $c[1] -ge 190 * $k) ($c -join "x")
    Check "popup" "isn't pinned on top" (-not [E2E]::Topmost($w))
    # Esc
    [E2E]::Key($w, 0x1B)
    Check "popup" "Esc cancels" (Wait-Until { [E2E]::Find($a.P.Id, [E2E]::Title($w)) -eq [IntPtr]::Zero -or -not [E2E]::IsWindow($w) } 3000)
    Start-Sleep -Milliseconds 400
    Check "popup" "cancel adds nothing" ((Text $a) -match "Paste a link above")
    # Cancel button
    [void](Catch-Link $a (U "/golf.zip?size=2000000&seed=42"))
    $w = Popup-Of $a
    Start-Sleep -Milliseconds 800
    [void][E2E]::InstallHook($script:hookDll, $w)
    [void](Click-Word $w "Cancel")
    Check "popup" "Cancel button cancels" (Wait-Until { -not [E2E]::IsWindow($w) } 3000)
    # X
    [void](Catch-Link $a (U "/hotel.zip?size=2000000&seed=43"))
    $w = Popup-Of $a
    Start-Sleep -Milliseconds 500
    [E2E]::Close($w)
    Check "popup" "its X cancels" (Wait-Until { -not [E2E]::IsWindow($w) } 3000)
    Check "popup" "SDM keeps running" (-not $a.P.HasExited)
    # Enter
    [void](Catch-Link $a (U "/india.zip?size=2000000&seed=44"))
    $w = Popup-Of $a
    Start-Sleep -Milliseconds 800
    [E2E]::Key($w, 0x0D)
    Check "popup" "Enter downloads" (Wait-Until { Test-Path (Join-Path $a.Dl "india.zip") } 8000)
    Check "popup" "and closes the popup" (Wait-Until { -not [E2E]::IsWindow($w) } 3000)
    Check "popup" "file is byte-exact" ((Verify-File (Join-Path $a.Dl "india.zip") 44 2000000) -eq "OK")
    # Download button
    [void](Catch-Link $a (U "/juliet.zip?size=2000000&seed=45"))
    $w = Popup-Of $a
    Start-Sleep -Milliseconds 800
    [void][E2E]::InstallHook($script:hookDll, $w)
    [void](Click-Word $w "Download")
    Check "popup" "the Download button downloads" (Wait-Until { Test-Path (Join-Path $a.Dl "juliet.zip") } 8000)
    # THE REPORTED BUG: a GitHub release's signed link, no name given.
    $far = 4000000000000
    $signed = U "/signed/$far/f804704f-4e35-4e59-840d-ac5f65c70c79?sp=r&rscd=attachment%3B+filename%3Dv2rayN-windows-64.zip&rsct=application%2Foctet-stream&size=3000000&seed=46"
    [void](Catch-Link $a $signed)
    $w = Popup-Of $a
    Start-Sleep -Milliseconds 900
    $t = Get-WindowText $w
    Check "popup" "GitHub signed link: popup shows the real name" ($t -match "v2rayN-windows-64\.zi" -and $t -notmatch "f804704f-4e35-4e59-840d-ac5f65c70c79\s*\n") $t
    [void](Save-Shot $w "23-popup-github")
    [E2E]::Key($w, 0x0D)
    Check "popup" "GitHub signed link: saved under the real name" (Wait-Until { Test-Path (Join-Path $a.Dl "v2rayN-windows-64.zip") } 8000) ((Files $a) -join ",")
    Check "popup" "GitHub signed link: file is byte-exact" ((Verify-File (Join-Path $a.Dl "v2rayN-windows-64.zip") 46 3000000) -eq "OK")
    # The same, but the name is only in the header: the popup asks the server.
    $cd = "attachment%3B%20filename%3Dfrom-server.exe"
    [void](Catch-Link $a (U "/dl/8e2f?size=2000000&seed=47&cd=$cd"))
    $w = Popup-Of $a
    $found = Wait-Until { (Get-WindowText $w) -match "from-server.exe" } 4000
    Check "popup" "the popup fills in the server's name" $found (Get-WindowText $w)
    [E2E]::Key($w, 0x0D)
    Check "popup" "saved under the server's name" (Wait-Until { Test-Path (Join-Path $a.Dl "from-server.exe") } 8000) ((Files $a) -join ",")
    # The server's name doesn't override what the browser said
    [void](Catch-Link $a (U "/dl/99?size=2000000&seed=48&cd=$cd") "browser-name.zip")
    $w = Popup-Of $a
    Start-Sleep -Milliseconds 900
    $t = Get-WindowText $w
    Check "popup" "the browser's name stands" ($t -match "browser-name\.zi") $t
    [E2E]::Key($w, 0x0D)
    Check "popup" "saved under the browser's name" (Wait-Until { Test-Path (Join-Path $a.Dl "browser-name.zip") } 8000) ((Files $a) -join ",")
    # Edited name
    [void](Catch-Link $a (U "/kilo.zip?size=2000000&seed=49"))
    $w = Popup-Of $a
    Start-Sleep -Milliseconds 900
    [E2E]::Type($w, "-renamed")
    Start-Sleep -Milliseconds 400
    $t = Get-WindowText $w
    Check "popup" "the name can be edited" ($t -match "renamed") $t
    [E2E]::Key($w, 0x0D)
    Check "popup" "saved under the edited name" (Wait-Until { (Files $a) -match "renamed" } 8000) ((Files $a) -join ",")
    # Illegal characters
    [void](Catch-Link $a (U "/lima.zip?size=2000000&seed=50") "bad:na*me?.zip")
    $w = Popup-Of $a
    Start-Sleep -Milliseconds 800
    [E2E]::Key($w, 0x0D)
    Check "popup" "illegal characters are replaced" (Wait-Until { Test-Path (Join-Path $a.Dl "bad_na_me_.zip") } 8000) ((Files $a) -join ",")
    # A second catch retargets the open popup
    [void](Catch-Link $a (U "/mike.zip?size=2000000&seed=51"))
    $w = Popup-Of $a
    Start-Sleep -Milliseconds 700
    [void](Catch-Link $a (U "/november.zip?size=2000000&seed=52"))
    Start-Sleep -Milliseconds 900
    $t = Get-WindowText $w
    [void](Save-Shot $w "25b-popup-retargeted")
    Check "popup" "a second catch retargets the popup" ($t -match "seed\s*=\s*52" -and $t -notmatch "seed\s*=\s*51|mike") $t
    $count = @([E2E]::Windows($a.P.Id) | Where-Object { [E2E]::IsWindowVisible($_) -and [E2E]::Title($_) -like "*New download" }).Count
    Check "popup" "still one popup" ($count -eq 1) "$count"
    [E2E]::Key($w, 0x1B)
    Start-Sleep -Milliseconds 500
    # Cookies, referrer, UA
    $q = "size=2000000&seed=53&need_cookie=sid%3Dabc&need_referer=shop.example&need_ua=TestBrowser"
    [void](Catch-Link $a (U "/secure.zip?$q") "" "sid=abc" "https://shop.example/page" "TestBrowser/1.0")
    $w = Popup-Of $a
    Start-Sleep -Milliseconds 800
    [E2E]::Key($w, 0x0D)
    Check "popup" "cookies, referrer and agent reach the server" (Wait-Until { Test-Path (Join-Path $a.Dl "secure.zip") } 8000) ((Files $a) -join ",")
    # Long and non-Latin
    $longName = ("very-long-file-name-" * 12) + "end.zip"
    [void](Catch-Link $a (U "/$($longName)?size=1000000&seed=54"))
    $w = Popup-Of $a
    Start-Sleep -Milliseconds 900
    [void](Save-Shot $w "24-popup-long")
    $t = Get-WindowText $w
    Check "popup" "a very long link keeps the buttons on screen" ($t -match "Cancel" -and $t -match "Download") $t
    [E2E]::Key($w, 0x1B)
    Start-Sleep -Milliseconds 500
    $cjk = -join ([char]0x738B, [char]0x69D8, [char]0x604B, [char]0x611B)
    $enc = [uri]::EscapeDataString($cjk)
    [void](Catch-Link $a (U "/$enc.part1.rar?size=1000000&seed=55"))
    $w = Popup-Of $a
    Start-Sleep -Milliseconds 900
    [void](Save-Shot $w "25-popup-cjk")
    [E2E]::Key($w, 0x0D)
    Check "popup" "a non-Latin name saves correctly" (Wait-Until { Test-Path -LiteralPath (Join-Path $a.Dl "$cjk.part1.rar") } 8000) ((Files $a) -join ",")
    Start-Sleep -Milliseconds 600
    Shot $a "26-list-after-popups"
    # Refused requests
    $port = [int](Get-Content (Join-Path $a.Cfg "ipc-port") -Raw).Trim()
    foreach ($bad in '{"token":"wrong","url":"http://x/a.zip"}', 'not json at all', '{"url":"http://x/a.zip"}', '{"token":"","op":"quit"}') {
        $c = New-Object System.Net.Sockets.TcpClient
        $c.Connect("127.0.0.1", $port)
        $s = $c.GetStream()
        $b = [Text.Encoding]::UTF8.GetBytes($bad + "`n")
        $s.Write($b, 0, $b.Length)
        $s.ReadTimeout = 700
        $got = ""
        try { $buf = New-Object byte[] 64; $n = $s.Read($buf, 0, 64); $got = [Text.Encoding]::UTF8.GetString($buf, 0, $n) } catch {}
        $c.Close()
        Check "popup" "refuses: $bad" ($got -notmatch '"ok":true') $got
    }
    Start-Sleep -Milliseconds 600
    Check "popup" "refused requests open nothing" ((Popup-Of $a 300) -eq [IntPtr]::Zero)
    Check "popup" "refused requests don't end SDM" (-not $a.P.HasExited)
    Close-App $a
}

Section "clipboard" {
    $saved = $null
    try { $saved = Get-Clipboard -Raw -ErrorAction SilentlyContinue } catch {}
    $a = Open-App "clipboard"
    $h = $a.H
    function Copy-Text([string]$s) { Set-Clipboard -Value $s; Start-Sleep -Milliseconds 700 }
    function Popup-Closed($w) { if ($w -ne [IntPtr]::Zero) { [E2E]::Key($w, 0x1B); [void](Wait-Until { -not [E2E]::IsWindow($w) } 2000) } }
    Copy-Text (U "/oscar.zip?size=1000")
    $w = Popup-Of $a 3000
    Check "clipboard" "a copied download link opens the popup" ($w -ne [IntPtr]::Zero)
    if ($w -ne [IntPtr]::Zero) { Start-Sleep -Milliseconds 500; $t = Get-WindowText $w; Check "clipboard" "showing its name" ($t -match "oscar.?zi") $t }
    Popup-Closed $w
    Copy-Text (U "/oscar.zip?size=1000")
    Check "clipboard" "the same link again is ignored" ((Popup-Of $a 1200) -eq [IntPtr]::Zero)
    Copy-Text (U "/page.html")
    Check "clipboard" "a page link is ignored" ((Popup-Of $a 1200) -eq [IntPtr]::Zero)
    Copy-Text "just some text"
    Check "clipboard" "plain text is ignored" ((Popup-Of $a 1200) -eq [IntPtr]::Zero)
    Copy-Text ("see " + (U "/papa.zip"))
    Check "clipboard" "a link inside a sentence is ignored" ((Popup-Of $a 1200) -eq [IntPtr]::Zero)
    # Catch all
    Menu $h 3 1; Start-Sleep -Milliseconds 300
    Copy-Text (U "/quebec.html")
    $w = Popup-Of $a 3000
    Check "clipboard" "Catch all link types takes a page link" ($w -ne [IntPtr]::Zero)
    Popup-Closed $w
    Menu $h 3 1; Start-Sleep -Milliseconds 300
    # Off
    Menu $h 3 0; Start-Sleep -Milliseconds 300
    Copy-Text (U "/romeo.zip?size=1000")
    Check "clipboard" "with catching off nothing happens" ((Popup-Of $a 1200) -eq [IntPtr]::Zero)
    Menu $h 3 0; Start-Sleep -Milliseconds 300
    # A signed link names its file in the query
    Copy-Text (U "/signed/4000000000000/f804704f?rscd=attachment%3B+filename%3Dsigned.zip&size=1000")
    $w = Popup-Of $a 3000
    Check "clipboard" "a copied signed link is caught" ($w -ne [IntPtr]::Zero)
    if ($w -ne [IntPtr]::Zero) { Start-Sleep -Milliseconds 600; $t = Get-WindowText $w; Check "clipboard" "and shows the real name" ($t -match "signed\.?zi") $t }
    Popup-Closed $w
    Close-App $a
    if ($saved) { Set-Clipboard -Value $saved }
}

Section "themes" {
    $a = Open-App "themes"
    $h = $a.H
    Add-Link $a (U "/done.zip?size=2000000&seed=61")
    [void](Wait-Text $h "Done" 8000)
    Add-Link $a (U "/failed.zip?status=403")
    [void](Wait-Text $h "Failed" 8000)
    Add-Link $a (U "/paused.bin?size=30000000&seed=62&rate=500000")
    [void](Wait-Text $h "/s" 6000)
    Start-Sleep -Seconds 1
    Add-Link $a (U "/running.bin?size=90000000&seed=63&rate=500000")
    [void](Row-Click $a "paused.bin")
    [void](Click-Word $h "Pause")
    [void](Wait-Text $h "paused" 3000)
    Start-Sleep -Seconds 2
    Shot $a "27-all-states-dark"
    Menu $h 2 3 1
    Start-Sleep -Milliseconds 700
    Check "themes" "Light ticks in the menu" ([E2E]::MenuChecked($h,2,3,1) -and -not [E2E]::MenuChecked($h,2,3,0))
    Check "themes" "the choice is saved" ((Get-Content (Join-Path $a.Cfg "settings.json") -Raw) -match '"theme":\s*"light"')
    Shot $a "28-all-states-light"
    $bmp = Get-ClientBitmap $h
    $px = $bmp.GetPixel(5, 5); $bmp.Dispose()
    Check "themes" "the window really is light" ($px.R -gt 200) "$px"
    # Open windows follow
    Menu $h 4 0
    $w = Wait-Window $a.P.Id "About SDM" 5000
    Start-Sleep -Milliseconds 700
    [void](Save-Shot $w "29-about-light")
    [void](Catch-Link $a (U "/sierra.zip?size=1000"))
    $pw = Popup-Of $a
    Start-Sleep -Milliseconds 800
    [void](Save-Shot $pw "30-popup-light")
    Menu $h 2 3 0
    Start-Sleep -Milliseconds 800
    $b1 = Get-ClientBitmap $w; $p1 = $b1.GetPixel(3, 3); $b1.Dispose()
    $b2 = Get-ClientBitmap $pw; $p2 = $b2.GetPixel(3, 3); $b2.Dispose()
    Check "themes" "About follows the theme live" ($p1.R -lt 60) "$p1"
    Check "themes" "the popup follows the theme live" ($p2.R -lt 60) "$p2"
    [void](Save-Shot $pw "31-popup-back-dark")
    [E2E]::Key($pw, 0x1B); [E2E]::Key($w, 0x1B)
    Close-App $a
}

Section "layout" {
    $a = Open-App "layout"
    $h = $a.H
    $long = ("a-really-long-name-" * 10) + "end.iso"
    Add-Link $a (U "/$($long)?size=200000000&seed=71&rate=100000")
    Start-Sleep -Seconds 2
    $t = Text $a
    Check "layout" "a very long name stays on one line" (@($t -split "`n" | Where-Object { $_ -match "really|long-name" }).Count -eq 1) $t
    Check "layout" "and keeps its end visible" ($t -match "end.*iso") $t
    $cjk = -join ([char]0x738B, [char]0x69D8, [char]0x604B, [char]0x611B, [char]0x306E, [char]0x7269, [char]0x8A9E)
    Add-Link $a (U "/$([uri]::EscapeDataString($cjk))-v2.zip?size=2000000&seed=72")
    [void](Wait-Text $h "Done" 8000)
    Add-Link $a (U "/refused.zip?status=401")
    [void](Wait-Text $h "Failed" 8000)
    Start-Sleep -Seconds 1
    Shot $a "32-long-names"
    # The narrowest window the app allows (520 x 380 inside)
    $o = [E2E]::ClientOrigin($h); $r = [E2E]::Rect($h); $cl = [E2E]::Client($h)
    $k = [E2E]::GetDpiForWindow($h) / 96
    [void][E2E]::SetWindowPos($h, [IntPtr]::Zero, 100, 100, [int](520 * $k) + $r[2] - $cl[0], [int](380 * $k) + $r[3] - $cl[1], 0x0014)
    Start-Sleep -Milliseconds 800
    $t = Text $a
    Check "layout" "at the narrowest, long names still take one line" (@($t -split "`n" | Where-Object { $_ -match "really|long-name" }).Count -eq 1) $t
    [void](Row-Click $a "refused.zip")
    Start-Sleep -Milliseconds 500
    $t = Text $a
    Check "layout" "at the narrowest, a failed card keeps its buttons" ($t -match "Resume" -and $t -match "Open" -and $t -match "Remove") $t
    Check "layout" "and its reason" ($t -match "401") $t
    Shot $a "33-narrow-failed"
    [void](Row-Click $a "refused.zip")
    # A long list
    [void][E2E]::SetWindowPos($h, [IntPtr]::Zero, 100, 40, 776, 659, 0x0014)
    Start-Sleep -Milliseconds 500
    foreach ($i in 1..24) { Add-Link $a (U "/file-$i.bin?size=2000&seed=$i") }
    Start-Sleep -Seconds 2
    Shot $a "34-long-list"
    $before = Text $a
    [E2E]::Wheel($h, 300, 400, -120 * 5)
    Start-Sleep -Milliseconds 700
    $after = Text $a
    Check "layout" "the list scrolls" ($before -ne $after) ""
    Shot $a "35-long-list-scrolled"
    [E2E]::Wheel($h, 300, 400, 120 * 30)
    Start-Sleep -Milliseconds 500
    # Newest first; running pinned on top
    $lines = @(Read-Window $h)
    $runY = ($lines | Where-Object { $_.Text -match "really" } | Select-Object -First 1).Words[0].Y
    $newY = ($lines | Where-Object { $_.Text -match "file-2[0-9]" } | Select-Object -First 1).Words[0].Y
    Check "layout" "what's running is pinned above newer rows" ($runY -lt $newY) "running at y=$runY, newest at y=$newY"
    Close-App $a
}

Section "windowing" {
    # Keep in tray on: X hides; off: X quits
    $a = Open-App "windowing"
    $h = $a.H
    [E2E]::Close($h)
    Check "windowing" "with tray on, X hides the window" (Wait-Until { -not [E2E]::IsWindowVisible($h) } 3000)
    Check "windowing" "and SDM keeps running" (-not $a.P.HasExited)
    # A catch while hidden: only the popup
    [void](Catch-Link $a (U "/tango.zip?size=2000000&seed=81"))
    $w = Popup-Of $a
    Check "windowing" "a catch while hidden opens just the popup" ($w -ne [IntPtr]::Zero -and -not [E2E]::IsWindowVisible($h))
    Start-Sleep -Milliseconds 700
    [E2E]::Key($w, 0x1B)
    Start-Sleep -Milliseconds 600
    Check "windowing" "cancelling leaves the window hidden" (-not [E2E]::IsWindowVisible($h))
    [void](Catch-Link $a (U "/uniform.zip?size=2000000&seed=82"))
    $w = Popup-Of $a
    Start-Sleep -Milliseconds 700
    [E2E]::Key($w, 0x0D)
    Check "windowing" "accepting brings the window forward" (Wait-Until { [E2E]::IsWindowVisible($h) } 3000)
    Check "windowing" "and downloads" (Wait-Until { Test-Path (Join-Path $a.Dl "uniform.zip") } 8000)
    # Second launch while hidden
    [E2E]::Close($h)
    [void](Wait-Until { -not [E2E]::IsWindowVisible($h) } 3000)
    $second = Start-Process -FilePath $script:exe -PassThru
    Check "windowing" "a second launch steps aside" ($second.WaitForExit(8000))
    Check "windowing" "and shows the running window" (Wait-Until { [E2E]::IsWindowVisible($h) } 3000)
    Check "windowing" "only one SDM is running" (@(Get-Process | Where-Object { $_.Path -eq $script:exe }).Count -eq 1)
    # Minimized, second launch
    [void][E2E]::ShowWindow($h, 6)
    Start-Sleep -Milliseconds 500
    $second = Start-Process -FilePath $script:exe -PassThru
    [void]$second.WaitForExit(8000)
    Check "windowing" "a second launch restores a minimized window" (Wait-Until { -not [E2E]::IsIconic($h) } 3000)
    # --quit
    $q = Start-Process -FilePath $script:exe -ArgumentList "--quit" -PassThru
    Check "windowing" "--quit returns" ($q.WaitForExit(15000))
    Check "windowing" "--quit ends the running SDM" (Gone $a.P 5000)
    # Tray off
    $a = Open-App "windowing2"
    $h = $a.H
    Menu $h 3 3; Start-Sleep -Milliseconds 300
    [E2E]::Close($h)
    Check "windowing" "with tray off, X quits" (Gone $a.P 5000)
    # File > Exit
    $a = Open-App "windowing3"
    $h = $a.H
    Menu $h 0 3
    Check "windowing" "File > Exit quits" (Gone $a.P 5000)
    # Started hidden
    $a = Open-App "windowing4" -Args2 @("--background")
    Check "windowing" "--background starts in the tray" (-not [E2E]::IsWindowVisible($a.H))
    [void](Catch-Link $a (U "/victor.zip?size=2000000&seed=83"))
    $w = Popup-Of $a
    Check "windowing" "and shows the popup for a catch" ($w -ne [IntPtr]::Zero)
    Close-App $a
    # Tray: messages to its hidden window
    $a = Open-App "windowing5"
    $h = $a.H
    [E2E]::Close($h)
    [void](Wait-Until { -not [E2E]::IsWindowVisible($h) } 3000)
    $tray = [E2E]::Windows($a.P.Id) | Where-Object { [E2E]::Class($_) -eq "tray_icon_app" } | Select-Object -First 1
    Check "windowing" "the tray icon's window exists" ($null -ne $tray)
    # The tray menu: its items' WM_COMMAND ids come just before the menu bar's. Find
    # Open SDM by trying them with the window hidden; Quit is the next one.
    $first = [int][E2E]::MenuId($h, 0, 0, -1)
    $openId = 0
    foreach ($id in ($first - 12)..($first - 1)) {
        if ($id -lt 1) { continue }
        [void][E2E]::PostMessage($tray, 0x0111, [IntPtr]$id, [IntPtr]::Zero)
        Start-Sleep -Milliseconds 250
        if ([E2E]::IsWindowVisible($h)) { $openId = $id; break }
        if ($a.P.HasExited) { break }
    }
    Check "windowing" "the tray menu's Open SDM shows the window" ($openId -ne 0) "id $openId"
    if ($openId) {
        [void][E2E]::PostMessage($tray, 0x0111, [IntPtr]($openId + 1), [IntPtr]::Zero)
        Check "windowing" "the tray menu's Quit ends SDM" (Gone $a.P 6000)
    }
    Close-App $a
    # Two background launches at once (two catches with SDM not running)
    $base = Join-Path $here "p\windowing6"
    Clear-Dir $base
    New-Item -ItemType Directory -Force -Path "$base\cfg", "$base\dl" | Out-Null
    $env:SDM_CONFIG_DIR = "$base\cfg"; $env:SDM_DOWNLOAD_DIR = "$base\dl"
    $p1 = Start-Process -FilePath $script:exe -ArgumentList "--background" -PassThru -WindowStyle Hidden
    $p2 = Start-Process -FilePath $script:exe -ArgumentList "--background" -PassThru -WindowStyle Hidden
    Start-Sleep -Seconds 4
    $running = @(Get-Process | Where-Object { $_.Path -eq $script:exe })
    Check "windowing" "two background starts at once leave one SDM" ($running.Count -eq 1) "$($running.Count) running"
    $running | ForEach-Object { try { $_.Kill() } catch {} }
    Start-Sleep -Milliseconds 500
}

Section "persist" {
    # A clean exit keeps every state
    $a = Open-App "persist" -Settings @{ theme = "light"; always_on_top = $true; delete_file_on_remove = $true; show_completed = $true }
    $h = $a.H
    Check "persist" "saved theme applies at start" ([E2E]::MenuChecked($h,2,3,1))
    Check "persist" "saved always-on-top applies at start" (Wait-Until { [E2E]::Topmost($h) } 2000)
    Check "persist" "saved option applies at start" ([E2E]::MenuChecked($h,3,4,-1))
    Add-Link $a (U "/finished.zip?size=2000000&seed=91")
    [void](Wait-Text $h "Done" 8000)
    Add-Link $a (U "/refused.zip?status=404")
    [void](Wait-Text $h "Failed" 8000)
    $size = 30000000
    Add-Link $a (U "/running.bin?size=$size&seed=92&rate=500000")
    [void](Wait-Text $h "/s" 6000); Start-Sleep -Seconds 2
    Shot $a "36-before-exit"
    Menu $h 0 3
    Check "persist" "exit with a download running ends SDM" (Gone $a.P 8000)
    Check "persist" "the running download saved its progress" (Test-Path (Join-Path $a.Dl "running.bin.sdmdownload.sdm"))
    $a = Open-App "persist" -KeepProfile
    $h = $a.H
    $t = Text $a
    Shot $a "37-after-restart"
    Check "persist" "after restart: finished still Done" ($t -match "finished.?zip" -and $t -match "Done") $t
    Check "persist" "after restart: failed still Failed" ($t -match "refused.?zip" -and $t -match "Failed") $t
    Check "persist" "after restart: running is Paused with progress" ($t -match "running.bin" -and $t -match "Paused" -and $t -match "MB /") $t
    [void](Row-Click $a "refused.zip")
    $t = Text $a
    Check "persist" "after restart: the failure still says why" ($t -match "no longer there") $t
    [void](Row-Click $a "refused.zip")
    [void](Row-Click $a "running.bin")
    [void](Click-Word $h "Resume")
    Check "persist" "after restart: Resume finishes" (Wait-Until { Test-Path (Join-Path $a.Dl "running.bin") } 60000)
    Check "persist" "after restart: file is byte-exact" ((Verify-File (Join-Path $a.Dl "running.bin") 92 $size) -eq "OK")
    # A finished file that was moved
    Close-App $a
    Remove-Item (Join-Path $a.Dl "finished.zip")
    $a = Open-App "persist" -KeepProfile
    $t = Text $a
    Check "persist" "a moved file's row stays Done" ($t -match "finished.?zip" -and $t -match "Done") $t
    Close-App $a

    # A crash mid-download
    $a = Open-App "crash"
    $h = $a.H
    Add-Link $a (U "/crash.bin?size=$size&seed=93&rate=500000")
    [void](Wait-Text $h "/s" 6000); Start-Sleep -Seconds 5
    Stop-Sdm $a.P
    Check "persist" "a crash leaves a checkpoint" (Test-Path (Join-Path $a.Dl "crash.bin.sdmdownload.sdm"))
    $a = Open-App "crash" -KeepProfile
    $h = $a.H
    $t = Text $a
    Shot $a "38-after-crash"
    Check "persist" "after a crash it's Paused with progress" ($t -match "crash.bin" -and $t -match "Paused" -and $t -match "MB /") $t
    [void](Row-Click $a "crash.bin")
    Check "persist" "after a crash it offers Resume" ((Click-Word $h "Resume"))
    Check "persist" "after a crash Resume finishes" (Wait-Until { Test-Path (Join-Path $a.Dl "crash.bin") } 60000)
    Check "persist" "after a crash the file is byte-exact" ((Verify-File (Join-Path $a.Dl "crash.bin") 93 $size) -eq "OK")
    Close-App $a

    # A crash before any progress: the job can still start
    $a = Open-App "crash2"
    $h = $a.H
    Add-Link $a (U "/early.bin?size=2000000&seed=94&delay_ms=4000&fault_probe=1")
    Start-Sleep -Milliseconds 800
    Stop-Sdm $a.P
    $a = Open-App "crash2" -KeepProfile
    $h = $a.H
    $t = Text $a
    Check "persist" "a job from before the crash is listed" ($t -match "early.bin" -and $t -match "Paused") $t
    [void](Row-Click $a "early.bin")
    [void](Click-Word $h "Resume")
    Check "persist" "and can be started" (Wait-Until { Test-Path (Join-Path $a.Dl "early.bin") } 15000) ((Files $a) -join ",")
    Close-App $a

    # Damaged files
    $base = Join-Path $here "p\damaged"
    Clear-Dir $base
    New-Item -ItemType Directory -Force -Path "$base\cfg", "$base\dl" | Out-Null
    "this is { not json" | Out-File -Encoding ascii "$base\cfg\queue.json"
    "also } not json" | Out-File -Encoding ascii "$base\cfg\settings.json"
    $a = Open-App "damaged" -KeepProfile
    Check "persist" "damaged queue and settings don't stop SDM" (-not $a.P.HasExited)
    $t = Text $a
    Check "persist" "it starts empty" ($t -match "Paste a link above") $t
    Check "persist" "the damaged queue is kept aside" (Test-Path "$base\cfg\queue.json.bad")
    Add-Link $a (U "/after.zip?size=1000")
    Check "persist" "and works normally" (Wait-Until { Test-Path (Join-Path $a.Dl "after.zip") } 8000)
    Close-App $a

    # A stale endpoint from a crash
    $a = Open-App "stale"
    Stop-Sdm $a.P
    $a = Open-App "stale" -KeepProfile
    Check "persist" "starting over a stale endpoint works" (-not $a.P.HasExited)
    [void](Catch-Link $a (U "/whiskey.zip?size=1000"))
    Check "persist" "and catches again" ((Popup-Of $a) -ne [IntPtr]::Zero)
    Close-App $a
}

Section "host" {
    # The browser's native-messaging host, as Chrome starts it
    $base = Join-Path $here "p\host"
    Clear-Dir $base
    New-Item -ItemType Directory -Force -Path "$base\cfg", "$base\dl", "$base\io" | Out-Null
    $env:SDM_CONFIG_DIR = "$base\cfg"; $env:SDM_DOWNLOAD_DIR = "$base\dl"
    function Frame([string]$json, [string]$file) {
        $b = [Text.Encoding]::UTF8.GetBytes($json)
        $len = [BitConverter]::GetBytes([uint32]$b.Length)
        [IO.File]::WriteAllBytes($file, [byte[]]($len + $b))
    }
    function Run-Host([string]$in, [string]$out) {
        $cmd = '"' + $script:exe + '" chrome-extension://mbaoofinhcijeneijhaglnjonhikbojj/ < "' + $in + '" > "' + $out + '"'
        $p = Start-Process -FilePath cmd.exe -ArgumentList '/d','/s','/c',('"' + $cmd + '"') -WindowStyle Hidden -PassThru
        [void]$p.WaitForExit(15000)
        if (Test-Path $out) { $bytes = [IO.File]::ReadAllBytes($out); if ($bytes.Length -gt 4) { return [Text.Encoding]::UTF8.GetString($bytes, 4, $bytes.Length - 4) } }
        ""
    }
    # SDM not running: the host starts it hidden, and the catch arrives
    $url = U "/xray.zip?size=1000000&seed=95"
    Frame (@{ url = $url; filename = "" } | ConvertTo-Json -Compress) "$base\io\f1"
    $r = Run-Host "$base\io\f1" "$base\io\o1"
    Check "host" "starts SDM and hands the catch over" ($r -match '"ok":true') $r
    $gui = Get-Process | Where-Object { $_.Path -eq $script:exe } | Select-Object -First 1
    Check "host" "SDM is running" ($null -ne $gui)
    if ($gui) {
        $w = Wait-Window $gui.Id ("SDM " + [char]0x2014 + " New download") 8000
        Check "host" "only the popup comes up" ($w -ne [IntPtr]::Zero -and -not [E2E]::IsWindowVisible([E2E]::FindMain($gui.Id)))
        Start-Sleep -Milliseconds 700
        [E2E]::Key($w, 0x1B)
        Start-Sleep -Milliseconds 500
    }
    # SDM running
    Frame (@{ url = (U "/yankee.zip?size=1000") } | ConvertTo-Json -Compress) "$base\io\f2"
    Check "host" "hands over to a running SDM" ((Run-Host "$base\io\f2" "$base\io\o2") -match '"ok":true')
    Start-Sleep -Milliseconds 700
    if ($gui) { $w = Wait-Window $gui.Id ("SDM " + [char]0x2014 + " New download") 4000; if ($w -ne [IntPtr]::Zero) { [E2E]::Key($w, 0x1B) } }
    # Bad messages
    Frame '{"filename":"no-url"}' "$base\io\f3"
    Check "host" "rejects a message with no link" ((Run-Host "$base\io\f3" "$base\io\o3") -match '"ok":false')
    Frame 'not json' "$base\io\f4"
    Check "host" "rejects garbage" ((Run-Host "$base\io\f4" "$base\io\o4") -match '"ok":false')
    [IO.File]::WriteAllBytes("$base\io\f5", [byte[]](0xFF, 0xFF, 0xFF, 0x7F))
    Check "host" "ends on an absurd frame length" ((Run-Host "$base\io\f5" "$base\io\o5") -eq "")
    [IO.File]::WriteAllBytes("$base\io\f6", [byte[]]@())
    Check "host" "ends quietly when the browser closes the port" ((Run-Host "$base\io\f6" "$base\io\o6") -eq "")
    Start-Sleep -Milliseconds 300
    if ($gui) { Check "host" "SDM survived all of it" (-not $gui.HasExited) }
    Get-Process | Where-Object { $_.Path -eq $script:exe } | ForEach-Object { try { $_.Kill() } catch {} }
}

Section "cli" {
    $cli = Join-Path $repo "target\debug\sdm.exe"
    $base = Join-Path $here "p\cli"
    Clear-Dir $base
    New-Item -ItemType Directory -Force -Path "$base\out" | Out-Null
    $o = & $cli (U "/cli.zip?size=3000000&seed=96") -o "$base\out" 2>&1
    Check "cli" "downloads, exit code 0" ($LASTEXITCODE -eq 0) (($o | Select-Object -Last 2) -join " | ")
    Check "cli" "file is byte-exact" ((Verify-File "$base\out\cli.zip" 96 3000000) -eq "OK")
    $o = & $cli (U "/named?size=3000&seed=97") -o "$base\out" -f "chosen.bin" -n 2 2>&1
    Check "cli" "-f and -n work" (Test-Path "$base\out\chosen.bin") (($o | Select-Object -Last 1) -join " ")
    $o = & $cli (U "/nope.zip?status=404") -o "$base\out" 2>&1
    Check "cli" "404: exit code 1 and a reason" ($LASTEXITCODE -eq 1 -and (($o -join " ") -match "no longer there")) (($o | Select-Object -Last 1) -join " ")
    $o = & $cli "not a link" -o "$base\out" 2>&1
    Check "cli" "bad link: exit code 1 and a reason" ($LASTEXITCODE -eq 1 -and (($o -join " ") -match "link")) (($o | Select-Object -Last 1) -join " ")
    # A leftover sidecar: running it again resumes
    $again = U "/again.bin?size=30000000&seed=98&rate=300000"
    $job = Start-Process -FilePath $cli -ArgumentList $again, "-o", "$base\out" -PassThru -WindowStyle Hidden
    [void](Wait-Until { Test-Path "$base\out\again.bin.sdmdownload.sdm" } 15000)
    Start-Sleep -Seconds 3
    $job.Kill(); [void]$job.WaitForExit(3000)
    $before = (Get-Content "$base\out\again.bin.sdmdownload.sdm" -Raw | ConvertFrom-Json).segments | ForEach-Object { $_[2] } | Measure-Object -Sum
    $o = & $cli $again -o "$base\out" 2>&1
    Check "cli" "running it again finishes" ($LASTEXITCODE -eq 0 -and (Test-Path "$base\out\again.bin")) (($o | Select-Object -Last 2) -join " | ")
    Check "cli" "and the file is byte-exact" ((Verify-File "$base\out\again.bin" 98 30000000) -eq "OK")
    Check "cli" "it had progress to resume from" ($before.Sum -gt 0) "$($before.Sum) bytes"
    Check "cli" "and it really resumed" (($o -join " ") -match "Resuming at") (($o | Select-Object -First 1) -join " ")
}

Section "monitors" {
    # A real drag needs real monitors; this models what one does to the window.
    $a = Open-App "monitors"
    $h = $a.H
    $msg = [E2E]::RegisterWindowMessage("SdmTestDpiStorm")
    $other = if ([E2E]::GetDpiForWindow($h) -eq 144) { 96 } else { 144 }
    $start = [E2E]::Client($h)
    foreach ($flips in 2, 10, 50) {
        [void][E2E]::PostMessage($h, $msg, [IntPtr]$flips, [IntPtr]$other)
        Start-Sleep -Milliseconds 900
        $c = [E2E]::Client($h)
        Check "monitors" "after $flips flips between scales it keeps its size" ([Math]::Abs($c[0] - $start[0]) -le 1 -and [Math]::Abs($c[1] - $start[1]) -le 1) ($c -join "x")
    }
    Close-App $a
}

# ---- report --------------------------------------------------------------------
Fault "quit"
$fails = @($script:results | Where-Object { -not $_.Ok })
Write-Output ""
Write-Output ("{0} checks, {1} failed" -f $script:results.Count, $fails.Count)
foreach ($f in $fails) { Write-Output ("FAILED  {0}: {1} -- {2}" -f $f.Area, $f.Name, $f.Detail) }
$script:results | ConvertTo-Json -Depth 3 | Out-File -Encoding utf8 (Join-Path $here "results.json")
exit $fails.Count

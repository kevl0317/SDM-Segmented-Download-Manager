<#
.SYNOPSIS
Installs SDM over a running copy of an older build, the way an upgrade does, and
checks that Setup closes it cleanly.

.DESCRIPTION
Everything happens under a throwaway identity: "SDMTest", with its own AppId,
folder (target\installer-tests) and settings, and without the browser-host steps,
so the real SDM and its browser registration are never touched. It installs
-OldExe with the current installer logic, starts it, installs this checkout's
release build over it with SDM.iss, then uninstalls the test copy.

Passes when the upgrade finishes within 20 seconds, the old SDM quit by itself
(it saves its download list on the way out), and SDM.exe was replaced. Build
first: cargo build --release -p sdm-gui. Windows (the test SDM and Setup's
progress) appear on screen for a few seconds.

.EXAMPLE
.\tests\installer\run.ps1 -OldExe "$env:LOCALAPPDATA\Programs\SDM\SDM.exe"
.EXAMPLE
.\tests\installer\run.ps1 -OldExe path\to\older\sdm-gui.exe -NotRunning
#>
param(
    # The SDM build to upgrade from (any 2.x SDM.exe or sdm-gui.exe).
    [Parameter(Mandatory = $true)][string]$OldExe,
    # Upgrade over an SDM that isn't running.
    [switch]$NotRunning
)
$ErrorActionPreference = "Continue"
$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$iscc = @(
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 (ISCC.exe) not found" }
$newExe = Join-Path $repo "target\release\sdm-gui.exe"
if (-not (Test-Path $newExe)) { throw "Build first: cargo build --release -p sdm-gui" }
$work = Join-Path $repo "target\installer-tests"
if (Test-Path $work) { [IO.Directory]::Delete("\\?\$work", $true) }
New-Item -ItemType Directory -Force "$work\cfg", "$work\dl" | Out-Null
$dir = Join-Path $work "SDMTest"
$testAppId = "{5D3F0C11-2B7A-4E10-9C55-7A1E0D0C7E01}"

# SDM.iss's logic under a test identity.
$s = [IO.File]::ReadAllText((Join-Path $repo "SDM.iss"))
$s = $s.Replace('#define MyAppName "SDM"', '#define MyAppName "SDMTest"')
$s = $s.Replace('AppId={{B3F1A2C4-7E5D-4A8B-9C6F-1D2E3F4A5B6C}', "AppId={$testAppId")
$s = $s.Replace('OutputDir=dist', "OutputDir=$work")
$s = $s.Replace('OutputBaseFilename=SDM-Setup-{#MyAppVersion}', 'OutputBaseFilename={#Label}')
$s = $s.Replace('SetupIconFile=sdm.ico', "SetupIconFile=$repo\sdm.ico")
$s = $s.Replace('Source: "target\release\sdm-gui.exe"', 'Source: "{#SourceExe}"')
$s = $s.Replace('Source: "extension\*"', "Source: `"$repo\extension\*`"")
$s = ($s -split "`r?`n" | Where-Object { $_ -notmatch 'browserhost|register-host|unregister-host' }) -join "`r`n"
[IO.File]::WriteAllText("$work\test.iss", $s)
function Compile([string]$label, [string]$exe) {
    $o = & $iscc /Q "/DLabel=$label" "/DSourceExe=$exe" "$work\test.iss" 2>&1
    if ($LASTEXITCODE -ne 0) { throw "ISCC failed: $o" }
    Join-Path $work "$label.exe"
}
$oldSetup = Compile "old" $OldExe
$newSetup = Compile "new" $newExe

# Whatever SDMTest runs uses the test profile, never the real one.
$env:SDM_CONFIG_DIR = "$work\cfg"; $env:SDM_DOWNLOAD_DIR = "$work\dl"
$p = Start-Process $oldSetup -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/DIR=`"$dir`"" -PassThru
[void]$p.WaitForExit(60000)
$oldWritten = (Get-Item "$dir\SDM.exe").LastWriteTime
$app = $null
if (-not $NotRunning) {
    $app = Start-Process "$dir\SDM.exe" -PassThru
    Start-Sleep -Seconds 4
}

$clock = [Diagnostics.Stopwatch]::StartNew()
$p = Start-Process $newSetup -ArgumentList "/SILENT", "/SUPPRESSMSGBOXES", "/DIR=`"$dir`"", "/LOG=`"$work\upgrade.log`"" -PassThru
$finished = $p.WaitForExit(90000)
$took = [int]$clock.Elapsed.TotalSeconds

$checks = [ordered]@{
    "Setup finished within 20 s (took $took s)" = $finished -and $took -le 20 -and $p.ExitCode -eq 0
    "SDM.exe was replaced" = (Get-Item "$dir\SDM.exe").LastWriteTime -ne $oldWritten
}
if ($app) {
    $checks["the old SDM quit"] = $app.HasExited
    $checks["it quit by itself, saving its list"] = Test-Path "$work\cfg\queue.json"
}

# Clean up only what this test started.
if (-not $finished) { try { $p.Kill() } catch {} }
Get-Process | Where-Object { $_.Path -like "$work\*" } | ForEach-Object { try { $_.Kill() } catch {} }
$uninstaller = Get-ChildItem "$dir\unins*.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
if ($uninstaller) { $u = Start-Process $uninstaller.FullName -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES" -PassThru; [void]$u.WaitForExit(60000) }
Get-Process | Where-Object { $_.Path -like "$work\*" } | ForEach-Object { try { $_.Kill() } catch {} }

$failed = 0
foreach ($c in $checks.GetEnumerator()) {
    if ($c.Value) { "PASS  $($c.Key)" } else { "FAIL  $($c.Key)"; $failed++ }
}
if ($failed) { "Setup's log: $work\upgrade.log" }
exit $failed

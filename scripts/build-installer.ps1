<#
.SYNOPSIS
Builds the release app and the installer, optionally signed.

.DESCRIPTION
Unsigned by default. With -SignCommand it signs SDM.exe, the installer and the
uninstaller with your own certificate and prints each signature's status.
Official releases are built and signed by GitHub Actions instead (see
"Release builds and signing" in README.md).

.EXAMPLE
.\scripts\build-installer.ps1
.EXAMPLE
.\scripts\build-installer.ps1 -SignCommand '"C:\...\signtool.exe" sign /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 /a'
#>
param(
    # A full signtool command line to sign with; the file to sign is appended.
    [string]$SignCommand,
    # Where the installer goes.
    [string]$OutputDir = "dist"
)

$ErrorActionPreference = "Stop"
Set-Location (Split-Path -Parent $PSScriptRoot)

# Native tools print progress on stderr, which Windows PowerShell turns into
# errors (fatal under "Stop") whenever output is captured; judge them by their
# exit code instead.
function Invoke-Tool([string]$what, [scriptblock]$run) {
    $saved = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try { & $run 2>&1 | ForEach-Object { "$_" } } finally { $ErrorActionPreference = $saved }
    if ($LASTEXITCODE -ne 0) { throw "$what failed (exit code $LASTEXITCODE)" }
}

$iscc = @(
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 (ISCC.exe) not found" }

Invoke-Tool "cargo build" { cargo build --release -p sdm-gui }

$version = (Select-String -Path SDM.iss -Pattern '#define MyAppVersion "([^"]+)"').Matches[0].Groups[1].Value
$setup = Join-Path $OutputDir "SDM-Setup-$version.exe"

if ($SignCommand) {
    # Inno runs this on SDM.exe, the installer and the uninstaller: $f is the
    # file (quoted), $q a literal quote.
    $tool = $SignCommand.Replace('"', '$q') + ' $f'
    Invoke-Tool "ISCC" { & $iscc /Q "/O$OutputDir" /DSIGN "/Ssdmsign=$tool" SDM.iss }
    foreach ($file in "target\release\sdm-gui.exe", $setup) {
        $sig = Get-AuthenticodeSignature $file
        $signer = if ($sig.SignerCertificate) { $sig.SignerCertificate.Subject } else { "no signature" }
        "{0}: {1} ({2})" -f $file, $sig.Status, $signer
    }
} else {
    Invoke-Tool "ISCC" { & $iscc /Q "/O$OutputDir" SDM.iss }
    "Unsigned build."
}
"Installer: $setup"

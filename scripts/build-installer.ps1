<#
.SYNOPSIS
Builds the release app and the installer, signed when signing is set up.

.DESCRIPTION
Unsigned by default. With -SignMetadata (or $env:SDM_SIGN_METADATA) it signs
SDM.exe, the installer and the uninstaller through Azure Artifact Signing
(signtool with the Artifact Signing dlib) and checks the signatures. Sign in
with `az login` first. See "Signing" in README.md for the one-time setup.

.EXAMPLE
.\scripts\build-installer.ps1
.EXAMPLE
.\scripts\build-installer.ps1 -SignMetadata C:\signing\metadata.json
#>
param(
    # Artifact Signing metadata.json: Endpoint, CodeSigningAccountName, CertificateProfileName.
    [string]$SignMetadata = $env:SDM_SIGN_METADATA,
    # Azure.CodeSigning.Dlib.dll; looked up in the Artifact Signing Client Tools if not given.
    [string]$Dlib = $env:SDM_SIGN_DLIB,
    # A full signtool command line to sign with instead, e.g. for an OV certificate
    # on a hardware token; the file to sign is appended.
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

function Find-SignTool {
    $kits = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"
    $tool = Get-ChildItem "$kits\*\x64\signtool.exe" -ErrorAction SilentlyContinue |
        Sort-Object FullName -Descending | Select-Object -First 1
    if (-not $tool) { throw "signtool.exe not found: install the Windows SDK" }
    $tool.FullName
}

function Find-Dlib {
    $roots = @("$env:LOCALAPPDATA\Microsoft", $env:ProgramFiles, "$env:ProgramFiles\Microsoft", ${env:ProgramFiles(x86)})
    foreach ($root in $roots) {
        if (-not (Test-Path $root)) { continue }
        foreach ($dir in Get-ChildItem $root -Directory -Filter "*Signing*" -ErrorAction SilentlyContinue) {
            $hits = @(Get-ChildItem $dir.FullName -Recurse -Filter "Azure.CodeSigning.Dlib.dll" -ErrorAction SilentlyContinue)
            $x64 = $hits | Where-Object { $_.DirectoryName -match "x64" } | Select-Object -First 1
            if ($x64) { return $x64.FullName }
            if ($hits) { return $hits[0].FullName }
        }
    }
    throw "Azure.CodeSigning.Dlib.dll not found: install the Artifact Signing Client Tools or pass -Dlib"
}

$iscc = @(
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 (ISCC.exe) not found" }

if (-not $SignCommand -and $SignMetadata) {
    if (-not (Test-Path $SignMetadata)) { throw "metadata file not found: $SignMetadata" }
    if (-not $Dlib) { $Dlib = Find-Dlib }
    $SignCommand = '"{0}" sign /fd SHA256 /tr http://timestamp.acs.microsoft.com /td SHA256 /dlib "{1}" /dmdf "{2}"' -f
        (Find-SignTool), $Dlib, (Resolve-Path $SignMetadata)
}

Invoke-Tool "cargo build" { cargo build --release -p sdm-gui }

$version = (Select-String -Path SDM.iss -Pattern '#define MyAppVersion "([^"]+)"').Matches[0].Groups[1].Value
$setup = Join-Path $OutputDir "SDM-Setup-$version.exe"

if ($SignCommand) {
    # Inno runs this on SDM.exe, the installer and the uninstaller: $f is the
    # file (quoted), $q a literal quote.
    $tool = $SignCommand.Replace('"', '$q') + ' $f'
    Invoke-Tool "ISCC" { & $iscc /Q "/O$OutputDir" /DSIGN "/Ssdmsign=$tool" SDM.iss }
} else {
    Invoke-Tool "ISCC" { & $iscc /Q "/O$OutputDir" SDM.iss }
}

if ($SignCommand) {
    foreach ($file in "target\release\sdm-gui.exe", $setup) {
        $sig = Get-AuthenticodeSignature $file
        $signer = if ($sig.SignerCertificate) { $sig.SignerCertificate.Subject } else { "no signature" }
        "{0}: {1} ({2})" -f $file, $sig.Status, $signer
    }
} else {
    "Unsigned build (no -SignMetadata)."
}
"Installer: $setup"

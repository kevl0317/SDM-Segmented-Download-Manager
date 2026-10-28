; Inno Setup script for SDM. Build the app, then compile the installer:
;   cargo build --release -p sdm-gui
;   iscc SDM.iss
; Produces dist\SDM-Setup-<version>.exe -- a per-user installer (no admin),
; with a Start-Menu shortcut, optional desktop shortcut, and an uninstaller.
; The AppId is the one v1.x used, so installing over v1.x upgrades it in place.
;
; Signed builds (scripts\build-installer.ps1 does all of this): compile with
;   iscc /DSIGN "/Ssdmsign=<sign command; $f = the file, $q = a quote>" SDM.iss
; to sign SDM.exe, the installer and the uninstaller with the same identity.

#define MyAppName "SDM"
#define MyAppVersion "2.0.1"
#define MyAppPublisher "Kevin Liu"
#define MyAppURL "https://github.com/kevl0317/SDM-Segmented-Download-Manager"
#define MyAppExeName "SDM.exe"
#ifdef SIGN
  #define SignFlag " sign"
#else
  #define SignFlag ""
#endif

[Setup]
AppId={{B3F1A2C4-7E5D-4A8B-9C6F-1D2E3F4A5B6C}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
VersionInfoVersion={#MyAppVersion}
; Per-user install: {autopf} -> %LocalAppData%\Programs under PrivilegesRequired=lowest
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
; A 64-bit build for Windows 10 and later (ARM64 runs it under emulation).
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir=dist
OutputBaseFilename=SDM-Setup-{#MyAppVersion}
SetupIconFile=sdm.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
; Closing SDM's window only hides it to the tray, so a running copy has to be
; closed forcibly before SDM.exe can be replaced.
CloseApplications=force
#ifdef SIGN
SignTool=sdmsign
SignedUninstaller=yes
#endif

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Additional icons:"; Flags: unchecked

[Files]
Source: "target\release\sdm-gui.exe"; DestDir: "{app}"; DestName: "{#MyAppExeName}"; Flags: ignoreversion{#SignFlag}
Source: "extension\*"; DestDir: "{app}\extension"; Flags: ignoreversion recursesubdirs; Excludes: "*.pem,.sdm-key-info.json"

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\Uninstall {#MyAppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Parameters: "--register-host"; Flags: runhidden
Filename: "{app}\{#MyAppExeName}"; Description: "Launch {#MyAppName}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; SDM lives on in the tray, and a running SDM.exe can't be removed: ask it to
; quit first (it saves its queue and waits until it has exited).
Filename: "{app}\{#MyAppExeName}"; Parameters: "--quit"; Flags: runhidden; RunOnceId: "QuitSdm"
Filename: "{app}\{#MyAppExeName}"; Parameters: "--unregister-host"; Flags: runhidden; RunOnceId: "UnregisterSdmHost"

; Inno Setup script for SDM. Compile with:  iscc SDM.iss
; Produces dist\SDM-Setup-<version>.exe -- a per-user installer (no admin),
; with a Start-Menu shortcut, optional desktop shortcut, and an uninstaller.

#define MyAppName "SDM"
#define MyAppVersion "1.2.0"
#define MyAppPublisher "Kevin Liu"
#define MyAppURL "https://github.com/kevl0317/SDM"
#define MyAppExeName "SDM.exe"

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
OutputDir=dist
OutputBaseFilename=SDM-Setup-{#MyAppVersion}
SetupIconFile=sdm.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Additional icons:"; Flags: unchecked

[Files]
Source: "dist\SDM.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\Uninstall {#MyAppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "Launch {#MyAppName}"; Flags: nowait postinstall skipifsilent

; Inno Setup script for SpoutPeak.
; Build (from the repo root):
;   pwsh installer\fetch-ffmpeg.ps1      (once, or to update ffmpeg)
;   cargo build --release
;   & "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe" installer\spoutpeak.iss

#define AppName "SpoutPeak"
#define AppVersion "0.1.0"
#define AppExe "spoutpeak.exe"

[Setup]
; Keep this AppId unchanged across releases so upgrades replace the old install.
AppId={{AD4104FD-A375-4023-909E-EBCEFA67D523}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppName}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\{#AppExe}
OutputDir=..\dist
OutputBaseFilename=SpoutPeak-Setup-{#AppVersion}
SetupIconFile=..\target\release\spoutpeak.ico
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Per-user install by default (no admin prompt); users can choose all-users.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
; LGPL ffmpeg build (see installer\fetch-ffmpeg.ps1), with its license.
Source: "..\vendor\ffmpeg\*"; DestDir: "{app}\ffmpeg"; Flags: ignoreversion recursesubdirs

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

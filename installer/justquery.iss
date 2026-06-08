; Inno Setup script for JustQuery — produces a silent-capable Windows installer
; that winget can drive (InstallerType: inno, /VERYSILENT).
; Build locally:  iscc installer\justquery.iss
; CI overrides the version: iscc /DMyAppVersion=1.2.3 installer\justquery.iss

#define MyAppName "JustQuery"
#define MyAppPublisher "votinvv"
#define MyAppExeName "justquery.exe"
#define MyAppURL "https://github.com/votinvv/justquery"
#ifndef MyAppVersion
  #define MyAppVersion "0.1.0"
#endif

[Setup]
; AppId uniquely identifies the app for install/upgrade/uninstall — keep it STABLE across versions.
AppId={{8F2A6C31-4B7D-4E9A-9C12-7A3E5D8B1F40}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
LicenseFile=..\LICENSE
OutputDir=..\dist
OutputBaseFilename=JustQuery-{#MyAppVersion}-setup
SetupIconFile=..\assets\justquery.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent

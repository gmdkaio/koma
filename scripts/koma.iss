; built by scripts/package-windows.sh, which passes AppVersion.
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{6F3B1E52-8C0A-4D7E-9B21-5A4C2F0D9E17}
AppName=koma
AppVersion={#AppVersion}
AppPublisher=koma
DefaultDirName={autopf}\koma
DefaultGroupName=koma
DisableProgramGroupPage=yes
PrivilegesRequiredOverridesAllowed=dialog
OutputDir=..\dist
OutputBaseFilename=koma-setup
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\koma.ico
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Files]
Source: "..\dist\koma\*"; DestDir: "{app}"; Flags: recursesubdirs ignoreversion
Source: "..\assets\icon.ico"; DestDir: "{app}"; DestName: "koma.ico"

[Icons]
Name: "{group}\koma"; Filename: "{app}\bin\koma.exe"; IconFilename: "{app}\koma.ico"
Name: "{autodesktop}\koma"; Filename: "{app}\bin\koma.exe"; IconFilename: "{app}\koma.ico"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Run]
Filename: "{app}\bin\koma.exe"; Description: "Launch koma"; Flags: nowait postinstall skipifsilent

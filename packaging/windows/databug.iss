; Inno Setup script for the DataBug Windows installer.
; Build: iscc /DAppVersion=0.1.0 /DSourceExe=..\..\target\release\databug.exe packaging\windows\databug.iss
; Output: dist\DataBug-windows-x86_64-setup.exe
;
; Installs for the current user by default (no admin prompt); the first
; page offers "all users" instead. Adds Start menu and optional desktop
; shortcuts, and an uninstaller in Settings → Apps.

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef SourceExe
  #define SourceExe "..\..\target\release\databug.exe"
#endif

[Setup]
AppId={{6F0C1D8E-3B7A-4E52-9C1D-DA7ABA6B0001}
AppName=DataBug
AppVersion={#AppVersion}
AppVerName=DataBug {#AppVersion}
AppPublisher=DataBug
AppPublisherURL=https://github.com/hayes-roach/DataBug
AppSupportURL=https://github.com/hayes-roach/DataBug/issues
DefaultDirName={autopf}\DataBug
DefaultGroupName=DataBug
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\..\dist
OutputBaseFilename=DataBug-windows-x86_64-setup
SetupIconFile=..\..\crates\databug-app\assets\icon\icon.ico
UninstallDisplayIcon={app}\DataBug.exe
UninstallDisplayName=DataBug
LicenseFile=..\..\LICENSE
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[InstallDelete]
; 0.3.0 to 0.4.x shipped two-letter placeholder tiles for these engines; the logos are
; installed only when missing (so your own replacements survive), so remove the
; old placeholders once to let the real logos in.
Type: files; Name: "{app}\logos\oracle.svg"
Type: files; Name: "{app}\logos\db2.svg"
Type: files; Name: "{app}\logos\duckdb.svg"
Type: files; Name: "{app}\logos\cassandra.svg"
Type: files; Name: "{app}\logos\cockroachdb.svg"
Type: files; Name: "{app}\logos\timescaledb.svg"
Type: files; Name: "{app}\logos\questdb.svg"
Type: files; Name: "{app}\logos\influxdb.svg"
Type: files; Name: "{app}\logos\spark.svg"
Type: files; Name: "{app}\logos\h2.svg"

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "DataBug.exe"; Flags: ignoreversion
Source: "..\..\crates\databug-app\assets\logos\*"; DestDir: "{app}\logos"; Flags: ignoreversion onlyifdoesntexist
Source: "..\..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "..\..\NOTICE"; DestDir: "{app}"; DestName: "NOTICE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\DataBug"; Filename: "{app}\DataBug.exe"
Name: "{autodesktop}\DataBug"; Filename: "{app}\DataBug.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\DataBug.exe"; Description: "{cm:LaunchProgram,DataBug}"; Flags: nowait postinstall skipifsilent
; DataBug's in-app updater runs this setup silently with /relaunch=1
Filename: "{app}\DataBug.exe"; Flags: nowait; Check: ShouldRelaunch

[Code]
function ShouldRelaunch: Boolean;
begin
  Result := WizardSilent and (ExpandConstant('{param:relaunch|0}') = '1');
end;

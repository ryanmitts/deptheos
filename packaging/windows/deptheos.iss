; Inno Setup script for the Windows installer (script/bundle-windows.ps1
; runs it). A per-user install: no administrator prompt, and the updater
; can rerun it in place.
;
; Defines passed by the build script:
;   AppVersion  e.g. 0.1.0
;   SourceExe   the release deptheos-gui.exe
;   CliExe      the release deptheos.exe, the command line tool
;   RuntimeDir  the C++ runtime DLLs it loads, copied from Visual Studio's
;               redistributable folder (docs/windows.md)
;   OutputDir   where the installer goes
; Signing, when it is set up: pass /DSign and /Sdeptheos="<sign command> $f"
; to iscc, and Inno signs the installer and its uninstaller.

#define AppName "Deptheos"

[Setup]
; Never change AppId: Windows matches upgrades and uninstalls by it.
AppId={{CFDC2FBF-1867-4001-86A4-09C2B657C283}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Ryan Mitts
AppPublisherURL=https://deptheos.com
AppSupportURL=https://github.com/ryanmitts/deptheos/issues
AppUpdatesURL=https://github.com/ryanmitts/deptheos/releases
; Per user: {autopf} is %LOCALAPPDATA%\Programs. No choice of all users
; is offered: the updater reruns this without an administrator, and PATH
; below is the user's.
PrivilegesRequired=lowest
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
LicenseFile=..\..\LICENSE.md
OutputDir={#OutputDir}
OutputBaseFilename=Deptheos-{#AppVersion}-windows-x64-setup
SetupIconFile=deptheos.ico
UninstallDisplayIcon={app}\deptheos-gui.exe
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Close a running copy before replacing it, and reopen it afterwards.
CloseApplications=yes
RestartApplications=yes
#ifdef Sign
SignTool=deptheos
SignedUninstaller=yes
#endif

; Tell running programs (Explorer, new terminals) that PATH changed.
ChangesEnvironment=yes

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "addtopath"; Description: "Add the deptheos command to PATH, for the command line"; GroupDescription: "Command line:"

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#CliExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RuntimeDir}\*.dll"; DestDir: "{app}"; Flags: ignoreversion
; .txt, which Windows opens without asking for an app (the About window
; opens these).
Source: "..\..\LICENSE.md"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "..\..\TRADEMARKS.md"; DestDir: "{app}"; DestName: "TRADEMARKS.txt"; Flags: ignoreversion
Source: "..\..\THIRD_PARTY_NOTICES.txt"; DestDir: "{app}"; Flags: ignoreversion
; The sample stacks, which Try a Sample Stack copies into the Library folder
; (deptheos_gpui's app/sample.rs finds them at samples\hornet and samples\pcb
; beside the exe): their frames, not samples' README.
Source: "..\..\samples\hornet\*.jpg"; DestDir: "{app}\samples\hornet"; Flags: ignoreversion
Source: "..\..\samples\pcb\*.jpg"; DestDir: "{app}\samples\pcb"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\deptheos-gui.exe"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\deptheos-gui.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\deptheos-gui.exe"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

; The user's PATH (per user, like the install: no administrator needed).
; Added once, and taken out again on uninstall.
[Code]
function PathHas(Path, Dir: string): Boolean;
begin
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Path) + ';') > 0;
end;

// Added with one ';' between, and none for an empty PATH or one that
// ends with its own.
procedure CurStepChanged(CurStep: TSetupStep);
var
  Path, Dir: string;
begin
  if (CurStep <> ssPostInstall) or not WizardIsTaskSelected('addtopath') then
    exit;
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Path) then
    Path := '';
  Dir := ExpandConstant('{app}');
  if PathHas(Path, Dir) then
    exit;
  if (Path <> '') and (Path[Length(Path)] <> ';') then
    Path := Path + ';';
  RegWriteExpandStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Path + Dir);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Path, Dir: string;
  P: Integer;
begin
  if CurUninstallStep <> usPostUninstall then
    exit;
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Path) then
    exit;
  Dir := ExpandConstant('{app}');
  Path := ';' + Path + ';';
  P := Pos(';' + Uppercase(Dir) + ';', Uppercase(Path));
  if P = 0 then
    exit;
  Delete(Path, P, Length(Dir) + 1);
  Path := Copy(Path, 2, Length(Path) - 2);
  RegWriteExpandStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Path);
end;

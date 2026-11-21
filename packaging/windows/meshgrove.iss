; Inno Setup script for the Windows installer, built by .github/workflows/release.yml:
;   iscc /DAppVersion=0.1.0 /DBinDir=C:\path\to\target\release /DOutputDir=C:\out packaging\windows\meshgrove.iss
; Installs for the current user without admin rights; the first page also offers an
; installation for all users.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef BinDir
  #define BinDir "..\..\target\release"
#endif
#ifndef OutputDir
  #define OutputDir "..\.."
#endif
#define RepoDir "..\.."

[Setup]
AppId={{41E2E309-39C6-4F4F-8C14-F37A11629AF8}
AppName=Meshgrove
AppVersion={#AppVersion}
AppVerName=Meshgrove {#AppVersion}
AppPublisher=Davis and Rec0iL, German Redneck Engineering
AppPublisherURL=https://redneck-design.de/
AppSupportURL=https://github.com/DonDavis-vibe/MeshGrove/issues
AppUpdatesURL=https://github.com/DonDavis-vibe/MeshGrove/releases
AppCopyright=Copyright (C) 2026 Davis and Rec0iL, German Redneck Engineering. GPL-3.0-or-later.
DefaultDirName={autopf}\Meshgrove
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
ChangesAssociations=yes
ShowLanguageDialog=auto
WizardStyle=modern
SetupIconFile={#RepoDir}\packaging\icons\meshgrove.ico
UninstallDisplayIcon={app}\meshgrove.exe
UninstallDisplayName=Meshgrove
OutputDir={#OutputDir}
OutputBaseFilename=Meshgrove-{#AppVersion}-windows-x64-setup
Compression=lzma2/ultra64
SolidCompression=yes

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "de"; MessagesFile: "compiler:Languages\German.isl"

[CustomMessages]
en.ModelFile=3D model
de.ModelFile=3D-Modell

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#BinDir}\meshgrove.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\meshgrove-step-mesher.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\meshgrove-mcp.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoDir}\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"
Source: "{#RepoDir}\THIRD-PARTY-LICENSES.md"; DestDir: "{app}"
Source: "{#RepoDir}\README.md"; DestDir: "{app}"

[Icons]
Name: "{autoprograms}\Meshgrove"; Filename: "{app}\meshgrove.exe"
Name: "{autodesktop}\Meshgrove"; Filename: "{app}\meshgrove.exe"; Tasks: desktopicon

[Registry]
; Meshgrove under "Open with" for its model formats; the default program stays as it is.
Root: HKA; Subkey: "Software\Classes\Meshgrove.Model"; ValueType: string; ValueName: ""; ValueData: "{cm:ModelFile}"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Meshgrove.Model\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\meshgrove.exe,0"
Root: HKA; Subkey: "Software\Classes\Meshgrove.Model\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\meshgrove.exe"" ""%1"""
Root: HKA; Subkey: "Software\Classes\.stl\OpenWithProgids"; ValueType: string; ValueName: "Meshgrove.Model"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.3mf\OpenWithProgids"; ValueType: string; ValueName: "Meshgrove.Model"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.step\OpenWithProgids"; ValueType: string; ValueName: "Meshgrove.Model"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.stp\OpenWithProgids"; ValueType: string; ValueName: "Meshgrove.Model"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.obj\OpenWithProgids"; ValueType: string; ValueName: "Meshgrove.Model"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.ply\OpenWithProgids"; ValueType: string; ValueName: "Meshgrove.Model"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.glb\OpenWithProgids"; ValueType: string; ValueName: "Meshgrove.Model"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.gltf\OpenWithProgids"; ValueType: string; ValueName: "Meshgrove.Model"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\Applications\meshgrove.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "Meshgrove"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Applications\meshgrove.exe\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\meshgrove.exe,0"
Root: HKA; Subkey: "Software\Classes\Applications\meshgrove.exe\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\meshgrove.exe"" ""%1"""
Root: HKA; Subkey: "Software\Classes\Applications\meshgrove.exe\SupportedTypes"; ValueType: string; ValueName: ".stl"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\meshgrove.exe\SupportedTypes"; ValueType: string; ValueName: ".3mf"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\meshgrove.exe\SupportedTypes"; ValueType: string; ValueName: ".step"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\meshgrove.exe\SupportedTypes"; ValueType: string; ValueName: ".stp"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\meshgrove.exe\SupportedTypes"; ValueType: string; ValueName: ".obj"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\meshgrove.exe\SupportedTypes"; ValueType: string; ValueName: ".ply"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\meshgrove.exe\SupportedTypes"; ValueType: string; ValueName: ".glb"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\meshgrove.exe\SupportedTypes"; ValueType: string; ValueName: ".gltf"; ValueData: ""

[Run]
Filename: "{app}\meshgrove.exe"; Description: "{cm:LaunchProgram,Meshgrove}"; Flags: nowait postinstall skipifsilent
; After Meshgrove's own update (it runs this installer quietly with /relaunch=1 and has closed):
; start the new version, as the user, not as the administrator an all-users update ran as.
Filename: "{app}\meshgrove.exe"; Flags: nowait runasoriginaluser; Check: RelaunchAfterUpdate

[Code]
function RelaunchAfterUpdate: Boolean;
begin
  Result := WizardSilent and (ExpandConstant('{param:relaunch|0}') = '1');
end;

// Meshgrove closes once this installer runs (it watches for the log). Wait until it has (until
// meshgrove.exe can be opened for writing), at most a minute: the Restart Manager would otherwise
// find it still saving, fail to close a program whose window is already gone, and the quiet
// installer would roll back.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Exe: String;
  Waited: Integer;
  Stream: TFileStream;
begin
  Result := '';
  if not RelaunchAfterUpdate then
    Exit;
  Exe := ExpandConstant('{app}\meshgrove.exe');
  Waited := 0;
  while FileExists(Exe) and (Waited < 60000) do
  begin
    try
      Stream := TFileStream.Create(Exe, fmOpenReadWrite or fmShareExclusive);
      Stream.Free;
      Log(Format('Meshgrove has closed (waited %d ms).', [Waited]));
      Exit;
    except
      Sleep(200);
      Waited := Waited + 200;
    end;
  end;
  Log(Format('Meshgrove still runs after %d ms; the Restart Manager takes over.', [Waited]));
end;

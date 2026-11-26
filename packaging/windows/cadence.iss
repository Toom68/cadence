; Inno Setup script for the Cadence Windows installer.
;
; Built by scripts/windows-package.ps1 (it passes the defines below):
;   ISCC /DAppVersion=1.2.0 /DSourceDir=<staged app folder> /DOutputDir=dist
;        /DRepoRoot=<repo> [/DRequireSoundFont=1] packaging\windows\cadence.iss
;
; The staged folder holds soundfonts\Cadence GM.sf2 (the built-in General
; MIDI SoundFont, GeneralUser GS) and soundfonts\Cadence Harmonium.sf2 (the
; sampled harmonium), see packaging/soundfont/README.md; [Files] installs
; them with everything else. /DRequireSoundFont=1 (CI) refuses to build an
; installer without either.
;
; Per-user install into %LOCALAPPDATA%\Programs\Cadence: no admin prompt,
; ever, so the in-app updater can run a newer setup silently. The AppId
; must never change, or updates install side by side instead of upgrading.
; The setup is unsigned, like Cadence.exe.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef RepoRoot
  #define RepoRoot "..\.."
#endif
#ifndef SourceDir
  #define SourceDir RepoRoot + "\dist\Cadence-" + AppVersion + "-windows-x64"
#endif
#ifndef OutputDir
  #define OutputDir RepoRoot + "\dist"
#endif
#ifdef RequireSoundFont
  #if !FileExists(SourceDir + "\soundfonts\Cadence GM.sf2")
    #error The built-in SoundFont is missing from the staged app (soundfonts\Cadence GM.sf2); run scripts\windows-package.ps1
  #endif
  #if !FileExists(SourceDir + "\soundfonts\Cadence Harmonium.sf2")
    #error The built-in harmonium SoundFont is missing from the staged app (soundfonts\Cadence Harmonium.sf2); run scripts\windows-package.ps1
  #endif
#endif

[Setup]
AppId={{70714BE1-6A64-4AC4-B737-7351AA386AB6}
AppName=Cadence
AppVersion={#AppVersion}
AppVerName=Cadence {#AppVersion}
AppPublisher=Cadence
AppPublisherURL=https://github.com/Toom68/cadence
AppSupportURL=https://github.com/Toom68/cadence/issues
AppUpdatesURL=https://github.com/Toom68/cadence/releases/latest
AppCopyright=Cadence contributors
VersionInfoVersion={#AppVersion}
VersionInfoProductName=Cadence
VersionInfoDescription=Cadence Setup
PrivilegesRequired=lowest
DefaultDirName={autopf}\Cadence
DisableProgramGroupPage=yes
DefaultGroupName=Cadence
UninstallDisplayName=Cadence
UninstallDisplayIcon={app}\Cadence.exe
SetupIconFile={#RepoRoot}\assets\icon.ico
OutputDir={#OutputDir}
OutputBaseFilename=Cadence-{#AppVersion}-windows-x64-setup
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ChangesAssociations=yes
CloseApplications=yes
RestartApplications=no
#if FileExists(RepoRoot + "\EULA.txt")
LicenseFile={#RepoRoot}\EULA.txt
#endif

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Cadence"; Filename: "{app}\Cadence.exe"
Name: "{autodesktop}\Cadence"; Filename: "{app}\Cadence.exe"; Tasks: desktopicon

[Registry]
; Double-clicking a .cadence song opens it in Cadence (per user).
Root: HKA; Subkey: "Software\Classes\.cadence"; ValueType: string; ValueName: ""; ValueData: "Cadence.Song"; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.cadence\OpenWithProgids"; ValueType: string; ValueName: "Cadence.Song"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\Cadence.Song"; ValueType: string; ValueName: ""; ValueData: "Cadence Song"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Cadence.Song\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\Cadence.exe,0"
Root: HKA; Subkey: "Software\Classes\Cadence.Song\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\Cadence.exe"" ""%1"""

[Run]
; Also runs after a silent in-app update (Restart & Update); the updater
; passes /norelaunch=1 when the user chose "install when I quit".
Filename: "{app}\Cadence.exe"; Description: "{cm:LaunchProgram,Cadence}"; Flags: nowait postinstall; Check: ShouldLaunch

[Code]
function ShouldLaunch: Boolean;
begin
  Result := ExpandConstant('{param:norelaunch|0}') <> '1';
end;

; Jilo Studio installer for Windows 10 and 11 (Inno Setup 6). Built by package.ps1:
;
;   ISCC.exe /DVersion=1.0.0 /DBinDir=<staged exes> /DRoot=<repo> /DOutDir=<dist> jilo-studio.iss
;
; Installs for the current user by default (no administrator rights, %LOCALAPPDATA%\Programs\Jilo
; Studio); the dialog at the start offers all users instead (Program Files, needs elevation).
; Start menu shortcut, optional desktop shortcut, .jsproj / .jstpl open with Jilo Studio, and an
; uninstaller in Settings > Apps. Preferences, presets, models and logs live in the data folder
; (%APPDATA%\com.rustamguloff.jilostudio) and are kept on uninstall, as on the Mac.

#ifndef Version
  #define Version "0.0.0"
#endif
#ifndef Root
  #define Root "..\.."
#endif
#ifndef BinDir
  #define BinDir Root + "\target\x86_64-pc-windows-gnu\release"
#endif
#ifndef OutDir
  #define OutDir Root + "\dist\release"
#endif

[Setup]
AppId={{6F2C8A51-3B7E-4C1D-9A2F-5E8B7D4C3A10}
AppName=Jilo Studio
AppVersion={#Version}
AppVerName=Jilo Studio {#Version}
AppPublisher=Rustam Gulov
AppPublisherURL=https://rustamguloff.com/en/page/projects/jilo-studio
AppSupportURL=https://github.com/guloff/jilo-studio/issues
AppUpdatesURL=https://github.com/guloff/jilo-studio/releases/latest
AppCopyright=Copyright (c) Rustam Gulov and the Jilo Studio contributors
VersionInfoVersion={#Version}
VersionInfoProductName=Jilo Studio
VersionInfoDescription=Jilo Studio installer
DefaultDirName={autopf}\Jilo Studio
DefaultGroupName=Jilo Studio
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
ChangesAssociations=yes
CloseApplications=yes
RestartApplications=no
SetupIconFile={#Root}\assets\brand\jilo\jilo-studio.ico
UninstallDisplayIcon={app}\jilo-studio.exe
UninstallDisplayName=Jilo Studio
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
OutputDir={#OutDir}
OutputBaseFilename=jilo-studio-{#Version}-windows-x64-setup
LicenseFile={#Root}\LICENSE-MIT

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "ru"; MessagesFile: "compiler:Languages\Russian.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "{#BinDir}\jilo-studio.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\jilo-cli.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\assets\brand\jilo\jilo-studio.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\NOTICE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\ATTRIBUTION.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\assets\brand\jilo\LICENSE-brand.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Jilo Studio"; Filename: "{app}\jilo-studio.exe"; WorkingDir: "{app}"; Comment: "AI-first video editor"
Name: "{autodesktop}\Jilo Studio"; Filename: "{app}\jilo-studio.exe"; WorkingDir: "{app}"; Comment: "AI-first video editor"; Tasks: desktopicon

[Registry]
; Projects (.jsproj) and graphics templates (.jstpl) open with Jilo Studio
Root: HKA; Subkey: "Software\Classes\.jsproj"; ValueType: string; ValueName: ""; ValueData: "JiloStudio.Project"; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.jsproj\OpenWithProgids"; ValueType: string; ValueName: "JiloStudio.Project"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\JiloStudio.Project"; ValueType: string; ValueName: ""; ValueData: "Jilo Studio Project"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\JiloStudio.Project\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\jilo-studio.exe,0"
Root: HKA; Subkey: "Software\Classes\JiloStudio.Project\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\jilo-studio.exe"" ""%1"""
Root: HKA; Subkey: "Software\Classes\.jstpl"; ValueType: string; ValueName: ""; ValueData: "JiloStudio.Template"; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.jstpl\OpenWithProgids"; ValueType: string; ValueName: "JiloStudio.Template"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\JiloStudio.Template"; ValueType: string; ValueName: ""; ValueData: "Jilo Studio Graphics Template"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\JiloStudio.Template\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\jilo-studio.exe,0"
Root: HKA; Subkey: "Software\Classes\JiloStudio.Template\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\jilo-studio.exe"" ""%1"""
; "Open with" lists Jilo Studio for video, audio and images it imports
Root: HKA; Subkey: "Software\Classes\Applications\jilo-studio.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "Jilo Studio"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Applications\jilo-studio.exe\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\jilo-studio.exe"" ""%1"""
Root: HKA; Subkey: "Software\Classes\Applications\jilo-studio.exe\SupportedTypes"; ValueType: string; ValueName: ".mp4"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\jilo-studio.exe\SupportedTypes"; ValueType: string; ValueName: ".mov"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\jilo-studio.exe\SupportedTypes"; ValueType: string; ValueName: ".mkv"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\jilo-studio.exe\SupportedTypes"; ValueType: string; ValueName: ".jsproj"; ValueData: ""
; "jilo-studio" from Win+R and the command line
Root: HKA; Subkey: "Software\Microsoft\Windows\CurrentVersion\App Paths\jilo-studio.exe"; ValueType: string; ValueName: ""; ValueData: "{app}\jilo-studio.exe"; Flags: uninsdeletekey

[Run]
Filename: "{app}\jilo-studio.exe"; Description: "{cm:LaunchProgram,Jilo Studio}"; Flags: nowait postinstall skipifsilent
; An update from inside the app (Help > Check for Updates) runs this installer silently with
; /RELAUNCH=1: the new version opens when it is in place (crates/engine/src/updater.rs)
Filename: "{app}\jilo-studio.exe"; Flags: nowait; Check: RelaunchAfterUpdate

[Code]
function RelaunchAfterUpdate: Boolean;
begin
  Result := WizardSilent and (ExpandConstant('{param:RELAUNCH|0}') = '1');
end;

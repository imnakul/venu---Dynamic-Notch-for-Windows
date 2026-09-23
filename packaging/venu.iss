; Inno Setup script for Venu - per-user install, no elevation required.
;
; Build:  iscc /DMyAppVersion=0.3.0 packaging\venu.iss
; (the version define is optional; release.yml passes the tag version)
;
; Layout mirrors what the app expects:
;   - the autostart task writes the same Run-key value the app itself
;     manages from Settings > App > Preferences, so the two stay in step
;   - the uninstaller removes the Run value too, even when the app created
;     it after the install

#ifndef MyAppVersion
#define MyAppVersion "0.4.1"
#endif

#define MyAppName "Venu"
#define MyAppExeName "venu.exe"

[Setup]
AppId={{7E1A5C0F-3B94-4D28-9F6A-2C8E5B1D7A34}
AppVersion={#MyAppVersion}
AppName={#MyAppName}
AppPublisher=Venu
AppPublisherURL=https://github.com/imnakul/venu---Dynamic-Notch-for-Windows
; Per-user install into %LOCALAPPDATA%\Programs\Venu - no admin prompt.
PrivilegesRequired=lowest
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
UninstallDisplayName={#MyAppName} - Dynamic Notch for Windows
SetupIconFile=..\assets\venu.ico
OutputBaseFilename=venu-setup-{#MyAppVersion}
OutputDir=..\dist
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesInstallIn64BitMode=x64
; Venu keeps running in the tray; the Restart Manager closes it for the copy.
CloseApplications=yes
RestartApplications=no
; User settings (%APPDATA%\venu\config.json) deliberately survive an
; uninstall so a reinstall comes back exactly as it was left.

[Tasks]
Name: "autostart"; \
    Description: "Start Venu when Windows starts (quietly, in the tray)"; \
    GroupDescription: "Startup:"; \
    Flags: checkedonce
Name: "desktopicon"; \
    Description: "Create a &desktop shortcut"; \
    GroupDescription: "Shortcuts:"; \
    Flags: unchecked

[Files]
Source: "..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\Uninstall {#MyAppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Registry]
; Written before the first run; Venu adopts it and refreshes the path (and
; appends --startup) on every later start.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; \
    ValueType: string; ValueName: "Venu"; \
    ValueData: """{app}\{#MyAppExeName}"""; \
    Tasks: autostart; Flags: uninsdeletevalue

[Run]
Filename: "{app}\{#MyAppExeName}"; \
    Description: "{cm:LaunchProgram,{#MyAppName}}"; \
    Flags: nowait postinstall skipifsilent

[Code]
// The app manages the Run value itself once it runs; remove it here as well
// so an app-created entry never survives an uninstall either.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
  begin
    RegDeleteValue(HKEY_CURRENT_USER,
      'Software\Microsoft\Windows\CurrentVersion\Run', 'Venu');
    RegDeleteValue(HKEY_CURRENT_USER,
      'Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run',
      'Venu');
  end;
end;

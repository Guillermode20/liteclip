; LiteClip Installer — rewritten for reliability and proper cleanup
; https://jrsoftware.org/ishelp/index.php?topic=setup

#define MyAppName "LiteClip"
#define MyAppVersion "1.0.0"
#define MyAppPublisher "LiteClip"
#define MyAppURL "https://github.com/Guillermode20/smart-compressor"
#define MyAppExeName "liteclip.exe"
#define MyAppId "A1B2C3D4-E5F6-7890-1234-567890ABCDEF"

[Setup]
AppId={#MyAppId}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
AppUpdatesURL={#MyAppURL}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableDirPage=no
DisableProgramGroupPage=no
DisableFinishedPage=no
AppCopyright=LiteClip
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64
ArchitecturesInstallIn64BitMode=x64
Uninstallable=yes
CreateUninstallRegKey=yes
OutputDir=..\dist
OutputBaseFilename=LiteClip-Setup

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked; Components: main
Name: "uninstallicon"; Description: "Create an Uninstall icon"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked; Components: main

[Components]
Name: "main"; Description: "Main program files"; Types: full compact custom

[Files]
Source: "..\publish-win\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion; Components: main
Source: "..\publish-win\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs; Components: main

[Icons]
Name: "{autoprograms}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Components: main
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon; Components: main
Name: "{autoprograms}\{#MyAppName}\Uninstall {#MyAppName}"; Filename: "{uninstallexe}"; Tasks: uninstallicon; Components: main
Name: "{autodesktop}\Uninstall {#MyAppName}"; Filename: "{uninstallexe}"; Tasks: uninstallicon; Components: main

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#MyAppName}}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
Type: filesandordirs; Name: "{app}"
Type: dirifempty; Name: "{autodesktop}\{#MyAppName}"
Type: files; Name: "{autoprograms}\{#MyAppName}\*"

[Code]
function IsWebView2RuntimeInstalled(): Boolean;
var
  RegKey: String;
begin
  RegKey := 'SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients\\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}';
  Result := RegKeyExists(HKEY_LOCAL_MACHINE, RegKey);

  if not Result then
  begin
    RegKey := 'SOFTWARE\\Microsoft\\EdgeUpdate\\Clients\\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}';
    Result := RegKeyExists(HKEY_LOCAL_MACHINE, RegKey);
  end;

  if not Result then
  begin
    RegKey := 'Software\\Microsoft\\EdgeUpdate\\Clients\\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}';
    Result := RegKeyExists(HKEY_CURRENT_USER, RegKey);
  end;
end;

procedure InitializeWizard();
var
  ErrorCode: Integer;
begin
  if not IsWebView2RuntimeInstalled() then
  begin
    if MsgBox('Microsoft Edge WebView2 Runtime is required but not detected.' + #13#10 +
              'Do you want to download it now?', mbConfirmation, MB_YESNO) = IDYES then
    begin
      ShellExec('open', 'https://go.microsoft.com/fwlink/p/?LinkId=2124703', '', '', SW_SHOW, ewNoWait, ErrorCode);
    end;
  end;
end;

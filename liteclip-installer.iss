; Inno Setup Script for LiteClip
; Version: 1.0.0

#define MyAppName "LiteClip"
#define MyAppVersion "1.0.0"
#define MyAppPublisher "LiteClip Team"
#define MyAppURL "https://github.com/yourusername/liteclip"
#define MyAppExeName "liteclip.exe"

[Setup]
; NOTE: The value of AppId uniquely identifies this application.
; Do not use the same AppId value in installers for other applications.
AppId={{8A7F3C2D-9E4B-4A6C-B8D1-2F5E6A7C8D9E}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
AppUpdatesURL={#MyAppURL}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
AllowNoIcons=yes
OutputDir=dist
OutputBaseFilename=LiteClip-Setup-{#MyAppVersion}
Compression=lzma
SolidCompression=yes
; Windows 10 and above (version 10.0 = Windows 10)
MinVersion=10.0
ArchitecturesAllowed=x64
ArchitecturesInstallIn64BitMode=x64
; No admin rights required - installs to user's Program Files
PrivilegesRequired=lowest
; Wizard style
WizardStyle=modern
; Uninstall display icon
UninstallDisplayIcon={app}\{#MyAppExeName}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "startmenu"; Description: "Create a &Start Menu shortcut"; GroupDescription: "Additional shortcuts:"

[Files]
Source: "publish-win\liteclip.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: startmenu
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"; Tasks: startmenu

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent

[Code]
procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
begin
  if CurStep = ssPostInstall then
  begin
    MsgBox('LiteClip has been installed successfully!' + #13#10 + #13#10 + 
           'IMPORTANT: FFmpeg is required for video compression.' + #13#10 + #13#10 + 
           'If you do not have FFmpeg installed:' + #13#10 + 
           '1. Download FFmpeg from https://ffmpeg.org/download.html' + #13#10 + 
           '2. Install it and ensure it is in your system PATH' + #13#10 + 
           '   OR place ffmpeg.exe in the same directory as liteclip.exe' + #13#10 + #13#10 + 
           'Note: WebView2 Runtime is required (usually pre-installed on Windows 10/11).' + #13#10 + 
           'If the app does not start, download WebView2 from:' + #13#10 + 
           'https://developer.microsoft.com/microsoft-edge/webview2/', 
           mbInformation, MB_OK);
  end;
end;


; Inno Setup script for AndroidFS-F.

#define MyAppName "AndroidFS-F"
#define MyAppVersion "0.2.1"
#define MyAppPublisher "AndroidFS-F"
#define MyAppURL "https://github.com/Lauriethefish/AndroidFS"
#define MyAppExeName "androidfs_driver.exe"

[Setup]
AppId={{BAEB4BF1-971E-4801-993A-CDEBB92392FF}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
AppUpdatesURL={#MyAppURL}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
LicenseFile=..\LICENSE
InfoAfterFile=.\information.txt
OutputDir=.\
OutputBaseFilename=androidfs_f_installer
Compression=lzma
SolidCompression=yes
WizardStyle=modern
ArchitecturesInstallIn64BitMode=x64
PrivilegesRequired=admin
AlwaysRestart=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Files]
Source: "..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: ".\Dokan_x64.msi"; DestDir: "{tmp}"; DestName: "Dokan_x64.msi"; Flags: deleteafterinstall

[Run]
Filename: "msiexec.exe"; Parameters: "/i ""{tmp}\Dokan_x64.msi"" /qn /norestart"; StatusMsg: "Installing Dokan..."; Flags: waituntilterminated

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{commonstartup}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"

[UninstallDelete]
Type: filesandordirs; Name: "{app}"

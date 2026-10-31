Unicode true
!include "MUI2.nsh"
Name "Argus Cluster 0.2.0"
OutFile "/workspace/work/out/Argus-Setup.exe"
InstallDir "$LOCALAPPDATA\Programs\Argus Cluster"
RequestExecutionLevel user
SetCompressor /SOLID lzma
!define MUI_ICON "/workspace/projects/e40f4c0d-e664-4683-bfdc-2a267f6d4dee/usb-cluster/desktop/src-tauri/icons/icon.ico"
!define MUI_FINISHPAGE_RUN "$INSTDIR\Argus.exe"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "Russian"
!insertmacro MUI_LANGUAGE "English"
Section
  nsExec::Exec 'taskkill /F /IM Argus.exe'
  nsExec::Exec 'taskkill /F /IM llama-server.exe'
  SetOutPath "$INSTDIR"
  File /oname=Argus.exe "/workspace/tools/target/x86_64-pc-windows-msvc/release/usb-cluster.exe"
  SetOutPath "$INSTDIR\resources"
  File /x README.md "/workspace/projects/e40f4c0d-e664-4683-bfdc-2a267f6d4dee/usb-cluster/desktop/src-tauri/resources/*"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateShortcut "$DESKTOP\Argus Cluster.lnk" "$INSTDIR\Argus.exe"
  CreateShortcut "$SMPROGRAMS\Argus Cluster.lnk" "$INSTDIR\Argus.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ArgusCluster" "DisplayName" "Argus Cluster"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ArgusCluster" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ArgusCluster" "DisplayIcon" "$INSTDIR\Argus.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ArgusCluster" "DisplayVersion" "0.2.0"
  ; llama.cpp needs the MSVC runtime (MSVCP140/VCRUNTIME140); install silently if missing (one UAC prompt)
  ReadRegDWORD $1 HKLM "SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64" "Installed"
  StrCmp $1 "1" vcok
    SetOutPath "$TEMP"
    File "/workspace/work/nsis/vc_redist.x64.exe"
    ExecWait '"$TEMP\vc_redist.x64.exe" /install /quiet /norestart'
    Delete "$TEMP\vc_redist.x64.exe"
  vcok:
  ; WebView2 runtime ships with Windows 11 and updated Windows 10; install it if missing
  ReadRegStr $0 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
  StrCmp $0 "" 0 +2
    ExecShell "open" "https://go.microsoft.com/fwlink/p/?LinkId=2124703"
SectionEnd
Section "Uninstall"
  nsExec::Exec 'taskkill /F /IM Argus.exe'
  nsExec::Exec 'taskkill /F /IM adb.exe'
  RMDir /r "$INSTDIR"
  Delete "$DESKTOP\Argus Cluster.lnk"
  Delete "$SMPROGRAMS\Argus Cluster.lnk"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ArgusCluster"
SectionEnd

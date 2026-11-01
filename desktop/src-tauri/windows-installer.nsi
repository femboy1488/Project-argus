Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"

; Paths: defaults = the sandbox build; CI passes /DAPP_EXE=... /DRES_DIR=... /DOUT_FILE=... /DICON=... /DVC_REDIST=...
!ifndef VERSION
  !define VERSION "0.3.1"
!endif
!ifndef APP_EXE
  !define APP_EXE "/workspace/tools/target/x86_64-pc-windows-msvc/release/usb-cluster.exe"
!endif
!ifndef RES_DIR
  !define RES_DIR "/workspace/projects/e40f4c0d-e664-4683-bfdc-2a267f6d4dee/usb-cluster/desktop/src-tauri/resources"
!endif
!ifndef OUT_FILE
  !define OUT_FILE "/workspace/work/out/Argus-Setup.exe"
!endif
!ifndef ICON
  !define ICON "/workspace/projects/e40f4c0d-e664-4683-bfdc-2a267f6d4dee/usb-cluster/desktop/src-tauri/icons/icon.ico"
!endif
!ifndef VC_REDIST
  !define VC_REDIST "/workspace/work/nsis/vc_redist.x64.exe"
!endif

Name "Argus Cluster ${VERSION}"
OutFile "${OUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\Argus Cluster"
RequestExecutionLevel user
SetCompressor /SOLID lzma
VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "Argus Cluster"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "Argus Cluster installer"
VIAddVersionKey "LegalCopyright" "Argus"
!define MUI_ICON "${ICON}"
!define MUI_FINISHPAGE_RUN "$INSTDIR\Argus.exe"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "Russian"
!insertmacro MUI_LANGUAGE "English"

; /UPDATE = started by Argus itself after the user clicked «Обновить» (always together with /S).
; Silent mode over a running copy: wait for / kill Argus.exe, keep user data, relaunch Argus.exe at the end.
Var UpdateMode

Function .onInit
  StrCpy $UpdateMode 0
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "/UPDATE" $R1
  ${IfNot} ${Errors}
    StrCpy $UpdateMode 1
  ${EndIf}
  ; an update must land where the current copy lives (the registry knows; default otherwise)
  ReadRegStr $R2 HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ArgusCluster" "InstallLocation"
  ${If} $R2 != ""
    StrCpy $INSTDIR $R2
  ${EndIf}
FunctionEnd

Section
  ${If} $UpdateMode = 1
    Sleep 1500                                   ; Argus exits on its own right after starting us
  ${EndIf}
  nsExec::Exec 'taskkill /F /IM Argus.exe'
  nsExec::Exec 'taskkill /F /IM llama-server.exe'
  ; adb.exe from resources\ keeps running as the adb server and would block overwriting it
  ${If} ${FileExists} "$INSTDIR\resources\adb.exe"
    nsExec::Exec '"$INSTDIR\resources\adb.exe" kill-server'
  ${EndIf}
  Sleep 500
  SetOverwrite on
  SetOutPath "$INSTDIR"
  File /oname=Argus.exe "${APP_EXE}"
  SetOutPath "$INSTDIR\resources"
  File /x README.md "${RES_DIR}/*"
  ; user data is NOT here: settings in %APPDATA%\com.harkcluster.desktop, models in the llama.cpp/HF cache
  WriteUninstaller "$INSTDIR\uninstall.exe"
  ${IfNot} ${Silent}
    CreateShortcut "$DESKTOP\Argus Cluster.lnk" "$INSTDIR\Argus.exe"
  ${EndIf}
  CreateShortcut "$SMPROGRAMS\Argus Cluster.lnk" "$INSTDIR\Argus.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ArgusCluster" "DisplayName" "Argus Cluster"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ArgusCluster" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ArgusCluster" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ArgusCluster" "DisplayIcon" "$INSTDIR\Argus.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ArgusCluster" "DisplayVersion" "${VERSION}"
  ; llama.cpp needs the MSVC runtime (MSVCP140/VCRUNTIME140); install silently if missing (one UAC prompt)
  ReadRegDWORD $1 HKLM "SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64" "Installed"
  StrCmp $1 "1" vcok
    SetOutPath "$TEMP"
    File "${VC_REDIST}"
    ExecWait '"$TEMP\vc_redist.x64.exe" /install /quiet /norestart'
    Delete "$TEMP\vc_redist.x64.exe"
  vcok:
  ; WebView2 runtime ships with Windows 11 and updated Windows 10; offer it if missing (not during a silent update)
  ReadRegStr $0 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
  ${If} $0 == ""
  ${AndIfNot} ${Silent}
    ExecShell "open" "https://go.microsoft.com/fwlink/p/?LinkId=2124703"
  ${EndIf}
  ; silent install (self-update): start the new version again
  ${If} ${Silent}
  ${OrIf} $UpdateMode = 1
    SetOutPath "$INSTDIR"
    Exec '"$INSTDIR\Argus.exe"'
  ${EndIf}
SectionEnd

Section "Uninstall"
  nsExec::Exec 'taskkill /F /IM Argus.exe'
  ${If} ${FileExists} "$INSTDIR\resources\adb.exe"
    nsExec::Exec '"$INSTDIR\resources\adb.exe" kill-server'
  ${EndIf}
  RMDir /r "$INSTDIR"
  Delete "$DESKTOP\Argus Cluster.lnk"
  Delete "$SMPROGRAMS\Argus Cluster.lnk"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ArgusCluster"
SectionEnd

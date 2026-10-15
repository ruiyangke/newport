; Native Newport installer. Generated payload macros contain only known app
; files. NEVER recursively remove $INSTDIR: it may also hold the user's vault.
Unicode true
ManifestDPIAware true
RequestExecutionLevel user
SetCompressor /SOLID lzma
!include MUI2.nsh
!include FileFunc.nsh
!include LogicLib.nsh
!include WordFunc.nsh
!include x64.nsh

!define PRODUCT_KEY "Software\newport\Newport"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Newport"
Name "Newport"
OutFile "${OUTPUT}"
InstallDir "__NEWPORT_INSTALL_DIR__"
VIProductVersion "${VERSION_QUAD}"
VIAddVersionKey "ProductName" "Newport"
VIAddVersionKey "FileDescription" "Newport Setup"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "Newport contributors"
!define MUI_ICON "${ICON}"
!define MUI_UNICON "${ICON}"
!define MUI_ABORTWARNING
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipPassive
!insertmacro MUI_PAGE_WELCOME
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipPassive
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipPassive
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_FUNCTION RunNewport
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"
Var Passive
Var Update
Var Restart
Var NoShortcuts
Var Stage
Var Backup
Var Journal
Var RecoveryFailed
Var InstallerMutex

Function .onInit
  SetShellVarContext current
  SetRegView 64
  !if "${ARCH}" == "aarch64"
    ${IfNot} ${IsNativeARM64}
      MessageBox MB_OK|MB_ICONSTOP "This installer requires Windows on ARM64." /SD IDOK
      Abort
    ${EndIf}
  !else
    ${IfNot} ${IsNativeAMD64}
      MessageBox MB_OK|MB_ICONSTOP "This installer requires Windows on x64." /SD IDOK
      Abort
    ${EndIf}
  !endif
  System::Call 'kernel32::CreateMutexW(p 0, i 0, w "Local\NewportInstaller") p .r0 ?e'
  Pop $1
  StrCpy $InstallerMutex $0
  ${If} $0 = 0
  ${OrIf} $1 = 183
    MessageBox MB_OK|MB_ICONSTOP "Another Newport installer is running." /SD IDOK
    Abort
  ${EndIf}
  StrCpy $Passive 0
  StrCpy $Update 0
  StrCpy $Restart 0
  StrCpy $NoShortcuts 0
  ClearErrors
  ${GetOptions} $CMDLINE "/P" $0
  ${IfNot} ${Errors}
    StrCpy $Passive 1
  ${EndIf}
  ClearErrors
  ${GetOptions} $CMDLINE "/UPDATE" $0
  ${IfNot} ${Errors}
    StrCpy $Update 1
  ${EndIf}
  ClearErrors
  ${GetOptions} $CMDLINE "/R" $0
  ${IfNot} ${Errors}
    StrCpy $Restart 1
  ${EndIf}
  ClearErrors
  ${GetOptions} $CMDLINE "/NS" $0
  ${IfNot} ${Errors}
    StrCpy $NoShortcuts 1
  ${EndIf}
  ${If} $INSTDIR == "__NEWPORT_INSTALL_DIR__"
    ReadRegStr $INSTDIR HKCU "${PRODUCT_KEY}" ""
    ${If} $INSTDIR == ""
      StrCpy $INSTDIR "$LOCALAPPDATA\Newport"
    ${EndIf}
  ${EndIf}
  ReadRegStr $0 HKCU "${UNINSTALL_KEY}" "DisplayVersion"
  ${If} $0 != ""
    ${VersionCompare} $0 "${VERSION}" $1
    ${If} $1 = 1
      MessageBox MB_OK|MB_ICONSTOP "A newer Newport version is installed." /SD IDOK
      Abort
    ${EndIf}
  ${EndIf}
FunctionEnd

Function SkipPassive
  ${If} $Passive = 1
    Abort
  ${EndIf}
FunctionEnd

Function RunNewport
  SetOutPath "$INSTDIR"
  Exec '"$INSTDIR\newport.exe"'
FunctionEnd

Function FlushInstallJournal
  Push $0
  Push $1
  ClearErrors
  ; The profile-cache flush has no success return value. WriteINIStr checks
  ; each write; use the file handle below to check the durable disk flush.
  System::Call 'kernel32::WritePrivateProfileStringW(p 0, p 0, p 0, w "$Journal")'
  System::Call 'kernel32::CreateFileW(w "$Journal", i 0x40000000, i 3, p 0, i 3, i 0x80, p 0) p .r0'
  ${If} $0 = -1
    SetErrors
    Goto flush_done
  ${EndIf}
  System::Call 'kernel32::FlushFileBuffers(p r0) i .r1'
  System::Call 'kernel32::CloseHandle(p r0)'
  ${If} $1 = 0
    SetErrors
  ${EndIf}
  flush_done:
  Pop $1
  Pop $0
FunctionEnd

!macro FlushJournal
  Call FlushInstallJournal
  IfErrors install_failed
!macroend

!macro BackupFile ID PATH
  ${GetParent} "$Backup\${PATH}" $0
  CreateDirectory "$0"
  ${If} ${FileExists} "$INSTDIR\${PATH}"
    WriteINIStr "$Journal" "files" "${ID}" "old"
    IfErrors install_failed
    !insertmacro FlushJournal
    ; The old updater exits immediately after launching NSIS. Give it time to
    ; release its executable; never force-kill a process with active sessions.
    StrCpy $1 0
    retry_backup_${ID}:
    ClearErrors
    Rename "$INSTDIR\${PATH}" "$Backup\${PATH}"
    ${If} ${Errors}
      IntOp $1 $1 + 1
      ${If} $Update = 1
      ${AndIf} $1 < 60
        Sleep 500
        Goto retry_backup_${ID}
      ${EndIf}
      Goto install_failed
    ${EndIf}
  ${Else}
    WriteINIStr "$Journal" "files" "${ID}" "new"
    IfErrors install_failed
    !insertmacro FlushJournal
  ${EndIf}
!macroend

!macro PromoteFile ID PATH
  ${GetParent} "$INSTDIR\${PATH}" $0
  CreateDirectory "$0"
  ClearErrors
  Rename "$Stage\${PATH}" "$INSTDIR\${PATH}"
  IfErrors install_failed
!macroend

!macro RestoreFile ID PATH
  ReadINIStr $0 "$Journal" "files" "${ID}"
  ${If} $0 == "old"
    ${If} ${FileExists} "$Backup\${PATH}"
      ClearErrors
      ${If} ${FileExists} "$INSTDIR\${PATH}"
        Delete "$INSTDIR\${PATH}"
      ${EndIf}
      Rename "$Backup\${PATH}" "$INSTDIR\${PATH}"
      ${If} ${Errors}
        StrCpy $RecoveryFailed 1
      ${EndIf}
    ${EndIf}
  ${ElseIf} $0 == "new"
    ${IfNot} ${FileExists} "$Stage\${PATH}"
    ${AndIf} ${FileExists} "$INSTDIR\${PATH}"
      ClearErrors
      Delete "$INSTDIR\${PATH}"
      ${If} ${Errors}
        StrCpy $RecoveryFailed 1
      ${EndIf}
    ${EndIf}
  ${EndIf}
!macroend

Function RecoverFiles
  StrCpy $RecoveryFailed 0
  !insertmacro RestorePayload
FunctionEnd

Section "Newport"
  StrCpy $Stage "$INSTDIR\.newport-install-stage"
  StrCpy $Backup "$INSTDIR\.newport-install-backup"
  StrCpy $Journal "$Stage\transaction.ini"
  ${If} ${FileExists} "$Journal"
    ReadINIStr $0 "$Journal" "transaction" "fingerprint"
    ${If} $0 != "${FINGERPRINT}"
      MessageBox MB_OK|MB_ICONSTOP "An interrupted installation needs recovery. Run the same installer that started it." /SD IDOK
      Abort
    ${EndIf}
    ReadINIStr $0 "$Journal" "transaction" "phase"
    ${If} $0 != "committed"
      Call RecoverFiles
      ${If} $RecoveryFailed != 0
        Goto recovery_failed
      ${EndIf}
    ${EndIf}
    ClearErrors
    RMDir /r "$Backup"
    IfErrors recovery_failed
    RMDir /r "$Stage"
  ${EndIf}
  ; Refuse unrecognized directories instead of deleting arbitrary contents.
  ${If} ${FileExists} "$Stage"
  ${OrIf} ${FileExists} "$Backup"
    MessageBox MB_OK|MB_ICONSTOP "Newport's installer workspace is occupied. The existing installation was preserved." /SD IDOK
    Abort
  ${EndIf}
  ; Optional registry reads in .onInit can leave the NSIS error flag set on a
  ; first install. Only errors from this transaction should trigger rollback.
  ClearErrors
  CreateDirectory "$Stage"
  IfErrors install_failed
  CreateDirectory "$Backup"
  IfErrors install_failed
  WriteINIStr "$Journal" "transaction" "fingerprint" "${FINGERPRINT}"
  IfErrors install_failed
  WriteINIStr "$Journal" "transaction" "phase" "pending"
  IfErrors install_failed
  !insertmacro FlushJournal
  SetOverwrite on
  ClearErrors
  !insertmacro ExtractPayload
  IfErrors install_failed
  ; Extraction leaves the process in $Stage. Release that directory before
  ; cleanup; Windows cannot remove a process's current working directory.
  SetOutPath "$INSTDIR"
  WriteUninstaller "$Stage\uninstall.exe"
  IfErrors install_failed
  !insertmacro BackupPayload
  !insertmacro PromotePayload
  ; This checks the executable and bundled grammars without opening or migrating
  ; a user profile. Full UI/profile upgrade acceptance is a separate release gate.
  ClearErrors
  ExecWait '"$INSTDIR\newport.exe" --check-grammars' $0
  IfErrors install_failed
  ${If} $0 != 0
    Goto install_failed
  ${EndIf}

  ClearErrors
  ExecWait '"$INSTDIR\newport.exe" --check-pdf' $0
  IfErrors install_failed
  ${If} $0 != 0
    Goto install_failed
  ${EndIf}

  WriteINIStr "$Journal" "transaction" "phase" "committed"
  IfErrors install_failed
  !insertmacro FlushJournal

  ClearErrors
  WriteRegStr HKCU "${PRODUCT_KEY}" "" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "Newport"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "newport"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "MainBinaryName" "newport.exe"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" '"$INSTDIR\newport.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" '"$INSTDIR"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  ${If} $NoShortcuts != 1
  ${AndIf} $Update != 1
    CreateShortcut "$SMPROGRAMS\Newport.lnk" "$INSTDIR\newport.exe"
    CreateShortcut "$DESKTOP\Newport.lnk" "$INSTDIR\newport.exe"
  ${EndIf}
  IfErrors registration_failed
  ; The Run entry belongs to the app. Preserve it during every upgrade.
  ClearErrors
  RMDir /r "$Backup"
  IfErrors cleanup_pending
  RMDir /r "$Stage"
  cleanup_pending:
  ${If} $Passive = 1
    SetAutoClose true
  ${EndIf}
  Goto done

  install_failed:
    SetOutPath "$INSTDIR"
    ; Record rollback before undoing a possibly committed exchange. If the
    ; journal cannot be written, retain both sides instead of guessing on retry.
    ClearErrors
    WriteINIStr "$Journal" "transaction" "phase" "rollback"
    IfErrors recovery_failed
    Call FlushInstallJournal
    IfErrors recovery_failed
    Call RecoverFiles
    ${If} $RecoveryFailed != 0
      Goto recovery_failed
    ${EndIf}
    ClearErrors
    RMDir /r "$Backup"
    IfErrors recovery_failed
    RMDir /r "$Stage"
    ${If} $Restart = 1
    ${AndIf} ${FileExists} "$INSTDIR\newport.exe"
      Call RunNewport
    ${EndIf}
    MessageBox MB_OK|MB_ICONSTOP "Newport could not be installed. The previous files were restored. Quit Newport and try again." /SD IDOK
    SetErrorLevel 1
    Abort
  registration_failed:
    MessageBox MB_OK|MB_ICONSTOP "Newport's files were installed, but Windows registration or shortcuts could not be updated. Run this installer again to finish." /SD IDOK
    SetErrorLevel 1
    Abort
  recovery_failed:
    MessageBox MB_OK|MB_ICONSTOP "Recovery could not finish. The backup was retained. Quit Newport and run this installer again." /SD IDOK
    SetErrorLevel 1
    Abort
  done:
SectionEnd

Function .onInstSuccess
  ${If} $Restart = 1
    ${If} ${Silent}
    ${OrIf} $Passive = 1
      Call RunNewport
    ${EndIf}
  ${EndIf}
FunctionEnd

Function un.onInit
  SetShellVarContext current
  SetRegView 64
  System::Call 'kernel32::CreateMutexW(p 0, i 0, w "Local\NewportInstaller") p .r0 ?e'
  Pop $1
  StrCpy $InstallerMutex $0
  ${If} $0 = 0
  ${OrIf} $1 = 183
    MessageBox MB_OK|MB_ICONSTOP "Another Newport installer is running." /SD IDOK
    Abort
  ${EndIf}
FunctionEnd

Section "Uninstall"
  ${If} ${FileExists} "$INSTDIR\.newport-install-stage\transaction.ini"
    MessageBox MB_OK|MB_ICONSTOP "Finish recovery with the original installer before uninstalling." /SD IDOK
    Abort
  ${EndIf}
  ClearErrors
  Delete "$INSTDIR\newport.exe"
  ${If} ${Errors}
    MessageBox MB_OK|MB_ICONSTOP "Quit Newport before uninstalling." /SD IDOK
    Abort
  ${EndIf}
  !insertmacro RemovePayload
  ReadRegStr $0 HKCU "${UNINSTALL_KEY}" "InstallLocation"
  ${If} $0 == '"$INSTDIR"'
    DeleteRegKey HKCU "${UNINSTALL_KEY}"
    DeleteRegValue HKCU "${PRODUCT_KEY}" ""
    DeleteRegKey /ifempty HKCU "${PRODUCT_KEY}"
    Delete "$SMPROGRAMS\Newport.lnk"
    Delete "$DESKTOP\Newport.lnk"
  ${EndIf}
  ReadRegStr $0 HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Newport"
  ${If} $0 == '"$INSTDIR\newport.exe" --autostart'
  ${OrIf} $0 == '"$INSTDIR\newport.exe"'
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Newport"
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" "Newport"
  ${EndIf}
  ; Non-recursive: vaults, metrics and preferences survive even if co-located.
  RMDir "$INSTDIR"
SectionEnd

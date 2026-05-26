; Qobee — NSIS installer hooks.
;
; Tauri's NSIS template runs the macros defined here at install /
; uninstall time. We use them to:
;   1. Register the `qobee://` protocol handler under HKCU.
;   2. Register the AppUserModelID (AUMID) so the taskbar icon and
;      jump list keep a stable identity across updates.
;   3. Optionally register the audio file + folder context menu
;      verbs. These are off by default — the user opts in from the
;      "Settings -> Windows Integration" panel — so the installer
;      only writes the minimum required keys here.
;   4. Clean up everything on uninstall.
;
; All registry writes go to HKCU so the installer never needs admin
; privileges and never modifies machine-wide associations.

!define QOBEE_AUMID "app.qobee.player"
!define QOBEE_PROGID "Qobee.Music.AudioFile"

; --- Helper: write the protocol handler keys --------------------------------

!macro QobeeRegisterProtocol exe
    WriteRegStr HKCU "Software\Classes\qobee" "" "URL:Qobee Protocol"
    WriteRegStr HKCU "Software\Classes\qobee" "URL Protocol" ""
    WriteRegStr HKCU "Software\Classes\qobee\DefaultIcon" "" '"${exe}",0'
    WriteRegStr HKCU "Software\Classes\qobee\shell\open\command" "" '"${exe}" "%1"'
!macroend

!macro QobeeRegisterAumid
    ; Persist the AUMID so the runtime can read it back even before
    ; the first window is shown. The value isn't strictly required
    ; by Windows but lets the uninstaller blow it away cleanly.
    WriteRegStr HKCU "Software\Qobee\WindowsIntegration" "AUMID" "${QOBEE_AUMID}"
!macroend

!macro QobeeUnregisterAll
    DeleteRegKey HKCU "Software\Classes\qobee"
    DeleteRegKey HKCU "Software\Classes\${QOBEE_PROGID}"
    ; Folder context menu verbs (mirror the Rust side).
    DeleteRegKey HKCU "Software\Classes\Directory\shell\QobeeFolder.PlayFolder"
    DeleteRegKey HKCU "Software\Classes\Directory\shell\QobeeFolder.EnqueueFolder"
    DeleteRegKey HKCU "Software\Classes\Directory\shell\QobeeFolder.ScanFolder"
    DeleteRegKey HKCU "Software\Classes\Directory\shell\QobeeFolder.ImportFolder"
    DeleteRegKey HKCU "Software\Classes\Directory\Background\shell\QobeeFolder.PlayFolder"
    DeleteRegKey HKCU "Software\Classes\Directory\Background\shell\QobeeFolder.EnqueueFolder"
    DeleteRegKey HKCU "Software\Classes\Directory\Background\shell\QobeeFolder.ScanFolder"
    DeleteRegKey HKCU "Software\Classes\Directory\Background\shell\QobeeFolder.ImportFolder"
    DeleteRegKey HKCU "Software\Classes\Drive\shell\QobeeFolder.PlayFolder"
    DeleteRegKey HKCU "Software\Classes\Drive\shell\QobeeFolder.EnqueueFolder"
    DeleteRegKey HKCU "Software\Classes\Drive\shell\QobeeFolder.ScanFolder"
    DeleteRegKey HKCU "Software\Classes\Drive\shell\QobeeFolder.ImportFolder"
    ; Drop the OpenWithProgids hints. We don't enumerate every
    ; extension here because each one is its own subkey and the
    ; runtime cleanup function takes care of them; the install-time
    ; sweep below covers the most common ones.
    DeleteRegValue HKCU "Software\Classes\.mp3\OpenWithProgids" "${QOBEE_PROGID}"
    DeleteRegValue HKCU "Software\Classes\.flac\OpenWithProgids" "${QOBEE_PROGID}"
    DeleteRegValue HKCU "Software\Classes\.wav\OpenWithProgids" "${QOBEE_PROGID}"
    DeleteRegValue HKCU "Software\Classes\.ogg\OpenWithProgids" "${QOBEE_PROGID}"
    DeleteRegValue HKCU "Software\Classes\.m4a\OpenWithProgids" "${QOBEE_PROGID}"
    DeleteRegValue HKCU "Software\Classes\.aac\OpenWithProgids" "${QOBEE_PROGID}"
    DeleteRegValue HKCU "Software\Classes\.opus\OpenWithProgids" "${QOBEE_PROGID}"
    DeleteRegValue HKCU "Software\Classes\.alac\OpenWithProgids" "${QOBEE_PROGID}"
    DeleteRegValue HKCU "Software\Classes\.aiff\OpenWithProgids" "${QOBEE_PROGID}"
    DeleteRegValue HKCU "Software\Classes\.wv\OpenWithProgids" "${QOBEE_PROGID}"
    ; Autostart entry written by the runtime.
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Qobee"
    ; Our own subtree.
    DeleteRegKey HKCU "Software\Qobee\WindowsIntegration"
    DeleteRegKey /ifempty HKCU "Software\Qobee"
!macroend

; --- Tauri NSIS hooks --------------------------------------------------------
; The Tauri template invokes these by name if they exist. We always
; register the protocol handler + AUMID; the file/folder context
; menus are toggled at runtime from the Settings panel.

!macro NSIS_HOOK_POSTINSTALL
    !insertmacro QobeeRegisterProtocol "$INSTDIR\qobee-app.exe"
    !insertmacro QobeeRegisterAumid
!macroend

!macro NSIS_HOOK_PREUNINSTALL
    !insertmacro QobeeUnregisterAll
!macroend

; Crochets de l'installateur NSIS (SPEC 4.6), insérés par Tauri dans son modèle.
;
; Avant d'installer (ou de mettre à jour) et avant de désinstaller : fermer l'application,
; qui sinon relancerait le moteur, puis arrêter proprement le moteur par `auro engine stop`
; (point d'accès par défaut de l'utilisateur ; le moteur enregistre tout avant de rendre
; la main), et ne forcer `aurod` qu'en dernier recours, s'il tourne encore depuis ce
; dossier. Ses fichiers ne sont pas remplaçables tant qu'il tourne.

!macro NSIS_HOOK_PREINSTALL
  !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
  ${If} ${FileExists} "$INSTDIR\auro.exe"
    nsExec::Exec /TIMEOUT=20000 '"$INSTDIR\auro.exe" engine stop'
    Pop $0
  ${EndIf}
  !insertmacro CheckIfAppIsRunning "$INSTDIR\aurod.exe" "${PRODUCTNAME}"
!macroend

; Langue de l'installation, pour la désinstallation : le modèle ne l'enregistre que
; lorsqu'il affiche son choix de langue (désactivé ici), et sinon la désinstallation la
; redemande à chaque fois.
!macro NSIS_HOOK_POSTINSTALL
  WriteRegStr HKCU "${MANUPRODUCTKEY}" "Installer Language" $LANGUAGE
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
  ${If} ${FileExists} "$INSTDIR\auro.exe"
    nsExec::Exec /TIMEOUT=20000 '"$INSTDIR\auro.exe" engine stop'
    Pop $0
  ${EndIf}
  !insertmacro CheckIfAppIsRunning "$INSTDIR\aurod.exe" "${PRODUCTNAME}"
!macroend

; Case « Supprimer aussi les données » (décochée par défaut) : Tauri efface les
; préférences et le cache de la page (%APPDATA% et %LOCALAPPDATA%\<identifiant>) ; on y
; ajoute la base du moteur et son journal (paths::data_dir), fichier par fichier : ce
; dossier peut aussi être celui du programme, et rien d'autre n'y est touché. Les fichiers
; téléchargés ne le sont jamais.
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    SetShellVarContext current
    Delete "$LOCALAPPDATA\AuroDownloader\auro.db"
    Delete "$LOCALAPPDATA\AuroDownloader\auro.db-wal"
    Delete "$LOCALAPPDATA\AuroDownloader\auro.db-shm"
    Delete "$LOCALAPPDATA\AuroDownloader\auro.db-journal"
    Delete "$LOCALAPPDATA\AuroDownloader\auro.db.bak-v*"
    Delete "$LOCALAPPDATA\AuroDownloader\aurod.log"
    RMDir "$LOCALAPPDATA\AuroDownloader"
  ${EndIf}
!macroend

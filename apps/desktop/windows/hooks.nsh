; Crochets de l'installateur NSIS (SPEC 4.6), insérés par Tauri dans son modèle.
;
; Avant d'installer (ou de mettre à jour) et avant de désinstaller : fermer l'application,
; qui sinon relancerait le moteur, puis arrêter proprement le moteur par `auro engine stop`
; (point d'accès par défaut de l'utilisateur ; le moteur enregistre tout avant de rendre
; la main), et ne forcer `aurod` qu'en dernier recours, s'il tourne encore depuis ce
; dossier. Ses fichiers ne sont pas remplaçables tant qu'il tourne. Le relais de
; l'extension (`auro-relay`), lancé par un navigateur, est fermé de même : le navigateur
; le relancera.

; L'installateur d'essai (« AuroDownloader Essai », bundle.ps1 -Essai) ne touche ni au
; moteur du point d'accès par défaut ni au dossier des données réelles : ce sont ceux
; d'une vraie installation sur la même machine.
!if "${PRODUCTNAME}" == "AuroDownloader"
  !define AURO_REAL
!endif

!macro AURO_STOP_ALL
  !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
  !insertmacro CheckIfAppIsRunning "$INSTDIR\auro-relay.exe" "${PRODUCTNAME}"
  !ifdef AURO_REAL
    ${If} ${FileExists} "$INSTDIR\auro.exe"
      nsExec::Exec /TIMEOUT=20000 '"$INSTDIR\auro.exe" engine stop'
      Pop $0
    ${EndIf}
  !endif
  !insertmacro CheckIfAppIsRunning "$INSTDIR\aurod.exe" "${PRODUCTNAME}"
!macroend

; Relais de l'extension, déclaré aux navigateurs (SPEC 4.5) : une clé par navigateur, dont
; la valeur par défaut est le chemin du manifeste de l'hôte.
!define AURO_RELAY "org.aurodownloader.relay"
!define AURO_CHROME "Software\Google\Chrome\NativeMessagingHosts\${AURO_RELAY}"
!define AURO_EDGE "Software\Microsoft\Edge\NativeMessagingHosts\${AURO_RELAY}"
!define AURO_FIREFOX "Software\Mozilla\NativeMessagingHosts\${AURO_RELAY}"

; Retire une déclaration, seulement si elle désigne le manifeste de ce dossier : une autre
; installation garde la sienne.
!macro AURO_UNREGISTER KEY MANIFEST
  ReadRegStr $0 HKCU "${KEY}" ""
  ${If} $0 == "$INSTDIR\${MANIFEST}"
    DeleteRegKey HKCU "${KEY}"
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro AURO_STOP_ALL
!macroend

; Langue de l'installation, pour la désinstallation : le modèle ne l'enregistre que
; lorsqu'il affiche son choix de langue (désactivé ici), et sinon la désinstallation la
; redemande à chaque fois.
!macro NSIS_HOOK_POSTINSTALL
  WriteRegStr HKCU "${MANUPRODUCTKEY}" "Installer Language" $LANGUAGE
  WriteRegStr HKCU "${AURO_CHROME}" "" "$INSTDIR\relay-chromium.json"
  WriteRegStr HKCU "${AURO_EDGE}" "" "$INSTDIR\relay-chromium.json"
  WriteRegStr HKCU "${AURO_FIREFOX}" "" "$INSTDIR\relay-firefox.json"
!macroend

; Association aux liens magnet et aux .torrent (option de l'application, étape 5b) : retirée
; par l'application elle-même, qui ne touche qu'à ce qui désigne son propre exécutable et
; rétablit le client d'avant. Gardée lors d'une mise à jour.
!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro AURO_STOP_ALL
  ${If} $UpdateMode <> 1
  ${AndIf} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
    nsExec::Exec /TIMEOUT=10000 '"$INSTDIR\${MAINBINARYNAME}.exe" --unregister-links'
    Pop $0
  ${EndIf}
  ; « Supprimer aussi les données » : les secrets de la base (cookies, mots de passe des
  ; sites et du proxy) sont au coffre du système, sous l'espace de noms de cette base, que
  ; seul le moteur connaît. Effacés par `aurod` tant qu'il est encore là (audit C4).
  !ifdef AURO_REAL
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
  ${AndIf} ${FileExists} "$INSTDIR\aurod.exe"
    nsExec::Exec /TIMEOUT=20000 '"$INSTDIR\aurod.exe" --erase-secrets'
    Pop $0
  ${EndIf}
  !endif
!macroend

; Case « Supprimer aussi les données » (décochée par défaut) : Tauri efface les
; préférences et le cache de la page (%APPDATA% et %LOCALAPPDATA%\<identifiant>) ; on y
; ajoute les données du moteur (paths::data_dir), fichier par fichier et dossier par
; dossier nommés : la base, son verrou, ses copies et son journal ; les journaux (logs), les
; plugins installés, les outils de la vidéo (tools) ; le certificat de l'accès à distance et
; sa clé privée (audit C4). Ce dossier peut aussi être celui du programme : rien d'autre n'y
; est touché. Les fichiers téléchargés ne le sont jamais.
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    !insertmacro AURO_UNREGISTER "${AURO_CHROME}" "relay-chromium.json"
    !insertmacro AURO_UNREGISTER "${AURO_EDGE}" "relay-chromium.json"
    !insertmacro AURO_UNREGISTER "${AURO_FIREFOX}" "relay-firefox.json"
  ${EndIf}
  !ifdef AURO_REAL
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    SetShellVarContext current
    Delete "$LOCALAPPDATA\AuroDownloader\auro.db"
    Delete "$LOCALAPPDATA\AuroDownloader\auro.db-wal"
    Delete "$LOCALAPPDATA\AuroDownloader\auro.db-shm"
    Delete "$LOCALAPPDATA\AuroDownloader\auro.db-journal"
    Delete "$LOCALAPPDATA\AuroDownloader\auro.db.bak-v*"
    Delete "$LOCALAPPDATA\AuroDownloader\auro.db.lock"
    Delete "$LOCALAPPDATA\AuroDownloader\aurod.log"
    Delete "$LOCALAPPDATA\AuroDownloader\remote-cert.pem"
    Delete "$LOCALAPPDATA\AuroDownloader\remote-key.pem"
    Delete "$LOCALAPPDATA\AuroDownloader\torrent-dht.json"
    RMDir /r "$LOCALAPPDATA\AuroDownloader\logs"
    RMDir /r "$LOCALAPPDATA\AuroDownloader\plugins"
    RMDir /r "$LOCALAPPDATA\AuroDownloader\tools"
    RMDir "$LOCALAPPDATA\AuroDownloader"
  ${EndIf}
  !endif
!macroend

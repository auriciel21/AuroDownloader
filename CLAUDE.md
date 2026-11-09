# AuroDownloader : gestionnaire de téléchargement libre

Gestionnaire de téléchargement gratuit et open source pour Windows, Linux et Android.
Un moteur unique, processus autonome, piloté par une API JSON-RPC 2.0. Les interfaces
(bureau, Android, web, CLI, extension navigateur) ne contiennent aucune logique de
téléchargement.

## Référence

- `docs/SPEC.md` : cahier des charges, modèle de données (section 5), protocole du moteur
  (section 6), algorithmes (section 7), tests et feuille de route (section 9). À lire avant
  toute modification du moteur.
- `docs/ESSAI-EXTENSION.md` : procédure de l'essai de l'extension dans les navigateurs, faite
  par l'utilisateur le 2026-11-08 (porte de la phase 3, franchie).
- `docs/MESURES.md` : relevés datés du banc de performance face à aria2. Ajouter une
  section datée à chaque nouvelle campagne, sans réécrire les anciennes.
- Les deux schémas du document d'origine (architecture, feuille de route) ne sont pas dans
  l'export Markdown, qui les remplace par des marqueurs « embedded content ». L'architecture
  est décrite en texte aux sections 4.2 à 4.6 et 6.2 ; la feuille de route a été recopiée en
  tableau à la section 9.3.
- Si le code et la spécification divergent, ne pas trancher en silence : le signaler, puis
  mettre à jour l'un ou l'autre.

## Structure

- `crates/engine` (`auro-engine`) : le moteur. Phase 1 : `store` (SQLite et migrations,
  fait), `segmenter` (couverture et redécoupage faits ; sources et nombre de connexions à
  écrire), `io` (`.part`, écriture positionnée, points de contrôle, renommage final,
  espace libre et place occupée, réservation de la place : F-10), `net` (préparation, requêtes de segments avec
  `If-Range`, noms de fichier, classement des erreurs, autorités de certification
  ajoutées par `--ca-cert` ; FTP, proxies à écrire),
  `store::tasks` (tâches, sources, états de la SPEC 5.5, fin de vie), `transfer`
  (exécution d'une tâche : relie `net`, `io` et `store`, redécoupage dynamique ;
  nombre de connexions automatique). Phase 2 : `scheduler` (objet `Engine` : une boucle
  unique reçoit les commandes par canal, démarre les tâches selon les files et
  priorités, nouvelles tentatives, reprise au démarrage, événements avec révision `rev`),
  `rpc` (JSON-RPC un message par ligne : `methods`, `session` avec abonnements et
  avancement regroupé, `local` pour le canal nommé Windows réservé au SID de
  l'utilisateur ou le socket Unix), `paths` (base et point d'accès par défaut), `launch`
  (démarrage détaché d'`aurod` par ses clients, CLI et bureau), `limit`
  (seaux de jetons des limites de débit ; le planificateur tient le global, ceux des
  files et ceux des tâches en cours), `store::settings` (réglages, dont la limite
  globale), `schedule` (plages horaires et action de fin, F-16 : règles, ouverture à
  une heure locale lue par SQLite), `store::schedules` (planifications en base),
  `scheduler::planning` (files fermées hors plage, action de fin annoncée puis
  exécutée), `power` (veille et extinction ; branchées par `aurod` seulement). Phase 3 :
  `secrets` (coffre du système des cookies et jetons : Gestionnaire d'identification,
  Secret Service, sinon mémoire ; un espace de noms par base), `store::headers`
  (en-têtes des tâches, secrets par référence). Hors phase (IDM, 2026-11-08) : `moves`
  (« Déplacer/Renommer », SPEC 7.12 : renommage, copie vers un autre volume, reprise au
  démarrage) et `store::moves` (emplacement d'une tâche, journal `file_moves`) ;
  « Actualiser l'adresse » (SPEC 7.13) : `Engine::set_url`, `store::tasks::replace_url`.
  Identifiants de sites (F-06, SPEC 7.14) : `net::auth` (Basic, Digest, portée d'un site,
  réponse au `401`), `store::logins` (table `site_logins`, mot de passe au coffre).
  Empreinte du fichier (F-11, SPEC 7.9, 2026-11-08) : `io::checksum` (SHA-256, SHA-1,
  MD5 ; lecture du `.part` fermé), état `verifying` durable (`transfer::finish_part`,
  reprise par `resume_verifying`).
  Empreinte lue dans un fichier voisin (F-11, 2026-11-08) : `net::sums` (`.sha256`,
  `SHA256SUMS`…, cherchés à la préparation, sans en-têtes de la tâche), colonne
  `tasks.checksum_source` (migration 5), réglage `downloads.neighborChecksum` ; les
  réglages oui ou non passent par `settings::Flag` (commandes `Engine::flag`, `set_flag`).
  Historique (F-35, 2026-11-08) : `store::history` (recherche, pages par curseur,
  effacement), méthodes `history.list` et `history.clear`.
  Réservation de la place (F-10, 2026-11-08) : `io::preallocate` (`fallocate`, NTFS et
  ReFS), réglage `downloads.preallocate`, vrai par défaut.
- `crates/daemon` (`auro-daemon`, binaire `aurod`) : le moteur en processus autonome, qui
  sert le protocole sur le point d'accès local.
- `crates/engine/migrations/` : migrations SQL numérotées, intégrées au binaire.
- `crates/cli` (`auro-cli`, binaires `auro` et `auro-relay`) : ligne de commande, cliente
  d'`aurod` (bibliothèque `auro_cli` : `client`, connexion, appels, démarrage d'`aurod`
  par `launch` ; `relay`, hôte de messagerie native de l'extension, SPEC 4.5 ; binaire
  `auro` : `remote`, commandes, et `display`). Le mode `direct` exécute une tâche avec la bibliothèque, sans moteur : les
  deux bancs l'utilisent, pour mesurer le transfert seul.
- Essais manuels de la CLI : toujours sur un point d'accès et une base d'essai
  (`--endpoint`, `engine start --db`), jamais sur le moteur réel de l'utilisateur ; finir
  par `engine stop` et vérifier qu'aucun `aurod` d'essai ne reste. Sous Windows,
  `--endpoint` est un chemin complet (`\\.\pipe\auro-essai-…`) ; `--no-start` sur toutes
  les commandes d'un script d'essai ; un script qui se trompe s'arrête avant d'en lancer un
  autre, et un script arrêté se vérifie (`Win32_Process`) : ses processus survivent à
  l'arrêt de la tâche. Incident du 2026-11-08 : un `auro list` d'essai au point d'accès
  invalide a démarré `aurod` sur la base réelle, migrée en version 3 (contenu intact,
  copie `auro.db.bak-v2`). Depuis, `aurod` refuse un point d'accès autre que celui par
  défaut sans `--db`, et la CLI ne démarre d'elle-même que le moteur par défaut.
- `apps/desktop` (`auro-desktop`, binaire `aurodownloader`) : application de bureau Tauri 2
  (SPEC 4.3), espace de travail Cargo à part (Tauri demande WebKitGTK 4.1, absent de
  l'Ubuntu 20.04 de WSL). `bridge` : liaison avec le moteur (démarrage d'`aurod`, session,
  abonnement, reconnexion, avancement suspendu fenêtre cachée), sans dépendre de Tauri ;
  `prefs` : préférences de l'interface (JSON, aucun secret) ; `files` : ouverture d'un
  fichier terminé, jamais d'un exécutable (SPEC 8.1). Paquets : `bundle.ps1`,
  `bundle.sh`, `tauri.bundle.json` (ajoute `aurod` et `auro`), `windows/hooks.nsh`
  (arrêt du moteur, effacement des données sur demande), `icons/icon.svg` (source des
  icônes).
- `ui` : la page web de l'application, destinée aussi à l'interface distante (F-39).
  `src/model.js` (instantané, notifications et révisions, SPEC 6.10 ; vues de
  l'arborescence, types de fichier, tri des colonnes), `src/i18n.js` (français source,
  anglais), `src/host.js` (services de l'hôte), `src/main.js` (fenêtre principale :
  barre d'outils, arborescence, tableau à sélection multiple, menu contextuel,
  propriétés, options) ; tests `node --test` dans `tests/`.
- Essais manuels de l'application : `--endpoint`, `--db` et `--prefs` d'essai (elle lance
  `aurod` avec les deux premiers), jamais le moteur ni les préférences réels ; ne pas
  toucher au démarrage avec la session (clé `Run` de l'utilisateur) ; finir par
  `engine stop` sur ce point d'accès.
- L'utilisateur se sert de l'application installée (depuis le 2026-11-07) : instance
  unique, donc un essai de l'application ne démarre pas tant que la sienne est ouverte
  (il lui passe la main et se ferme). Ne jamais fermer ni piloter la sienne, ni toucher à
  son moteur (`\\.\pipe\aurodownloader-<utilisateur>`) ; lui demander de la quitter s'il
  faut un essai de l'interface.
- Essais d'un installateur Windows : toujours l'installateur d'essai (`bundle.ps1 -Essai`,
  « AuroDownloader Essai ») : l'installateur normal partage les clés de registre
  (désinstallation, dossier mémorisé) et les raccourcis d'une vraie installation, et sa
  désinstallation d'essai les efface (incident du 2026-11-07, réparé à la main). Relever
  avant, et rétablir après, les clés `NativeMessagingHosts\org.aurodownloader.relay`,
  communes aux deux. `/S /D=<dossier d'essai>` (jamais le dossier par
  défaut, qui est aussi celui des données réelles), lancé avec `USERNAME` changé dans
  son environnement (ses crochets arrêtent le moteur du point d'accès par défaut, dérivé
  de `USERNAME` : sinon ils arrêteraient le moteur réel de l'utilisateur), application
  installée lancée avec `--endpoint`, `--db` et `--prefs` d'essai ; finir par la
  désinstallation (`/S`) et vérifier que la clé `Run` de l'utilisateur n'a pas changé, et
  que les clés `NativeMessagingHosts` du relais sont revenues à leur état d'avant.
- Essais d'une action de fin (veille, extinction) : toujours avec un `aurod` d'essai lancé
  à la main avec `--simulate-power` avant l'application (qui s'y connecte au lieu d'en
  lancer un), et vérifier sa ligne de commande ; jamais avec un `aurod` qui pourrait
  éteindre la machine de l'utilisateur.
- `extension` : extension navigateur (SPEC 4.5, F-37), Manifest V3 pour Firefox, Chrome et
  Edge ; `decide.js` (décision de capture, testée par `node --test extension/tests/`),
  `background.js`, `content.js` (clic avec Alt), `options.*`, `_locales` (en, fr).
  `AURO_ENDPOINT` remplace le point d'accès par défaut (`paths::endpoint`) : un navigateur
  d'essai lancé avec lui fait parler le relais à un moteur d'essai.
- Essais de l'extension : jamais dans les profils de navigateur de l'utilisateur. Un profil
  d'essai Edge se connecte de lui-même à son compte Microsoft et synchronise : ne pas
  l'utiliser. Chrome de marque ignore `--load-extension` ; Chrome for Testing a été refusé
  par la session (exécutable téléchargé). Essai automatisé : simuler le navigateur
  (`tools/fake_browser.py` du bloc-notes : clé de registre, manifeste, relais) ; l'essai
  dans un vrai navigateur revient à l'utilisateur. IDM, installé, intercepte certains
  téléchargements avant l'extension et ouvre ses propres fenêtres.
- `crates/android` (`auro-android`, `libauro_android.so`) : le moteur dans le processus de
  l'application Android (SPEC 4.4) : `Host` (runtime, moteur, connexion JSON-RPC dans le
  processus, testé partout) et `jni_glue` (Android seulement : `NativeEngine.start`, `send`,
  `stop`, rappel `onMessage` ; initialise `rustls-platform-verifier`, puis
  `net::platform_tls_ready`).
- `apps/android` : application Android (Gradle 8.14.3, AGP 8.10.1, Kotlin ; projet à part,
  hors de l'espace de travail Cargo). `app/build.gradle.kts` compile la bibliothèque par
  Cargo et le NDK (tâche `cargoBuild`) et prend la partie Kotlin de
  rustls-platform-verifier (groupe `org.rustls`) à la version de `Cargo.lock`.
  `local.properties` (chemin du SDK, non versionné). `AuroApp` (moteur démarré une fois
  par processus, dossier par défaut `Download/AuroDownloader`, indexation par
  `MediaScannerConnection`, actions) ; `engine/` : `NativeEngine`, `EngineClient` (client
  JSON-RPC Kotlin), `Downloads` (état des tâches pour Compose, mêmes règles de révision que
  `ui/src/model.js`) ; `ui/` : `Screens` (liste, détails, réglages, ajout ; Material 3),
  `Format` ; textes dans `res/values` (français) et `values-en` ; icône adaptative
  vectorielle (`mipmap-anydpi-v26/ic_launcher.xml`, `drawable/ic_launcher_*`), recopiée de
  `apps/desktop/icons/icon.svg`. `MainActivity` : Compose,
  `--es url <adresse>` (essais), ouverture par `FileProvider`. `ShareActivity` : liens
  partagés ou ouverts (activité sans fenêtre, toujours neuve). `Background` (SPEC 7.15) :
  dit au moteur si le réseau est utilisable (`engine.setNetwork`), demande la protection
  d'Android (`Transfers.kt` : `TransferJob`, transfert lancé par l'utilisateur, Android 14
  et plus ; `TransferService`, Android 8 à 13), notifications, « Wi-Fi seulement »,
  redémarrage (`BootReceiver`). Le moteur Android démarre sans réseau utilisable.
- Essais Android : tablette Teclast T70 (Android 15, `adb -s <série de la tablette>`) et
  téléphone Galaxy S25 Ultra (Android 17, `adb -s <série du téléphone>` ; antivirus AVG installé, qui signale l'application à l'installation : ne pas toucher à sa fenêtre) ; messages du moteur par
  `adb logcat -s AuroDownloader:I`. Sous Git Bash, `MSYS_NO_PATHCONV=1` pour les chemins
  passés à `adb shell`.
  Liens d'essai : `am start -a android.intent.action.VIEW -d <adresse> -t
  application/octet-stream`, une adresse différente à chaque fois (paramètre `?essai=…`).
  `cmd netpolicy set metered-network <SSID> true` simule « Wi-Fi seulement » : relever
  avant (`cmd netpolicy list wifi-networks`) et remettre ensuite (`undefined`). adb ne peut
  pas envoyer `BOOT_COMPLETED` sur la tablette : vrai redémarrage par `adb reboot`, seulement
  avec l'accord de l'utilisateur ; l'annonce n'arrive qu'après son déverrouillage (code de
  l'utilisateur, jamais saisi par adb). « Tout mettre en pause » : `cmd statusbar
  expand-notifications`, déplier le groupe puis la notification (`uiautomator dump`). Le
  volet montre les notifications privées de l'utilisateur : ne rien en citer. Finir par `pm clear org.aurodownloader` et
  supprimer `Download/AuroDownloader`.
- `experiments/android-saf` : essai de faisabilité Android (application Java minimale,
  construite sans Gradle par `build.sh --install`) ; ne fait pas partie du produit.
- `experiments/desktop` : prototypes Qt Quick (`qt`, CMake et Qt 6 MSVC dans `C:\Qt`) et
  Tauri (`tauri`, hors de l'espace de travail Cargo) de l'application de bureau, au
  périmètre identique, et leurs mesures (`README.md`) ; ne font pas partie du produit.
- `experiments/torrent` : prototype torrent (phase 5, étape 2) : `rqbit` (librqbit, hors de
  l'espace de travail Cargo, compilé sous WSL Ubuntu), `libtorrent/lt_proto.py` (liaisons
  Python dans un environnement de la distribution Ubuntu-24.04), scripts `run.sh` et
  `magnet.sh` (`PROTO_DIR`), mesures dans `README.md`. Essais sur l'essaim Debian
  seulement, envoi limité, arrêt sans partager ; mesurer sous WSL (sous Windows, un port
  d'écoute fait apparaître la fenêtre du pare-feu).

## Commandes

```
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
# Licences des dépendances (deny.toml ; cargo install cargo-deny --locked) :
cargo deny check licenses
cargo deny --manifest-path apps/desktop/Cargo.toml --config deny.toml check licenses
cargo run -p auro-cli -- db migrate auro.db
cargo run -p auro-cli -- probe https://deb.debian.org/debian/README
# Client du moteur ; `aurod` démarre de lui-même s'il ne tourne pas (base et point
# d'accès par défaut de l'utilisateur). Pour un essai isolé : --endpoint et
# `engine start --db`, puis `engine stop`.
cargo run -p auro-cli -- get https://deb.debian.org/debian/README -o .
cargo run -p auro-cli -- add <url>… --paused ; list ; resume <id>… ; watch ; remove <id>… ; engine stop
cargo run -p auro-cli -- restart <id>… --delete-files   # retélécharger depuis le début
cargo run -p auro-cli -- move <id> -o <dossier> -n <nom>   # déplacer, renommer
cargo run -p auro-cli -- refresh <id> <nouvelle-url>       # lien expiré
cargo run -p auro-cli -- login set <site> -u <nom> ; login list ; login remove <site>
cargo run -p auro-cli -- add <url> --checksum sha256:<hex>   # ou la valeur seule
cargo run -p auro-cli -- checksum <id> <empreinte> ; checksum <id> --clear
cargo run -p auro-cli -- history [texte] [--csv] ; history --clear
cargo run -p auro-cli -- schedule set --days 1-5 --from 23:00 --to 07:00 --then sleep ; schedule list
cargo run -p auro-cli -- dir <dossier> ; dir --reset   # dossier des ajouts sans dossier (extension)
cargo run -p auro-cli -- add <url> -H "Referer: …" -H "Cookie: …"   # Cookie : coffre du système
# Sans moteur (bancs, dépannage) : la bibliothèque sur une base donnée.
cargo run -p auro-cli -- direct --db essai.db get https://deb.debian.org/debian/README -o .
cargo run -p auro-daemon -- --db essai.db   # moteur ; Ctrl-C l'arrête proprement
# Banc de reprise sous arrêt brutal, version longue (la courte tourne avec cargo test) :
AURO_CRASH_CYCLES=1000 AURO_CRASH_SIZE=1073741824 AURO_CRASH_SECONDS=8 \
  cargo test --release -p auro-cli --test crash_resume -- --nocapture
# Banc de reprise du moteur autonome (aurod tué pendant que plusieurs tâches tournent ;
# la version courte tourne avec cargo test) :
AURO_CRASH_CYCLES=150 cargo test --release -p auro-daemon --test crash_daemon -- --nocapture
# Disque vraiment plein (Linux, WSL) : tmpfs de 8 Mio monté sans root, puis le test ignoré :
D=$(mktemp -d); unshare -rm sh -c "mount -t tmpfs -o size=8m tmpfs $D && \
  AURO_FULL_DISK_DIR=$D cargo test -p auro-engine --lib -- --ignored really_full"
# Banc de performance face à aria2, HTTP et HTTPS (ignoré par défaut ; aria2c installé
# par winget ; sous Linux, travail `perf` de la CI, lancé à la main ou chaque lundi) :
AURO_ARIA2=$LOCALAPPDATA/Microsoft/WinGet/Links/aria2c.exe AURO_PERF_SIZE=536870912 \
  AURO_PERF_RUNS=5 cargo test --release -p auro-cli --test perf_vs_aria2 -- --ignored --nocapture
# Application de bureau (dans apps/desktop ; aurod cherché à côté, sinon AURO_DAEMON) :
cargo fmt ; cargo clippy --all-targets -- -D warnings ; cargo test
AURO_DAEMON=../../target/release/aurod cargo run --release -- --endpoint <essai> \
  --db <essai.db> --prefs <essai.json>
# Page : tests sans navigateur (Node 18 et plus ; installé dans la distribution WSL
# d'essai Ubuntu-24.04) :
node --test ui/tests/
node --test extension/tests/
# Installateurs (SPEC 4.6 ; tauri-cli : cargo install tauri-cli --version "^2" --locked) :
apps/desktop/bundle.ps1   # Windows, NSIS : apps/desktop/target/release/bundle/nsis/
apps/desktop/bundle.sh    # Linux, .deb et AppImage (distribution WSL Ubuntu-24.04)
# Application Android (dans apps/android ; compile aussi le moteur pour arm64) :
cd apps/android && ./gradlew assembleDebug   # app/build/outputs/apk/debug/app-debug.apk
adb install -r app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n org.aurodownloader/.MainActivity --es url <adresse>
```

## Règles du projet

- La fiabilité passe avant les fonctionnalités : aucune modification de la reprise ou de
  l'intégrité sans test qui la couvre.
- Une migration publiée ne se modifie jamais : ajouter `000N_nom.sql`, l'enregistrer dans
  `MIGRATIONS` (`store.rs`) et mettre à jour la section 5 de la spécification.
- Protocole : ajouter un champ optionnel ou une méthode ne casse rien ; renommer ou supprimer
  exige une nouvelle version majeure (SPEC 6.1). Mettre à jour la section 6.
- Ordre d'écriture : octets écrits et vidés sur disque (`fdatasync`), puis seulement le
  compteur en base (SPEC 5.6, règle 1).
- Aucune télémétrie. Aucun secret en base ni dans les journaux (SPEC 5.1 et 8.2).
- Licence GPL-3.0 ou ultérieure (N-11). Toute dépendance ajoutée ou mise à jour passe
  `cargo deny check licenses` (et sa variante du bureau) ; une licence nouvelle ne s'ajoute
  à `deny.toml` qu'après vérification de sa compatibilité, notée dans la SPEC 8.3. Les
  dépendances Gradle d'Android se relèvent à la main (même section).
- Secrets des tâches (`Cookie`, `Authorization`, `Proxy-Authorization`) : seulement par
  `secrets::Vault`, la base n'en garde que la référence ; le secret est écrit avant sa
  référence, la référence effacée avant le secret. Tests : `Vault::memory()` (défaut
  d'`Options`) ; le coffre réel seulement dans les tests de `secrets`, sur un espace de
  noms d'essai vidé à la fin. Secret Service sous Linux : `AURO_SECRET_SERVICE=1`, dans
  une session D-Bus avec un trousseau d'essai (`gnome-keyring-daemon --unlock`,
  `XDG_DATA_HOME` à part). Mots de passe des identifiants de sites : même règle
  (`site_logins.secret_ref`). Toute nouvelle table qui référence un secret s'ajoute à
  `store::headers::all_secret_refs`, sinon le ramassage du démarrage efface ses secrets.
  Un mot de passe ne ressort jamais par le protocole ni dans un journal ; Basic ne part
  jamais en clair (HTTP).
- Aléa : aws-lc est compilé sans entropie de gigue (`AWS_LC_SYS_NO_JITTER_ENTROPY`,
  `.cargo/config.toml`, SPEC 8.1) ; il s'amorce sur l'aléa du système.
- Invariant de couverture (SPEC 5.6, règle 2) : calcul dans `segmenter`, application en
  base dans `store::segments` (vérifié dans la transaction de chaque écriture, contrôle au
  démarrage par `find_coverage_errors`). Ne jamais écrire dans `segments` sans passer par
  ce module.
- Ordre d'écriture, application : `written_bytes` ne s'enregistre que par
  `store::segments::save_progress`, qui n'accepte que des `io::SyncedProgress`, fabriqués
  par `io` après `sync_data`. Ne pas ajouter d'autre chemin d'écriture de ce compteur.
- Noms de fichier venus du serveur : toujours par `net::filename::sanitize` (SPEC 8.1).
  Les messages d'erreur réseau ne citent jamais l'URL ni la valeur d'un en-tête (SPEC 8.2).
- États d'une tâche : toujours par `store::tasks::change_state` ou `complete_task`, qui
  vérifient la table de la SPEC 5.5, ou `restart_task` (retéléchargement, hors de la
  table, SPEC 5.5). Ne jamais écrire `tasks.state` directement.
- Fichier d'une tâche déplacé (SPEC 7.12) : seulement par `Scheduler::relocate`, journal
  `store::moves` écrit avant de toucher au disque ; la tâche ne prend son nouvel
  emplacement que quand le fichier y est, et l'original d'une copie n'est effacé
  qu'ensuite. Un nom choisi par l'utilisateur est vérifié tel quel (`sanitize` doit le
  rendre inchangé), jamais corrigé en silence.
- Empreinte (SPEC 7.9) : le `.part` est vidé et fermé (`PartFile::seal`) avant le passage
  à `verifying`, et ne prend son nom (`SealedPart::finish`) qu'après une empreinte juste ;
  une empreinte différente le laisse sous son nom provisoire, tâche `failed`
  `CHECKSUM_MISMATCH`.
- Toute modification de `transfer`, `io` ou `store` passe aussi le banc de reprise
  (`crates/cli/tests/crash_resume.rs`) ; un échec s'y rejoue avec `AURO_CRASH_SEED`.
  Toute modification de `scheduler`, `rpc` ou du daemon passe aussi celui d'`aurod`
  (`crates/daemon/tests/crash_daemon.rs`). Leurs contrôles communs sont dans
  `testing::Checker`.
- Vidages du `.part` : toujours par le `SyncGate` du fichier (`PartFile::checkpoint`,
  `SegmentWriter::finish`), jamais par un `sync_data` direct pendant que des écrivains
  tournent : des vidages simultanés du même fichier coûtent très cher sous Linux.
- Redécoupage (SPEC 7.2) : la borne d'un segment en cours ne recule que par
  `ProgressHandle::shrink_end`, avant l'enregistrement de la coupe (`save_split`). Jamais
  l'inverse : la base ne doit pas promettre à un nouveau segment des octets que l'ancien
  écrivain peut encore accepter.
- Mesures de performance : seulement en `--release`, sur des téléchargements longs
  (512 Mio et plus) et répétés (médiane de 5) ; sous 4 s, le bruit atteint 10 %.
- Temps processeur sous Windows : par cycles (`QueryProcessCycleTime`), jamais par
  `GetProcessTimes`, qui échantillonne et compte mal les processus d'entrées-sorties.
- Banc de performance équitable (SPEC 9.1) : chaque mesure va jusqu'au fichier sur
  disque (le banc le vide après chaque outil) ; médiane de chaque grandeur, outils
  alternés. Ne rien faire tourner d'autre pendant une campagne.
- WSL : `wsl -d Ubuntu -- bash -lc '…'` développe les `$` trop tôt ; passer par un script
  (`wsl -d Ubuntu -- bash /mnt/c/…/script.sh`). Clone de travail : `~/AuroDownloader`
  (système de fichiers Linux, jamais `/mnt/c` pour mesurer).
- N-01 (SPEC 3), décision du 2026-11-06 :
  - la porte est le rapport par octet auro / aria2 du temps processeur du processus plus
    celui du vidage sur disque laissé au banc, à nombre de connexions égal (le vidage
    compte pour tous : auro paie le sien, règle 1) ; le processus seul n'est
    qu'indicatif (parité 0,93 à 1,07 à 16 connexions, dans le bruit) ;
  - le temps de toute la machine n'est qu'indicatif : ±30 % d'un passage à l'autre sous
    WSL ;
  - les 15 % d'un cœur ne sont qu'un objectif indicatif, sur la machine de référence
    nommée dans le relevé ;
  - mesurer en HTTP et en HTTPS ;
  - une validation sur Linux natif ou en intégration continue dédiée reste à mettre en
    place.
- Phase 2, plan validé (2026-10-16) : 1. planificateur (`scheduler`, fait) ; 2. protocole
  JSON-RPC (`rpc`) et binaire `aurod` (faits ; méthodes servies : SPEC 6.5) ; 3. CLI
  cliente du moteur (fait : commandes, démarrage d'`aurod`, mode `direct`, banc de reprise
  sur `aurod`) ; 4. limites de débit (F-15) et espace disque (F-18) (faits : SPEC 7.6
  et 7.6 bis, commande `auro limit`) ; 5. prototypes Qt Quick et Tauri comparés (faits,
  `experiments/desktop/README.md`), Tauri choisi le 2026-11-06, application de bureau
  (`apps/desktop`, `ui`, fenêtre à la manière d'IDM ; essai NVDA fait par
  l'utilisateur le 2026-11-08 sur l'application installée, Historique et Empreinte
  compris : fonctionnel) ; 6. plages horaires (F-16, fait le 2026-11-07 : SPEC 7.11, `auro schedule`,
  boîte « Planification »). Porte : utilisation quotidienne
  sans perte de données, franchie le 2026-11-08 (déclarée par l'utilisateur après un
  usage quotidien de l'application installée depuis le 2026-11-07, sans perte).
- Phase 3, plan du 2026-11-07 : 1. portée déclarée à l'ouverture, sans jeton sur le canal
  local (fait, SPEC 6.9) ; 2. `idempotencyKey` (fait) et dossier des téléchargements dans
  le moteur (fait) ; 3. relais `auro-relay` et sa déclaration aux navigateurs (fait) ;
  4. extension (écrite, `extension/` ; essai réel fait par l'utilisateur le 2026-11-08) ;
  5. cookies et en-têtes de la page (fait : `headers` de `task.add`, coffre du système,
  SPEC 4.5, 5.1 et 6.5 ; repli en mémoire sous Linux sans Secret Service, à
  confirmer).
- Porte de la phase 3 (SPEC 9.3), franchie le 2026-11-08 : les 9 essais de
  `docs/ESSAI-EXTENSION.md` validés par l'utilisateur sous Firefox, Chrome et Edge, ainsi que
  CCleaner Browser (Chromium). Porte de la phase 2 franchie
  le même jour.
- Phase 4, plan du 2026-11-08 (validé par l'utilisateur) : 1. moteur dans l'application
  (`crates/android`, `apps/android`, HTTPS ; fait le 2026-11-08) ; 2. dossier public
  `Download/AuroDownloader` par chemins ordinaires (fait le 2026-11-08, décision de
  l'utilisateur ; le sélecteur de stockage, avec son repli, vient plus tard) ;
  3. application Kotlin et Jetpack Compose (liste, ajout, partage, détails, réglages ;
  fait le 2026-11-08, essai TalkBack à faire) ; 4. arrière-plan (fait le
  2026-11-09, SPEC 7.15 : transfert lancé par l'utilisateur à partir d'Android 14, service
  avant, notification, Wi-Fi seulement, redémarrage ; vrai redémarrage et « Tout mettre en
  pause » essayés sur la tablette le 2026-11-09) ; 5. porte : téléchargements longs sur deux appareils
  réels, Android 15 et 16 (décision de l'utilisateur ; le téléphone est passé à Android 17).
  Porte franchie le 2026-11-09, déclarée par l'utilisateur après les essais sur les deux
  (SPEC 7.15 et 9.3), Android 17 accepté à la place du 16. Restes : TalkBack, Android 8 à
  13, sélecteur de stockage (carte SD, Drive).
- Phase 5, plan du 2026-11-09 (validé par l'utilisateur) : 1. licence GPL-3.0 ou ultérieure
  (N-11, décision du même jour : `LICENSE`, champ `license` des paquets) et contrôle des
  licences des dépendances (`cargo deny check licenses`, `deny.toml` ; fait le même jour,
  aucune dépendance incompatible) ; 2. prototype
  torrent : librqbit (Rust) face à libtorrent-rasterbar (C++), mesures présentées à
  l'utilisateur avant le choix ; 3. module `torrent` du moteur (magnet, `.torrent`, DHT,
  PEX, choix des fichiers, débit montant, liaison à une interface réseau, banc de
  reprise) ; 4. module `media` : yt-dlp et ffmpeg en processus séparés (yt-dlp récupéré
  avec contrôle de l'empreinte publiée, mise à jour indépendante, F-22, F-23, F-25),
  essais sur des vidéos sous licence libre ; 5. interfaces (bureau, CLI, extension ;
  torrent sur Android) ; 6. porte. Décisions du même jour : le torrent d'abord ; la vidéo
  seulement sous Windows et Linux en v1 (pas sur Android).
- Limites de débit (SPEC 7.6) : toute lecture réseau d'un segment attend
  `Throttle::ready`, puis paie ce qu'elle a reçu (`Throttle::pay`, dans
  `transfer::pump`). Un seau dont la limite change se règle par `Bucket::set_rate`,
  jamais en le remplaçant : les connexions en cours gardent le leur.
- Espace disque (SPEC 7.6 bis) : un manque de place, mesuré ou constaté à l'écriture,
  finit en pause `disk_full` par `record_failure`, jamais en échec ; le planificateur
  relance ces tâches quand la place revient.
- Changements d'état d'une tâche dans le moteur : par les commandes de `Engine`, que la
  boucle du planificateur traite une à une ; ne pas appeler `change_state` à côté d'elle
  pendant qu'elle tourne.
- Porte de la phase 1 (SPEC 9.3), franchie le 2026-10-16 :
  - N-01 et débit : portes tenues sous Linux (WSL, relevé du 2026-10-16 dans
    `docs/MESURES.md`) ; seuil 1,0 confirmé. Windows (relevé du 2026-10-16) : débit
    tenu ; processeur de la machine trop bruité sur un poste de travail ouvert, celui
    du processus seul tient (0,46 à 0,78). Le travail `perf` de la CI n'a pas encore
    tourné (pas de dépôt distant).
  - Banc de reprise en version longue (1 000 cycles sur 1 Gio) : passé le 2026-10-15
    (SPEC 9.1).
  - Essai Android (SPEC 7.5) : stockage interne et sélecteur de stockage (dossier
    « Téléchargements ») validés sur un appareil ; Google Drive refuse `"rw"`, le repli
    (fichier temporaire interne copié à la fin) y fonctionne. HTTPS y était refusé par `TLS_UNAVAILABLE`
    (`net::tls_available`) faute d'initialiser `rustls-platform-verifier` ; l'application
    le fait depuis le 2026-11-08 (phase 4, étape 1).
- Compilation Android : NDK du SDK (`ndk/27.2.12479018`), cible `aarch64-linux-android`,
  API 26 ; variables `CC_`, `AR_` et `CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER` vers
  `aarch64-linux-android26-clang.cmd` et `llvm-ar.exe`. Tests du moteur sur l'appareil :
  `cargo test --no-run`, `adb push` dans `/data/local/tmp`, `chmod 755`, lancer avec
  `TMPDIR` dans `/data/local/tmp`.

## Façon de travailler

- Relire le fichier concerné avant de le modifier, plutôt que de travailler de mémoire.
- Après toute modification : `cargo fmt`, `clippy` et `cargo test`, puis donner le rapport
  complet de compilation et de tests avant de dire que c'est terminé. Ne jamais annoncer
  qu'un changement est prêt sans l'avoir vérifié.
- Vérifier l'équilibre structurel (accolades, parenthèses) de tous les fichiers touchés, pas
  seulement de celui en cours, et les livrer tous.
- Si une fonctionnalité touche plusieurs fichiers, le dire explicitement avant de livrer.
- Quand un comportement visuel ou d'exécution n'est pas vérifiable directement (Android,
  Windows, réseau réel), le dire clairement plutôt qu'affirmer que ça fonctionnera.

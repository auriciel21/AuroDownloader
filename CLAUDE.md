# AuroDownloader : gestionnaire de téléchargement libre

Gestionnaire de téléchargement gratuit et open source pour Windows, Linux et Android.
Un moteur unique, processus autonome, piloté par une API JSON-RPC 2.0. Les interfaces
(bureau, Android, web, CLI, extension navigateur) ne contiennent aucune logique de
téléchargement.

## Référence

- `docs/SPEC.md` : cahier des charges, modèle de données (section 5), protocole du moteur
  (section 6), algorithmes (section 7), tests et feuille de route (section 9). À lire avant
  toute modification du moteur.
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
  espace libre et place occupée ; préallocation à écrire), `net` (préparation, requêtes de segments avec
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
  exécutée), `power` (veille et extinction ; branchées par `aurod` seulement).
- `crates/daemon` (`auro-daemon`, binaire `aurod`) : le moteur en processus autonome, qui
  sert le protocole sur le point d'accès local.
- `crates/engine/migrations/` : migrations SQL numérotées, intégrées au binaire.
- `crates/cli` (`auro-cli`, binaire `auro`) : ligne de commande, cliente d'`aurod`
  (`client` : connexion, appels, démarrage d'`aurod` par `launch` ; `remote` : commandes ;
  `display`). Le mode `direct` exécute une tâche avec la bibliothèque, sans moteur : les
  deux bancs l'utilisent, pour mesurer le transfert seul.
- Essais manuels de la CLI : toujours sur un point d'accès et une base d'essai
  (`--endpoint`, `engine start --db`), jamais sur le moteur réel de l'utilisateur ; finir
  par `engine stop` et vérifier qu'aucun `aurod` d'essai ne reste.
- `apps/desktop` (`auro-desktop`, binaire `aurodownloader`) : application de bureau Tauri 2
  (SPEC 4.3), espace de travail Cargo à part (Tauri demande WebKitGTK 4.1, absent de
  l'Ubuntu 20.04 de WSL). `bridge` : liaison avec le moteur (démarrage d'`aurod`, session,
  abonnement, reconnexion, avancement suspendu fenêtre cachée), sans dépendre de Tauri ;
  `prefs` : préférences de l'interface (JSON, aucun secret) ; `files` : ouverture d'un
  fichier terminé, jamais d'un exécutable (SPEC 8.1).
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
- Essais d'une action de fin (veille, extinction) : toujours avec un `aurod` d'essai lancé
  à la main avec `--simulate-power` avant l'application (qui s'y connecte au lieu d'en
  lancer un), et vérifier sa ligne de commande ; jamais avec un `aurod` qui pourrait
  éteindre la machine de l'utilisateur.
- `experiments/android-saf` : essai de faisabilité Android (application Java minimale,
  construite sans Gradle par `build.sh --install`) ; ne fait pas partie du produit.
- `experiments/desktop` : prototypes Qt Quick (`qt`, CMake et Qt 6 MSVC dans `C:\Qt`) et
  Tauri (`tauri`, hors de l'espace de travail Cargo) de l'application de bureau, au
  périmètre identique, et leurs mesures (`README.md`) ; ne font pas partie du produit.

## Commandes

```
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p auro-cli -- db migrate auro.db
cargo run -p auro-cli -- probe https://deb.debian.org/debian/README
# Client du moteur ; `aurod` démarre de lui-même s'il ne tourne pas (base et point
# d'accès par défaut de l'utilisateur). Pour un essai isolé : --endpoint et
# `engine start --db`, puis `engine stop`.
cargo run -p auro-cli -- get https://deb.debian.org/debian/README -o .
cargo run -p auro-cli -- add <url>… --paused ; list ; resume <id>… ; watch ; remove <id>… ; engine stop
cargo run -p auro-cli -- schedule set --days 1-5 --from 23:00 --to 07:00 --then sleep ; schedule list
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
  vérifient la table de la SPEC 5.5. Ne jamais écrire `tasks.state` directement.
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
  (`apps/desktop`, `ui`, fenêtre à la manière d'IDM ; essai NVDA par l'utilisateur à
  faire) ; 6. plages horaires (F-16, fait le 2026-11-07 : SPEC 7.11, `auro schedule`,
  boîte « Planification »). Porte : utilisation quotidienne
  sans perte de données.
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
    (fichier temporaire interne copié à la fin) y fonctionne. HTTPS y est refusé par `TLS_UNAVAILABLE`
    (`net::tls_available`) tant que l'application n'initialise pas
    `rustls-platform-verifier` (phase 4).
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

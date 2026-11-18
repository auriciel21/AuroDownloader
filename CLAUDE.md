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
  par l'utilisateur le 2026-11-08 (porte de la phase 3, franchie) ; essais 10 à 17 de
  l'étape 5d (torrents et vidéo) validés sous Edge le 2026-11-10.
- `docs/protocol` : contrat du protocole (SPEC 9.1, phase 7) : schémas JSON de chaque méthode
  et notification, définitions communes, corpus des requêtes de chaque client, version 1
  figée ; tests dans `crates/engine/src/rpc/contract.rs` ; mode d'emploi dans son
  `README.md`.
- `docs/SECURITE.md` : revue de sécurité de l'interface distante, des plugins et du canal
  local (porte de la phase 6, 2026-11-12) : modèle de menace, points vérifiés, constats
  corrigés, risques résiduels. À tenir à jour quand ces parties changent.
- `docs/ACCESSIBILITE.md` : revue d'accessibilité WCAG 2.2 A et AA (phase 7, étape 7, 2026-11-18) :
  tests automatiques, revue critère par critère de la page, d'Android et de l'extension,
  constats corrigés, restes, essais réels (NVDA, Orca, TalkBack). À tenir à jour quand
  l'interface change.
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
  ajoutées par `--ca-cert` ; FTP et FTPS : `net::ftp`, voir F-08 plus bas),
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
  Torrent (SPEC 7.16, phase 5, 2026-11-09) : `torrent` (librqbit derrière ce seul module :
  session partagée créée au premier torrent, `torrent::storage` écrit dans les `.part`,
  revérification des morceaux à chaque démarrage, vidage puis renommage à la fin ; tests
  sur un essaim local, `.torrent` écrits à la main), `store::torrents` (tâche torrent, sa
  source magnet, métadonnées, `task_files`, octets envoyés, fichiers choisis). `execute`
  aiguille selon `TaskRow::kind`. Session ouverte avec le premier torrent en cours, fermée
  avec le dernier ; partage (`seeding`) jusqu'au ratio, hors des places de la file.
  Réglages `torrent.ratioLimit`, `torrent.upnp`, `torrent.listenPort` ; `limits.uploadBps`.
  Liaison à une interface (étape 3d) : réglage `torrent.interface`, `torrent::Binding` et
  `system_interface` ; le planificateur la surveille (`INTERFACE_CHECK`) et arrête les
  torrents quand elle manque ou change (génération de l'interface).
  Vidéo (SPEC 7.17, phase 5) : `media` (yt-dlp et ffmpeg en processus séparés ; Windows et
  Linux seulement), `media::openpgp` (signature des publications de yt-dlp, clé
  `media/yt-dlp-public.key`), `media::tools` (installation sur demande, empreintes,
  `tools/tools.json`, mise à jour de yt-dlp au plus une fois par jour),
  `media::extract` (`media.probe` : `yt-dlp -J`), `media::process` (arbre des processus tué :
  Job Object sous Windows, groupe sous Linux),
  `media::run` (exécution d'une vidéo : résolution, flux, sous-titres gardés à côté, fusion
  par ffmpeg), `store::media` (`media_jobs`, `media_streams` ; sous-titres : lignes avec
  `language`, migration 7) ; un flux direct est une tâche HTTP interne
  (`tasks.parent_id`, migration 6) téléchargée par `transfer::run` ; faux serveur de
  publications des tests : `testing::tools`.
  Miroirs (SPEC 7.7, phase 6) : `mirrors` (validation d'un miroir face à la référence :
  taille et échantillons ; en-têtes envoyés à chaque source), `store::sources` (ajout d'un
  miroir, état, refus, échecs) ; `task.addSource`. Répartition dans `transfer::pool` :
  une `Slot` par source (état, débit lissé, repos), `pick_slot`, `measure_slots`, miroirs
  validés pendant le téléchargement (`check_mirror`) ; le nombre de connexions vaut par
  source ; tests dans `transfer/mirror_tests.rs`. Le serveur de test `FileSite` a `shift` pour servir un
  autre fichier de même taille.
  Metalink (F-27, SPEC 7.7, étape 1d) : `metalink` (lecture des formats 4 et 3 par `quick-xml`,
  sans DTD ; noms nettoyés, sources triées par priorité, 16 au plus, empreinte la plus forte ;
  fuzzing par proptest), `Engine::read_metalink` (contenu ou adresse lue par le moteur),
  `AddTask::sub_dir` (dossiers créés sous la destination) ; méthodes `metalink.read` et
  `metalink.add` (une tâche par fichier choisi).
  Analyse de liens (F-29, F-31, SPEC 7.18, étape 2) : `linkscan` (liens d'une page HTML par
  un petit lecteur de balises, adresses écrites en clair, nom, extension et sorte ; fuzzing
  par proptest), `Engine::scan_page`, `Engine::probe_links` (tailles, 4 à la fois),
  `net::Client::fetch_page` ; méthodes `linkscan.run`, `linkscan.probe`, `task.addBatch`.
  Les filtres sont dans les clients (`crates/cli/src/links.rs`, page).
  Interface distante (F-39, SPEC 7.19, étape 3) : `remote` (jetons et sessions, empreintes
  seules ; `remote::run`, l'écoute selon `remote.enabled` et `remote.listen`, lancée par
  `aurod`), `remote::server` (hyper et tokio-tungstenite : `/ws`, `POST /rpc`, `/login`,
  `/logout` ; `Host` et `Origin` vérifiés ; TLS hors de la boucle locale ; fuzzing des
  lecteurs de requêtes et clients lents dans `remote::fuzz`), `remote::page` (la page `ui/src` intégrée, `GET /session`), `remote::tls` (certificat
  fabriqué à côté de la base, `remote-cert.pem` et `remote-key.pem` lisible par l'utilisateur
  seul, 824 jours, refait à 30 jours ou quand un nom manque ; ou fourni par
  `remote.tlsCert` et `remote.tlsKey`), `winsec` (droits Windows réservés à l'utilisateur :
  canal nommé, clé), `store::remote`, `net::reach` (réseau public
  seulement : résolveur filtrant, `Client::restricted`), `Engine::restricted` (poignée
  d'une connexion à jeton : dossier des téléchargements, `tasks.public_only`),
  `rpc::serve_messages` (tout transport) ; méthodes `token.*` et `remote.status`.
  Plugins d'hébergeurs (F-41, SPEC 7.20, étape 4b) : `plugins` (`manifest` : section
  `auro-plugin` du `.wasm` ; `runtime` : wasmi, instance neuve, limites ; `fetch` : requêtes
  du plugin vers ses domaines, réseau public, cookies du site tenus par le moteur ;
  `watch` : lien expiré redemandé au plugin, poignée faible `scheduler::WeakEngine`),
  `store::plugins` (table `plugins`, migration 10 ; `tasks.plugin_page`, `plugin_id`),
  `store::headers::replace` ; méthodes `plugin.*`, `plugin` de `task.add` ; plugins
  d'essai en texte WebAssembly : `testing::plugin_files`.
  Journaux (N-12, phase 7) : `logs` (événements `tracing` en lignes JSON dans `logs/` à côté
  de la base, fichiers tournants `Rotating`, niveau `logs.level` suivi par `logs::follow`,
  `AURO_LOG` qui l'impose ; seuls nos modules y écrivent ; `scrub`, `collect` et
  `write_archive` pour le diagnostic) ; méthode `engine.diagnostic` ; installés par `aurod`
  et par l'hôte Android (`logs::init`).
  Proxy (F-07, phase 7) : `net::proxy` (modes, validation, chemin d'une connexion
  `route`, `apply` au client, variables d'environnement de yt-dlp), `net::Client` qui se
  refait quand le proxy change (génération partagée par ses copies), réglages `proxy.*`
  (`store::settings::proxy`, mot de passe au coffre, compté par `all_secret_refs`) ;
  méthodes `proxy.get`, `proxy.set` ; proxys de test HTTP et SOCKS5 : `testing::proxies`.
  Règles de rangement et commande de fin (F-32, F-34, SPEC 7.21, phase 7) : `rules`
  (critères, modèle de nom, dossier, validation ; types recopiés de `ui/src/model.js`,
  comparés par un test), `store::rules` (table `rules`, `tasks.rule_base` et
  `tasks.rule_id`, migration 11 ; `place_task` à la préparation de chaque sorte de tâche),
  `postproc` (`finish` à la place de `complete_task`, commande lancée par l'exécution du
  planificateur, état `post_processing` durable, `POSTPROCESS_FAILED`) ; méthodes
  `rules.list`, `rules.set`.
  Morceaux faux (F-12, SPEC 7.9, phase 7) : `pieces` (empreintes de morceaux d'un Metalink,
  plages des morceaux faux), `io::checksum::hash_pieces` (une lecture, fichier entier
  compris), `segmenter::reopen`, `store::pieces` (table `task_pieces`, migration 12 ;
  `start_round` : mise en cause des sources, segments rouverts, `verifying` vers `active`),
  `sources.bad_pieces`, `segments.source_id` noté par `transfer::pool` ; boucle de
  `transfer::execute` (`Finish::Redo`) ; tests dans `transfer/piece_tests.rs`, octets faux
  du serveur de test (`FileSite::corrupt`).
  FTP et FTPS (F-08, SPEC 7.22, phase 7, étape 6b) : `net::ftp` (suppaftp derrière
  `probe_as`, `get_range_as`, `get_all_as` ; `net::Body` en énumération ; `SIZE`, `MDTM`
  validateur, `REST`, `ABOR` ; réserve des connexions de commande `ftp::Shared` ; données
  toujours vers l'hôte de la commande ; TLS implicite ou `AUTH TLS` exigé avec un mot de passe,
  sauf `site_logins.allow_plain`, migration 13), `net::socks` (client SOCKS5 des connexions
  FTP), `net::parse_task_url` et `check_task_url` (HTTP(S) et FTP(S) ; `parse_url` reste
  HTTP(S) : miroirs, analyse de liens, plugins), identifiants de l'adresse rangés au coffre à
  l'ajout (`ftp::split_credentials`) ; tâches FTP de sorte `http`, une source, 4 connexions
  au plus en automatique ; serveur de test `testing::ftp` (libunftp, tests seulement).
  SFTP (F-08, SPEC 7.22, étape 6c) : `net::sftp` (russh et russh-sftp ; clé du serveur
  comparée à `HostKeys`, chargées de `ssh_host_keys` au démarrage, migration 14 ; clé inconnue
  ou changée gardée en attente en mémoire, `HOST_KEY_UNKNOWN` / `HOST_KEY_CHANGED` ; une
  connexion SSH par segment, réserve `sftp::Shared` ; identifiants à clé privée
  `site_logins.key_path`, `Auth::with_key`), `net::dial` (connexions TCP communes à FTP et
  SFTP : SOCKS5, réseau public), `store::host_keys` ; méthodes `hostkey.list`,
  `hostkey.accept` (relance les tâches bloquées), `hostkey.remove` ; CLI `auro hostkey`,
  `auro login set --key` ; serveur de test `testing::sftp` (russh, tests seulement).
- `third_party` : copies corrigées de `librqbit` et `librqbit-dualstack-sockets` (liaison à une
  interface sous Windows), branchées par `[patch.crates-io]` dans les deux espaces de
  travail, hors de l'espace de travail ; modifications et mise à jour dans
  `third_party/README.md`.
- `crates/daemon` (`auro-daemon`, binaire `aurod`) : le moteur en processus autonome, qui
  sert le protocole sur le point d'accès local.
- `crates/engine/migrations/` : migrations SQL numérotées, intégrées au binaire.
- `crates/cli` (`auro-cli`, binaires `auro` et `auro-relay`) : ligne de commande, cliente
  d'`aurod` (bibliothèque `auro_cli` : `client`, connexion, appels, démarrage d'`aurod`
  par `launch` ; `relay`, hôte de messagerie native de l'extension, SPEC 4.5 ; binaire
  `auro` : `remote`, commandes, et `display`), `video` (`auro video`). Le mode `direct` exécute une tâche avec la bibliothèque, sans moteur : les
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
  fichier terminé, jamais d'un exécutable (SPEC 8.1) ; `opened` : liens magnet et
  `.torrent` reçus (arguments, relance, glisser-déposer), lus pour le moteur ; `links` :
  association aux liens magnet et aux `.torrent` (option, désactivée par défaut ; ses tests
  n'écrivent que sous `HKCU\Software\AuroDownloaderEssai` ou dans un dossier temporaire ;
  ne jamais l'activer pour de vrai sans l'accord de l'utilisateur) ; `clipboard` : liens
  copiés (F-30, option désactivée par défaut ; notification du système, clic vers
  « Ajouter » ; sous Windows, une variante lancée depuis `target` notifie sous le nom de
  PowerShell) ; `qr` : matrice du QR code de l'accès à distance (étape 3d, rubrique des
  Options ; la page la dessine) ; plugins (étape 4c) : `pick_plugin` et `opened::read_plugin`
  (`.wasm` choisi, lu en base64 pour le moteur, jamais exécuté par l'application), rubrique
  « Plugins » des Options de la page. SFTP (étape 6d) : `pick_key` (fichier de clé privée choisi,
  relu par le moteur), section « Clé du serveur SSH » des Propriétés et clés SSH des Options
  dans la page. Paquets : `bundle.ps1`,
  `bundle.sh`, `tauri.bundle.json` (ajoute `aurod` et `auro`), `windows/hooks.nsh`
  (arrêt du moteur, effacement des données sur demande), `icons/icon.svg` (source des
  icônes).
- `ui` : la page web de l'application, destinée aussi à l'interface distante (F-39). `src/web.js`
  (hôte WebSocket de la page servie par le moteur, étape 3c ; éléments marqués `data-needs` selon
  la portée et l'hôte),
  `src/model.js` (instantané, notifications et révisions, SPEC 6.10 ; vues de
  l'arborescence, types de fichier, tri des colonnes), `src/i18n.js` (français source,
  anglais), `src/host.js` (services de l'hôte), `src/main.js` (fenêtre principale :
  barre d'outils, arborescence, tableau à sélection multiple, menu contextuel,
  propriétés, options) ; tests `node --test` dans `tests/`. Accessibilité (étape 7a) :
  `tests/axe.test.js` (balisage, chaque boîte), `tests/live.test.js` (la vraie page et un
  faux moteur par `window.__TAURI__`, focus au clavier), `tests/contrast.test.js`
  (couleurs de `style.css`) ; axe-core et jsdom en dépendances de développement npm
  (`package-lock.json`, `node_modules` hors du dépôt, jamais livrés).
- Essais manuels de l'application : `--endpoint`, `--db` et `--prefs` d'essai (elle lance
  `aurod` avec les deux premiers), jamais le moteur ni les préférences réels ; ne pas
  toucher au démarrage avec la session (clé `Run` de l'utilisateur) ; finir par
  `engine stop` sur ce point d'accès. Pour essayer sans fermer l'application de
  l'utilisateur : la construire avec l'identifiant d'essai (`cargo tauri build --no-bundle
  --config tauri.essai.json`), qui a sa propre instance unique. Ne jamais arrêter les
  processus `aurodownloader` ou `aurod` par leur nom : seulement ceux de `target\` ou du
  dossier d'essai (celui de l'utilisateur porte le même nom).
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
  Edge ; `decide.js` (décision de capture, `.torrent` et liens magnet, testée par
  `node --test extension/tests/`), `background.js`, `content.js` (clic avec Alt, clic sur un
  lien magnet), `popup.*` et `video.js` (fenêtre de l'icône : vidéo de la page ; qualités
  recopiées de `ui/src/model.js`, comparées par les tests), `options.*`, `_locales` (en, fr).
  Accessibilité (étape 7b) : `tests/a11y.test.cjs` (axe-core et jsdom pris dans `ui/node_modules`).
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
  partagés ou ouverts (activité sans fenêtre, toujours neuve ; liens magnet et `.torrent`
  depuis l'étape 5e, lus par `AuroApp.readTorrent`). `Background` (SPEC 7.15) :
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
- `experiments/plugins` : prototype des plugins (phase 6, étape 4a) : `sample` (plugin
  d'essai, `wasm32-unknown-unknown`, exports hostiles), `host` (wasmtime ou wasmi selon
  `--features`, sans eux binaire de référence), espace de travail Cargo à part ; mesures et
  décision (wasmi) dans `README.md`. Ne fait pas partie du produit.
- `experiments/ftp` : prototype FTP, FTPS et SFTP (phase 7, étape 6a) : `proto` (suppaftp,
  russh et russh-sftp ; `serve-ftp`, serveur libunftp local), espace de travail Cargo à
  part ; essais réels (`test.rebex.net`, FTP anonymes) et mesures dans `README.md`. Ne fait
  pas partie du produit.
- `plugins` : plugins d'hébergeurs d'exemple (étape 4d), espace de travail Cargo à part
  (`wasm32-unknown-unknown`) : `archive-org` (éléments d'Internet Archive par l'interface
  `archive.org/metadata`), module `auro.rs` recopiable (interface version 1), manifeste rangé
  par `#[link_section = "auro-plugin"]` ; guide de l'auteur dans `README.md`. Tests sur la
  machine (`cargo test`), licences par `cargo deny --manifest-path plugins/Cargo.toml
  --config deny.toml check licenses`.

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
cargo run -p auro-cli -- get <url> --mirror <url>… ; mirror <id> [<url>]   # miroirs
cargo run -p auro-cli -- metalink <fichier.meta4|adresse> ; get <fichier.meta4> --select 0,2   # Metalink
cargo run -p auro-cli -- links <page|liste.csv|-> -e iso,zip --min-size 10M ; links … --add -o .
cargo run -p auro-cli -- login set <site> -u <nom> ; login list ; login remove <site>
cargo run -p auro-cli -- token create <nom> --scope control ; token list ; token revoke <nom|id>
cargo run -p auro-cli -- plugin install <fichier.wasm> [--yes] ; plugin list ; plugin resolve <page>
cargo run -p auro-cli -- plugin add <page> --select 0,2 -o . ; add <page> --no-plugin
# Plugin d'exemple (dans plugins ; cible : rustup target add wasm32-unknown-unknown) :
cargo test ; cargo build --release --target wasm32-unknown-unknown
cargo run -p auro-cli -- remote enable --listen 127.0.0.1:7380 ; remote ; remote disable
cargo run -p auro-cli -- remote hosts nas.maison ; remote cert --file c.pem --key k.pem ; remote cert --generated
cargo run -p auro-cli -- add <url> --checksum sha256:<hex>   # ou la valeur seule
cargo run -p auro-cli -- checksum <id> <empreinte> ; checksum <id> --clear
cargo run -p auro-cli -- history [texte] [--csv] ; history --clear
cargo run -p auro-cli -- schedule set --days 1-5 --from 23:00 --to 07:00 --then sleep ; schedule list
cargo run -p auro-cli -- dir <dossier> ; dir --reset   # dossier des ajouts sans dossier (extension)
cargo run -p auro-cli -- add <url> -H "Referer: …" -H "Cookie: …"   # Cookie : coffre du système
cargo run -p auro-cli -- add fichier.torrent ; files <id> --select 0,2-4 ; limit --upload 500K
cargo run -p auro-cli -- config ; config torrent.ratioLimit 1.5 ; config media.autoUpdate false
cargo run -p auro-cli -- config logs.level debug ; diagnostic ; diagnostic --save diag.zip
cargo run -p auro-cli -- proxy ; proxy manual socks5://hôte:1080 --except localhost -u moi ; proxy none ; proxy system
cargo run -p auro-cli -- rules ; rules add --type video --folder Vidéos --rename "{date} {nom}.{ext}" ; rules remove 1
cargo run -p auro-cli -- rules add --exec /usr/bin/clamscan --arg {chemin} --timeout 600 ; get <url> --auto
cargo run -p auro-cli -- video tools --install ; video info <page> ; video get <page> --subs fr,en
# Sans moteur (bancs, dépannage) : la bibliothèque sur une base donnée.
cargo run -p auro-cli -- direct --db essai.db get https://deb.debian.org/debian/README -o .
cargo run -p auro-daemon -- --db essai.db   # moteur ; Ctrl-C l'arrête proprement
# Vidéo : installation réelle des outils depuis les publications officielles (Internet,
# rien n'est exécuté) :
# … et yt-dlp sur des pages de vidéos sous licence libre (outils gardés dans AURO_TOOLS_DIR) :
AURO_TOOLS_DIR=<dossier> cargo test -p auro-engine --lib -- --ignored real_probe --nocapture
# … et vidéos fabriquées par ffmpeg, servies en local et téléchargées (sans Internet) :
AURO_TOOLS_DIR=<dossier> cargo test -p auro-engine --lib -- --ignored real_local --nocapture
# … et vidéos libres réelles téléchargées jusqu'au bout, sous-titres et HLS compris
# (PeerTube, Commons, Archive, Framatube ; une centaine de Mio) :
AURO_TOOLS_DIR=<dossier> cargo test -p auro-engine --lib -- --ignored real_free --nocapture
# FTP (AUTH TLS) et SFTP réels vers test.rebex.net par le proxy SOCKS5 de test (Internet) :
cargo test -p auro-engine --lib -- --ignored real_socks5_rebex --nocapture
# Fuzzing du serveur de l'interface distante, version longue (requêtes abîmées) :
AURO_FUZZ_CASES=5000 cargo test -p auro-engine --lib remote::fuzz -- --nocapture
# Banc de reprise sous arrêt brutal, version longue (la courte tourne avec cargo test) :
AURO_CRASH_CYCLES=1000 AURO_CRASH_SIZE=1073741824 AURO_CRASH_SECONDS=8 \
  cargo test --release -p auro-cli --test crash_resume -- --nocapture
# Banc de reprise du moteur autonome (aurod tué pendant que plusieurs tâches tournent ;
# la version courte tourne avec cargo test) :
AURO_CRASH_CYCLES=150 cargo test --release -p auro-daemon --test crash_daemon -- --nocapture
# Banc de reprise des torrents (aurod tué pendant que plusieurs torrents se téléchargent
# depuis un pair local, écritures perdues simulées ; aurod --torrent-peers) :
AURO_CRASH_CYCLES=150 cargo test --release -p auro-daemon --test crash_torrent -- --nocapture
# Banc de reprise des vidéos (aurod tué pendant les flux et la fusion ; faux yt-dlp et
# ffmpeg imposés par aurod --media-ytdlp --media-ffmpeg --media-ffprobe) :
AURO_CRASH_CYCLES=150 cargo test --release -p auro-daemon --test crash_media -- --nocapture
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
# d'essai Ubuntu-24.04, avec npm 10.9.2 dans ~/tools/npm, lancé par
# `node ~/tools/npm/bin/npm-cli.js`) ; dépendances des tests d'accessibilité d'abord :
cd ui && npm ci --ignore-scripts && cd ..
node --test ui/tests/
node --test extension/tests/
# Installateurs (SPEC 4.6 ; tauri-cli : cargo install tauri-cli --version "^2" --locked) :
apps/desktop/bundle.ps1   # Windows, NSIS : apps/desktop/target/release/bundle/nsis/
apps/desktop/bundle.sh    # Linux, .deb et AppImage (distribution WSL Ubuntu-24.04)
# Application Android (dans apps/android ; compile aussi le moteur pour arm64) :
cd apps/android && ./gradlew assembleDebug   # app/build/outputs/apk/debug/app-debug.apk
adb install -r app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n org.aurodownloader/.MainActivity --es url <adresse>
# Accessibilité (étape 7c) : tests instrumentés sur un appareil, sans connectedAndroidTest
# (qui désinstalle l'application) ; l'application réelle tourne : effacer ses données après.
./gradlew assembleDebug assembleDebugAndroidTest
adb install -r app/build/outputs/apk/debug/app-debug.apk
adb install -r app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk
adb shell am instrument -w -r org.aurodownloader.test/androidx.test.runner.AndroidJUnitRunner
adb shell pm clear org.aurodownloader ; adb uninstall org.aurodownloader.test
```

## Règles du projet

- La fiabilité passe avant les fonctionnalités : aucune modification de la reprise ou de
  l'intégrité sans test qui la couvre.
- Une migration publiée ne se modifie jamais : ajouter `000N_nom.sql`, l'enregistrer dans
  `MIGRATIONS` (`store.rs`) et mettre à jour la section 5 de la spécification.
- Protocole : ajouter un champ optionnel ou une méthode ne casse rien ; renommer ou supprimer
  exige une nouvelle version majeure (SPEC 6.1). Mettre à jour la section 6, et le schéma de
  la méthode ou de la notification dans `docs/protocol` (les tests du protocole valident
  chaque réponse et chaque notification contre lui ; une méthode sans schéma les fait
  échouer). Une requête nouvelle d'un client va dans son corpus (`docs/protocol/corpus`).
  Ne jamais modifier `docs/protocol/v1` (version 1 figée, garde de compatibilité).
- Ordre d'écriture : octets écrits et vidés sur disque (`fdatasync`), puis seulement le
  compteur en base (SPEC 5.6, règle 1).
- Aucune télémétrie. Aucun secret en base ni dans les journaux (SPEC 5.1 et 8.2).
- Journaux (N-12) : par `tracing` seulement (jamais `eprintln!` dans le moteur, hors
  erreurs de démarrage d'`aurod`). Jamais une adresse complète, un en-tête ni un nom de
  fichier dans un événement : l'identifiant de la tâche, un hôte au plus (champ `host`,
  que le diagnostic retire), un chemin dans un champ (que le diagnostic coupe). Tout ce
  que rend `engine.diagnostic` passe par `logs::scrub`.
- Proxy (F-07) : toute connexion HTTP du moteur passe par `net::Client` (ou `plugin_http`),
  qui applique `net::proxy::apply` ; ne jamais construire un client reqwest à côté. Une
  connexion limitée au réseau public ne suit qu'un SOCKS5 en résolution locale
  (`proxy::route`), jamais un proxy HTTP. Les torrents ne passent jamais par le proxy.
  Programmes externes : le proxy par leur environnement, jamais en argument.
- Règles et commande de fin (SPEC 7.21) : une tâche ne finit que par `postproc::finish`
  (jamais `complete_task` directement à la fin d'un téléchargement), qui passe par
  `post_processing` quand sa règle a une commande. Le rangement s'enregistre dans la
  transaction qui enregistre le nom pris (`store::rules::save_placement`). Les règles ne
  s'écrivent que par le canal local (`rules.set`, portée `admin`) ; un jeton ne voit pas
  les commandes. Une commande : programme et arguments, jamais de shell, rien de ses
  arguments ni de ses sorties dans les journaux ; les flux internes d'une vidéo n'ont
  jamais de règle.
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
- Noms de fichier pris sur le disque (SPEC 5.6, règle 7) : toujours par `claims::claim`
  (réservation notée dans `name_claims` avant la création du `.part` ou du dossier), puis
  enregistrés dans une transaction qui appelle `store::claims::release` ; `claims::recover`
  au démarrage (moteur et mode `direct`) efface ce qu'un arrêt a laissé. Ne jamais créer
  un `.part` de nouveau nom autrement.
- Ordre d'écriture, application : `written_bytes` ne s'enregistre que par
  `store::segments::save_progress`, qui n'accepte que des `io::SyncedProgress`, fabriqués
  par `io` après `sync_data`. Ne pas ajouter d'autre chemin d'écriture de ce compteur.
- Noms de fichier venus du serveur : toujours par `net::filename::sanitize` (SPEC 8.1).
  Les messages d'erreur réseau ne citent jamais l'URL ni la valeur d'un en-tête (SPEC 8.2).
- États d'une tâche : toujours par `store::tasks::change_state` ou `complete_task`, qui
  vérifient la table de la SPEC 5.5, `store::pieces::start_round` (tour de reprise des morceaux
  faux, F-12) ou `restart_task` (retéléchargement, hors de la
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
  `testing::Checker`. Toute modification du module `torrent` ou de `store::torrents` passe
  aussi celui des torrents (`crates/daemon/tests/crash_torrent.rs`, essaim local de
  `testing::swarm`). Toute modification du module `media` ou de `store::media` passe aussi celui des
  vidéos (`crates/daemon/tests/crash_media.rs`, faux outils de `testing::tools`).
- Tests qui démarrent un moteur dans un dossier temporaire : finir par
  `testing::stop(&engine).await` (arrêt, puis fermeture de la base attendue) ; sous Windows,
  un fichier encore ouvert ne s'efface pas et le dossier resterait dans `%TEMP%`.
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
  torrent : librqbit (Rust) face à libtorrent-rasterbar (C++) (fait le même jour,
  `experiments/torrent/README.md` ; décision de l'utilisateur : librqbit, sans
  chiffrement du protocole, sources web ni torrents v2 en v1 ; la liaison à une
  interface sous Windows sera ajoutée par le projet) ; 3. module `torrent` du moteur,
  conception SPEC 7.16 (décisions de l'utilisateur : partage jusqu'au ratio 1, port
  d'écoute sans UPnP ; revérification complète à chaque démarrage, sans reprise rapide) :
  3a. ajout (magnet, `.torrent`) et téléchargement jusqu'à la fin, essaim local dans les
  tests (fait le 2026-11-09) ; 3b. partage, limites (envoi compris), choix des fichiers,
  réglages, réseau (fait le 2026-11-09, essai réel sous WSL : netinst Debian par magnet,
  DHT seul) ; 3c. banc de reprise et écritures perdues (fait le 2026-11-09,
  `crash_torrent`) ; 3d. liaison à une interface (VPN), Windows compris (faite le 2026-11-09 :
  copies corrigées de librqbit, torrents en attente sans l'interface, torrents seulement ;
  décisions de l'utilisateur ; essai réel par son VPN sous Windows le même jour) ; 4. module `media` : yt-dlp et ffmpeg en processus séparés (yt-dlp récupéré
  avec contrôle de l'empreinte publiée, mise à jour indépendante, F-22, F-23, F-25),
  essais sur des vidéos sous licence libre ; conception SPEC 7.17 (décisions de
  l'utilisateur du 2026-11-09 : architecture mixte, yt-dlp n'extrait que, le moteur
  télécharge les flux directs et yt-dlp les flux fragmentés, ffmpeg fusionne ; outils
  téléchargés au besoin sur accord, empreinte vérifiée ; yt-dlp mis à jour seul, signature
  vérifiée ; essais réels sans YouTube, exception à « pas d'exécutable téléchargé » pour
  yt-dlp et ffmpeg officiels) : 4a. outils (fait le 2026-11-09 : installation
  vérifiée, essai réel sous Windows et Linux) ; 4b. extraction (fait le 2026-11-09) ; 4c. ajout, flux et
  fusion (fait le 2026-11-09) ; 4d. banc de reprise des vidéos et limite de
  débit des flux fragmentés (fait le 2026-11-09) ; 4e. essais réels, sous-titres (fait le
  2026-11-09 : sous-titres à côté de la vidéo, au format du site, langue manquante signalée
  par `SUBTITLES_MISSING`, décisions de l'utilisateur ; essais réels sous Windows et Linux,
  qui ont corrigé `If-Range` sans guillemets et les plages annoncées sans validateur,
  SPEC 7.1 et 7.4) ; 5. interfaces, plan du 2026-11-09 (validé) : 5a. CLI (fait le
  2026-11-09 : `auro video`, `auro config`, `auro files`, `.torrent` local, `limit
  --upload`, SPEC 4.6 ; essais réels Framatube sous Windows et netinst Debian sous WSL),
  5b. bureau : torrents (fait le 2026-11-09, essai de l'interface validé par l'utilisateur
  sur la variante d'essai : ajout magnet et `.torrent`, glisser-déposer,
  « Ouvrir avec », Propriétés et choix des fichiers, rubrique BitTorrent, association en
  option désactivée, `torrent.interfaces`, SPEC 4.6 ; `opened.rs`, `links.rs`), 5c. bureau :
  vidéo (fait le 2026-11-09, essai de l'interface validé par l'utilisateur, outils
  installés depuis la boîte : boîte « Ajouter une vidéo », qualités et formats,
  sous-titres, installation des outils sur accord, rubrique Vidéo ; le moteur refuse
  empreinte, déplacement et adresse pour une vidéo),
  5d. extension (écrite le 2026-11-10 : clic sur un lien magnet capté, option active par
  défaut ; `.torrent` lu par l'extension et envoyé en base64 ; fenêtre de l'icône avec le
  choix de la qualité, menu contextuel en meilleure qualité ; messages `probe` et `video` du
  relais ; messages du moteur et du relais portés à 24 Mio ; décisions de l'utilisateur ;
  essais 10 à 17 de `docs/ESSAI-EXTENSION.md` validés par l'utilisateur sous Edge le même
  jour, Firefox et Chrome à la porte ; lien magnet rendu au navigateur seulement si
  AuroDownloader est injoignable ; menus contextuels recréés à chaque démarrage),
  5e. torrents sur Android (fait le 2026-11-10 : périmètre du bureau, ajout direct de ce
  qu'une autre application confie, partage seulement application à l'écran ou
  téléchargement protégé, pas de liaison à une interface ; décisions de l'utilisateur ;
  essai réel sur le téléphone, SPEC 4.4 ; « Annuler » et « Retirer » effacent le `.part`) ;
  6. porte, franchie le 2026-11-10, déclarée par l'utilisateur (campagne du même jour : bancs et essais réels sous Windows et Linux, fuzzing des `.torrent`
  (`torrent::fuzz`, décision de l'utilisateur), licences ; trouvés et corrigés : torrents
  aux fichiers au même chemin une fois nettoyés, refusés, et fin d'une vidéo interrompue
  après le renommage, `already_merged` ; SPEC 9.1 et 9.3). Décisions du même jour : le torrent d'abord ; la vidéo
  seulement sous Windows et Linux en v1 (pas sur Android).
- Phase 6, plan du 2026-11-10 (validé par l'utilisateur) : 1. miroirs (F-26, F-28 :
  plusieurs sources par tâche, répartition par débit, bascule ; puis Metalink, F-27 ;
  1a protocole et CLI, 1b répartition et bascule, faites le 2026-11-10 ; 1c bureau, faite
  le 2026-11-11 : champ « Miroirs », sources et source de chaque segment dans les
  Propriétés, pas de retrait, essai de l'interface validé par l'utilisateur sur la
  variante d'essai avec des miroirs Debian réels ; 1d Metalink, faite le 2026-11-11 :
  formats 4 et 3, une tâche par fichier, fichier local ou adresse, empreinte du fichier
  seule, ordre par priorité, torrents et signatures ignorés, décisions de l'utilisateur ;
  essai réel de la CLI sur openSUSE et essai de l'interface validé par l'utilisateur ;
  étape 1 terminée) ;
  2. analyse de liens (F-29, F-31 ; F-30 presse-papiers au bureau ; SPEC 7.18, décisions de
  l'utilisateur du 2026-11-11 : page HTML lue par le moteur sans JavaScript ni exploration,
  tailles lues d'office (500, 4 à la fois), pages masquées sauf si le serveur les décrit
  comme des fichiers, toute adresse d'une liste importée, ajout groupé sans tout-ou-rien,
  bureau et CLI ; 2a moteur et CLI, 2b boîte « Analyser des liens », faites le 2026-11-11,
  essai de l'interface validé ; 2c presse-papiers, faite le 2026-11-11 : notification du
  système, ajout sur clic, désactivée par défaut, avertissement si Windows coupe les
  notifications ; `apps/desktop/src/clipboard.rs` ; essai de l'interface validé ; étape 2
  terminée) ; 3. interface web
  distante (F-39 : WebSocket, jetons, `Host` et `Origin`, TLS, SSRF ; 3a à 3e faites, la
  dernière le 2026-11-12 : page servie, rubrique du bureau et QR code, fuzzing, essai réel
  depuis un téléphone sur le Wi-Fi validé ; étape 3 terminée) ; 4. plugins (F-41 :
  décisions de l'utilisateur du 2026-11-12 : 4a prototype comparant wasmtime et wasmi
  (`experiments/plugins`, fait le même jour : wasmi choisi par l'utilisateur, SPEC 4.3) ; hébergeurs seulement en v1 (un plugin reçoit l'adresse d'une
  page de ses domaines déclarés et rend des liens directs, que le moteur télécharge) ; ni
  fichiers ni système, réseau seulement par le moteur vers les domaines déclarés (réseau
  public), limites de mémoire, de calcul et de temps ; installé depuis un fichier choisi
  par l'utilisateur, sans catalogue ni mise à jour seule, permissions et empreinte
  acceptées, de nouveau si elles changent ; Windows et Linux d'abord, Android plus tard ;
  4b moteur et CLI, faite le 2026-11-12, SPEC 7.20 ; 4c bureau et page : rubrique
  « Plugins », plugin lancé à la validation d'« Ajouter », liens à cocher comme un
  Metalink, case « sans plugin », page d'origine dans les Propriétés, décisions de
  l'utilisateur du 2026-11-12 ; 4d plugin d'exemple Internet Archive en Rust, `plugins/`,
  essai réel par la CLI ; essai de l'interface validé par l'utilisateur le 2026-11-12 sur
  la variante d'essai ; étape 4 terminée) ;
  5. porte : revue de sécurité de l'API distante et des plugins (faite le 2026-11-12,
  décisions de l'utilisateur : revue écrite et essais, failles corrigées au fil ; rapport
  `docs/SECURITE.md`, six constats corrigés). Porte franchie le 2026-11-12, déclarée par
  l'utilisateur.
- Phase 7, plan du 2026-11-12 (validé par l'utilisateur ; version 1, porte : tous les tests
  bloquants de la SPEC 9.1) : 1. contrat du protocole (schémas JSON des échanges de
  référence, client de la version précédente ; décisions du 2026-11-12 : JSON Schema, tout
  le protocole, version 1 figée et corpus des clients ; fait le même jour) ; 2. journaux et
  diagnostic (N-12) (décisions du 2026-11-12 : lignes JSON et rotation, `info` réglable,
  archive nettoyée montrée avant l'enregistrement, jamais d'adresse ; fait le même jour) ; 3. proxy
  (F-07) (décisions du 2026-11-12 : HTTP, HTTPS et SOCKS5, système par défaut ou
  manuel, tout le HTTP sauf les torrents, réseau public par SOCKS5 seulement, sinon en
  direct ; fait le même jour) ; 4. règles de rangement et commande de fin (F-32, F-34)
  (décisions du 2026-11-12 : rangement à la préparation, tâches sans dossier choisi
  seulement, type, domaine et expression, première règle seule, dossier et modèle de nom
  (dossier seul pour un torrent), commande action d'une règle, programme et arguments sans
  shell, attendue en `post_processing`, échec `POSTPROCESS_FAILED` relançable ; règles
  écrites par le canal local seulement ; CLI : dossier courant sans `-o`, `--auto` pour
  les règles ; 4a et 4b le 2026-11-16 : moteur, protocole, CLI, commit 54b8c46 ; 4c bureau et page
  écrite le 2026-11-17, rubrique « Règles » des Options ; essai de l'interface des
  étapes 2 à 4 validé par l'utilisateur le même jour sur la variante d'essai, deux défauts
  d'affichage corrigés : dossier annoncé à la préparation, avancement complet en
  `post_processing` ; étape 4 terminée) ; 5. segment fautif (F-12) (décisions du
  2026-11-17 : Metalink seulement, contrôle à la fin en `verifying`, morceau redemandé de
  préférence à une autre source, source écartée à 2 morceaux faux, 3 tours puis
  `CHECKSUM_MISMATCH`, Propriétés ; écrite le même jour, SPEC 7.9 ; essai de l'interface et essai
  réel validés ; bascule d'une référence muette vers un miroir, décision du même jour) ;
  6. FTP, FTPS, SFTP (F-08) (décisions du 2026-11-17, SPEC 7.22 : Rust pur, prototype
  d'abord ; segments, 4 connexions au plus ; clé SSH inconnue montrée et acceptée par
  l'utilisateur ; mot de passe FTP seulement sous TLS sauf autorisation par site ; SFTP par
  mot de passe ou clé privée ; SOCKS5 seulement ; un fichier par adresse ; Android aussi) ;
  6a prototype (`experiments/ftp`, 59c6cb9), Rust 1.88 (04cf712), 6b moteur FTP et FTPS
  écrit le 2026-11-17 (13aa7a5) ; 6c moteur SFTP écrit le même jour (décisions : clé
  acceptée relance seule ses tâches, `known_hosts` jamais lu, clé privée dans les identifiants
  de sites) ; 6d interfaces écrites le même jour (clé acceptée dans les Propriétés, rubrique
  des clés SSH ; Android : ajout et clé, sans écran d'identifiants ; essai de l'interface validé
  par l'utilisateur le même jour) ; 6e essais réels faits le 2026-11-17 (CLI sous Windows,
  SOCKS5 par `real_socks5_rebex`, Android sur la tablette) ; étape 6 terminée ;
  7. accessibilité (contrôles WCAG et tests
  automatiques ; essais réels NVDA, Orca, TalkBack faits par l'utilisateur ; décisions du
  2026-11-18 : axe-core et jsdom pour la page, avec un test des contrastes à part ; tests
  Compose instrumentés avec l'Accessibility Test Framework pour Android ; audit WCAG 2.2 AA
  de toutes les interfaces (page, Android, extension) écrit dans `docs/ACCESSIBILITE.md`,
  défauts corrigés au fil ; Orca sous WSLg préparé par Claude ; 7a page, 7b extension et
  7c Android faites le 2026-11-18, puis 7d rapport et essais réels) ; 8. dépôt
  GitHub (créé et poussé par l'utilisateur, jamais par Claude), CI Windows, Linux et
  émulateur Android, ARM64 construits non essayés, signature SignPath, mise à jour Tauri
  signée proposée puis installée sur accord ; 9. guide de l'utilisateur en français et en
  anglais (`docs/guide`), page de manuel, README ; 10. porte (banc de 1 000 reprises sur
  1 Gio, performance, tout le 9.1).
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
- Torrent (SPEC 7.16) : librqbit ne se voit que dans le module `torrent`. Jamais sa
  reprise rapide ni sa persistance : la reprise revérifie les morceaux sur disque, et
  l'avancement enregistré n'est qu'une indication. Un fichier d'un torrent ne se renomme ni
  ne s'efface qu'après la fermeture de tous ses accès (`storage::Factory::wait_closed`), et
  se vide sur disque avant son nom final. Ne pas créer de `.torrent` de test par
  `librqbit::create_torrent` (empreinte manquante si le dernier fichier est vide).
- Vidéo (SPEC 7.17) : une tâche interne (`parent_id` non nul) ne se lance que par l'exécution
  de sa vidéo ; toute nouvelle requête qui liste, compte ou planifie des tâches l'exclut
  (`parent_id IS NULL`), mais celles des noms pris et des contrôles d'intégrité la gardent.
- Plugins (SPEC 7.20) : un plugin ne voit jamais un secret (cookies du site, identifiants,
  jetons) ; ses requêtes passent seulement par `plugins::fetch` (domaines déclarés, réseau
  public, en-têtes de la liste) ; seuls `auro.fetch` et `auro.log` sont importables ; ses
  liens font des tâches au réseau public seulement, et un lien neuf se vérifie par une
  poignée restreinte. Un fichier de plugin ne s'installe que sur l'empreinte acceptée par
  l'utilisateur, et se revérifie à chaque chargement. Rien ne s'exécute hors de
  `auro_resolve` (module à fonction de démarrage refusé) ; toute ressource d'une instance
  a sa limite dans `runtime` (calcul, mémoire, pile, taille des tables) : un nouveau
  réglage de wasmi qui alloue se borne aussi. Un lien neuf refusé ne laisse pas ses cookies.
- Canal local (SPEC 6.2) : le client vérifie le propriétaire du canal nommé
  (`winsec::owned_by_current_user`) ou du dossier du socket avant d'écrire ; le moteur fait
  de l'utilisateur le propriétaire de son canal. Ne pas retirer ces contrôles.
- Portées (SPEC 6.9) : toute méthode ajoutée à `rpc/methods.rs` reçoit la sienne dans
  `rpc::scope::required` (sinon permise à toute portée) ; le test
  `every_served_method_has_a_scope` le vérifie.
- Copies corrigées (`third_party`) : ne jamais modifier une dépendance ailleurs. Toute
  modification y est marquée « AuroDownloader » et notée dans `third_party/README.md` ;
  monter la version d'une de ces crates demande d'y reporter les modifications.
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

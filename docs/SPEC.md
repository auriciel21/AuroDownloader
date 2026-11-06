# Spécification — Gestionnaire de téléchargement libre (Windows, Linux, Android)

Oct 15, 2026 · @Maxime Frimon

## 1. Vision et périmètre

Le projet est un gestionnaire de téléchargement gratuit et open source pour Windows, Linux et Android, construit autour d'un moteur unique piloté par une API locale. Il vise la vitesse d'IDM, l'ouverture d'aria2 et une simplicité que JDownloader n'offre pas.

### Objectifs mesurables (cibles à valider par benchmark)

| Objectif | Cible |
| --- | --- |
| Vitesse | Écart ≤ 5 % face à IDM et aria2 sur le même fichier, serveur et réseau |
| Fiabilité | 0 fichier corrompu sur 1 000 cycles de coupure et reprise automatisés |
| Simplicité | Un téléchargement ajouté en 2 actions au plus (copier un lien, valider) |
| Légèreté | Moteur au repos sous 50 Mo de RAM sur ordinateur |
| Reprise | Reprise après redémarrage de la machine, sans perte de segment terminé |

### Profils visés

- **Grand public** : colle un lien, obtient le fichier, ne touche à aucun réglage.
- **Utilisateur avancé** : files, planification, limites de débit, règles de rangement, vidéo.
- **Administrateur** : moteur sur un NAS ou un serveur, piloté à distance depuis un navigateur, le téléphone ou un script.

### Périmètre de la version 1

**Inclus** : HTTP(S), FTP(S), torrent et magnet, flux HLS et DASH, vidéo via yt-dlp, multi-miroirs, analyse de liens, extension navigateur (Firefox, Chrome, Edge), application de bureau (Windows, Linux), application Android, interface web distante, plugins.

**Exclu** : macOS et iOS, contournement de DRM, résolution de captchas, comptes premium d'hébergeurs, synchronisation cloud, extracteurs maison pour hébergeurs de fichiers.

### Principes directeurs

1. **Un moteur, plusieurs clients.** Toute la logique de téléchargement vit dans le moteur ; les interfaces ne font qu'afficher et commander.
2. **La fiabilité passe avant les fonctionnalités.** Un fichier de 20 Go corrompu détruit la confiance ; les tests de reprise et d'intégrité bloquent toute livraison.
3. **Local par défaut.** Aucune télémétrie, aucun compte, aucun serveur central : les données restent sur la machine de l'utilisateur.
4. **Tout est scriptable.** Ce que fait l'interface passe par l'API publique, documentée et stable.
5. **Dégradation propre.** Un serveur sans `Range`, un disque plein ou un réseau instable ne perdent jamais de données : la tâche passe en pause avec une cause lisible.

### Vocabulaire

| Terme | Sens |
| --- | --- |
| Tâche | Un téléchargement complet, de l'ajout au fichier final |
| Segment | Plage d'octets d'un fichier, téléchargée par une connexion |
| Source | Une URL ou un pair capable de fournir le contenu ; plusieurs sources = miroirs |
| File | Ensemble ordonné de tâches avec ses propres limites |
| Moteur | Processus qui exécute les tâches et expose l'API |

## 2. Cahier des charges : exigences fonctionnelles

Le produit compte 41 exigences fonctionnelles, regroupées en neuf domaines. Priorités : **M** = indispensable à la v1, **S** = visé pour la v1 si le calendrier le permet, **C** = après la v1.

### 2.1 Téléchargement HTTP(S) et FTP

| ID | Exigence | Priorité |
| --- | --- | --- |
| F-01 | Téléchargement multi-connexions : de 1 à 32 connexions par tâche, valeur par défaut choisie automatiquement | M |
| F-02 | Segmentation dynamique : quand une connexion se libère, le plus gros segment restant est coupé en deux pour la réutiliser | M |
| F-03 | Reprise après coupure, redémarrage ou changement de réseau, via `Range` et validation par `ETag` ou `Last-Modified` | M |
| F-04 | Serveur sans `Range` : téléchargement en un seul flux, avec un avertissement clair que la reprise est impossible | M |
| F-05 | HTTP/2 et HTTP/3 lorsque le serveur les propose | S |
| F-06 | Redirections, cookies, en-têtes personnalisés, `User-Agent`, `Referer`, authentification Basic, Digest et Bearer | M |
| F-07 | Proxy HTTP et SOCKS5, avec détection des réglages du système | S |
| F-08 | FTP, FTPS et SFTP | S |
| F-09 | Nom du fichier tiré de `Content-Disposition` ou de l'URL, caractères interdits remplacés selon le système | M |
| F-10 | Écriture dans un fichier `.part`, renommé de façon atomique à la fin ; préallocation d'espace en option | M |

### 2.2 Intégrité

| ID | Exigence | Priorité |
| --- | --- | --- |
| F-11 | Vérification SHA-256, SHA-1 ou MD5, empreinte fournie par l'utilisateur ou lue dans un fichier voisin | M |
| F-12 | Re-téléchargement du seul segment fautif quand des empreintes par segment existent (torrent, Metalink) | S |
| F-13 | Si le fichier distant change pendant une reprise, la tâche se met en pause et demande : repartir de zéro ou conserver | M |

### 2.3 File d'attente et planification

| ID | Exigence | Priorité |
| --- | --- | --- |
| F-14 | Plusieurs files, priorités par tâche, nombre maximal de tâches simultanées par file | M |
| F-15 | Limite de débit globale, par file et par tâche, modifiable à chaud | M |
| F-16 | Planificateur par plages horaires et jours de la semaine, avec action de fin (veille, extinction, commande) | S |
| F-17 | Nouvelle tentative automatique avec délai exponentiel croissant et plafond configurable | M |
| F-18 | Contrôle de l'espace disque avant le départ et pendant le téléchargement ; pause avec cause explicite si le disque se remplit | M |

### 2.4 Torrent

| ID | Exigence | Priorité |
| --- | --- | --- |
| F-19 | Liens magnet et fichiers `.torrent`, DHT, PEX, trackers, sélection des fichiers à récupérer | S |
| F-20 | Limites de partage (ratio, durée) et arrêt automatique du partage | S |
| F-21 | Ouverture de port automatique (UPnP, NAT-PMP) et chiffrement du protocole | C |

### 2.5 Vidéo et flux

| ID | Exigence | Priorité |
| --- | --- | --- |
| F-22 | Détection des flux HLS et DASH, extraction via yt-dlp, choix de la qualité et du format | M |
| F-23 | Fusion audio et vidéo avec ffmpeg | S |
| F-24 | Téléchargement des sous-titres | C |
| F-25 | Mise à jour de yt-dlp indépendante de celle de l'application | M |

### 2.6 Multi-miroirs

| ID | Exigence | Priorité |
| --- | --- | --- |
| F-26 | Plusieurs URL pour un même fichier, segments répartis selon le débit mesuré de chaque source | S |
| F-27 | Lecture des fichiers Metalink (`.meta4`) | C |
| F-28 | Bascule automatique vers une autre source quand l'une échoue ou ralentit trop | S |

### 2.7 Analyse de liens

| ID | Exigence | Priorité |
| --- | --- | --- |
| F-29 | Coller un texte ou l'adresse d'une page et en extraire les fichiers, avec filtres par extension, taille et expression régulière | S |
| F-30 | Détection d'un lien dans le presse-papiers, désactivée par défaut | S |
| F-31 | Import d'une liste d'adresses (texte, CSV) | S |

### 2.8 Post-traitement et historique

| ID | Exigence | Priorité |
| --- | --- | --- |
| F-32 | Règles de rangement et de renommage par type de fichier, domaine ou expression régulière | S |
| F-33 | Extraction d'archives (zip intégré ; 7z et rar via un outil externe) | C |
| F-34 | Commande ou script externe à la fin d'une tâche, avec variables (chemin, taille, empreinte) | S |
| F-35 | Historique consultable, avec recherche et export CSV | M |

### 2.9 Clients et intégration

| ID | Exigence | Priorité |
| --- | --- | --- |
| F-36 | Application de bureau : fenêtre principale, icône de zone de notification, thème clair et sombre, interface traduisible | M |
| F-37 | Extension navigateur : capture automatique, menu contextuel, détection de vidéos, liste de sites exclus, taille minimale de capture | M |
| F-38 | Android : service au premier plan avec notification, partage depuis les autres applications, écriture via le sélecteur de stockage du système, option Wi-Fi seulement | M |
| F-39 | Interface web distante, protégée par jeton, accessible depuis un navigateur ou un téléphone | S |
| F-40 | Ligne de commande complète, mêmes capacités que l'interface graphique | M |
| F-41 | Plugins pour ajouter des hébergeurs ou des actions, exécutés dans un bac à sable | S |

## 3. Exigences non fonctionnelles

Quinze exigences encadrent la qualité du produit. Les chiffres sont des cibles de départ, à confirmer par des mesures sur du matériel réel dès le premier prototype.

| ID | Domaine | Exigence | Cible |
| --- | --- | --- | --- |
| N-01 | Performance | Coût processeur du transfert | Porte (bloquante) : temps processeur par octet transféré, celui du processus plus celui du vidage sur disque de ce qu'il a laissé en mémoire, au plus égal à celui d'aria2 (seuil 1,0), à nombre de connexions égal, sur la même machine, avec le même fichier et le même serveur ; passages alternés entre les outils, médiane retenue. Le vidage compte pour tous les outils parce qu'`auro` paie le sien dans son processus (règle 1 de la section 5.6) et qu'aria2 le laisse au système. Le temps processeur du seul processus et celui de toute la machine sont relevés à titre indicatif, non bloquant : il varie de ±30 % d'un passage à l'autre sur une machine virtuelle de poste de travail (`docs/MESURES.md`, 2026-11-06). Une validation périodique sur une machine Linux native ou en intégration continue dédiée est prévue. Objectif indicatif (non bloquant) : saturer 1 Gbit/s avec moins de 15 % d'un cœur sur la machine de référence nommée dans le relevé de performance |
| N-02 | Performance | Démarrage du moteur | Prêt à recevoir des commandes en moins d'une seconde sur ordinateur |
| N-03 | Mémoire | Consommation du moteur | Moins de 50 Mo au repos ; moins de 150 Mo avec 100 tâches actives |
| N-04 | Fiabilité | Reprise après arrêt brutal (coupure de courant, arrêt forcé) | Perte limitée aux octets d'un segment en cours ; aucun segment terminé perdu |
| N-05 | Fiabilité | Cohérence de l'état | Mode WAL de SQLite, point de contrôle au plus toutes les 2 secondes |
| N-06 | Sécurité | Exposition de l'API | Écoute sur l'interface locale uniquement par défaut ; jeton obligatoire pour tout accès distant |
| N-07 | Sécurité | Connexions chiffrées | Validation stricte des certificats ; pas d'option « ignorer » sans confirmation explicite par tâche |
| N-08 | Portabilité | Plateformes | Windows 10 et 11 (x86\_64, ARM64) ; Linux (x86\_64, aarch64) ; Android 8.0 et plus |
| N-09 | Accessibilité | Utilisation sans souris et avec lecteur d'écran | Conformité visée WCAG 2.2 niveau AA pour l'interface graphique |
| N-10 | Langues | Interface traduisible | Textes externalisés, UTF-8 partout ; français et anglais livrés en v1 |
| N-11 | Licence | Logiciel libre | GPL-3.0 ou ultérieure proposée pour le moteur et les clients (à confirmer) |
| N-12 | Observabilité | Journaux | Journaux structurés avec niveaux et rotation ; export de diagnostic sans adresses ni identifiants |
| N-13 | Maintenabilité | Qualité du code | Intégration continue sur les trois plateformes ; tests de reprise et d'intégrité bloquants |
| N-14 | Mises à jour | Distribution | Mise à jour signée ; sur Android, mise à jour par les boutiques et F-Droid, pas de mise à jour silencieuse |
| N-15 | Vie privée | Données personnelles | Aucune télémétrie ; historique stocké localement et effaçable |

## 4. Architecture

Le moteur est un processus autonome qui exécute les téléchargements et expose une API JSON-RPC ; les interfaces ne contiennent aucune logique de téléchargement. Les choix de cette section sont des propositions à valider par de petits prototypes, surtout pour l'interface et pour Android.

### 4.1 Vue d'ensemble

Le schéma montre les trois couches : les clients, l'API, puis le moteur et ce qu'il utilise.

&#91;embedded content: architecture · 5 clients, 1 API, 8 modules du moteur\]

Les clients envoient des commandes à l'API et affichent les notifications ; le moteur continue de travailler même si aucune interface n'est ouverte.

### 4.2 Modules du moteur

| Module | Rôle |
| --- | --- |
| `rpc` | Serveur JSON-RPC, transports, authentification, abonnements aux événements |
| `scheduler` | Files, priorités, limites de débit, planificateur horaire, nouvelles tentatives |
| `segmenter` | Découpage et redécoupage des segments, répartition sur les sources, nombre de connexions |
| `net` | Clients HTTP(S) et FTP, TLS, proxies, HTTP/2 et HTTP/3 |
| `io` | Écriture positionnée, préallocation, empreintes ; abstraction du stockage (fichier local ou sélecteur Android) |
| `store` | Base SQLite, migrations, points de contrôle |
| `transfer` | Exécution d'une tâche : préparation, connexions, points de contrôle, reprise et fin ; relie `net`, `io` et `store` |
| `torrent` | Interface interne et bibliothèque de torrent derrière elle |
| `media` | Pilotage de yt-dlp et ffmpeg comme processus externes |
| `postproc` | Règles de rangement, archives, commandes de fin de tâche |
| `plugins` | Exécution des plugins dans un bac à sable |

### 4.3 Choix techniques proposés

| Domaine | Choix | Raison |
| --- | --- | --- |
| Langage du moteur | Rust avec `tokio` | Un binaire sans environnement d'exécution, sûreté mémoire, compilation pour Windows, Linux et Android |
| HTTP et TLS | `reqwest` avec `rustls`, certificats racines du système (`rustls-platform-verifier`) | Même comportement TLS sur les trois plateformes, sans dépendre des bibliothèques TLS du système ; HTTP/3 ensuite (priorité S) |
| Base de données | SQLite via `rusqlite` | Embarquée, transactionnelle, adaptée à la reprise |
| Torrent | Bibliothèque Rust ou `libtorrent`, derrière une interface interne | À trancher par un prototype ; l'interface permet d'en changer |
| Vidéo | yt-dlp et ffmpeg en processus externes | Extracteurs maintenus par d'autres, licences séparées, mises à jour indépendantes (F-25) |
| Interface de bureau | Tauri 2 (choisi le 2026-11-06, après deux prototypes comparés sous Windows et Linux : `experiments/desktop/README.md`) | Une seule page web pour le bureau et l'interface distante (F-39), servie par l'hôte de chacune ; client du moteur en Rust, repris du moteur. Qt Quick était plus léger (mémoire, processeur fenêtre ouverte, démarrage), mais aurait demandé une seconde interface |
| Interface Android | Kotlin avec Jetpack Compose, moteur chargé comme bibliothèque native | Partage, services et stockage sont des API Android natives |
| Extension | WebExtension (Chrome, Edge, Firefox) et programme relais natif | Seule voie propre vers le moteur local |
| Plugins | WebAssembly | Bac à sable, plusieurs langages possibles |

### 4.4 Android

- Le moteur est compilé comme bibliothèque native et appelé directement, avec les mêmes messages JSON-RPC que partout ailleurs.
- Un service au premier plan héberge le moteur et affiche une notification avec la progression, la pause et la reprise.
- Le menu « Partager » d'Android et les liens ouverts depuis d'autres applications appellent `task.add`.
- Le dossier de destination est choisi par l'utilisateur dans le sélecteur de stockage, avec une permission conservée entre les sessions.
- Options : Wi-Fi seulement, respect de l'économiseur de batterie, reprise au redémarrage de l'appareil si l'utilisateur l'autorise.
- L'application peut aussi servir de simple télécommande d'un moteur distant (un NAS, par exemple), sans moteur local.
- Les contraintes d'Android sur les tâches longues sont le principal risque de cette plateforme (section 9.2).

### 4.5 Extension navigateur

1. L'utilisateur lance un téléchargement ; l'extension le détecte, l'annule dans le navigateur et relève l'adresse, les en-têtes utiles et les cookies nécessaires.
2. Elle transmet le tout au programme relais, installé avec l'application et déclaré au navigateur.
3. Le relais appelle `task.add` sur le socket local avec un jeton de portée `add` seulement (section 6.9).
4. Le moteur répond ; l'extension affiche la confirmation. Si le moteur n'est pas lancé, le relais le démarre.

Les cookies sont rangés dans le coffre du système, jamais dans la base (section 5.1).

### 4.6 Exécution et distribution

- **Bureau** : l'application démarre le moteur s'il n'est pas lancé et le laisse tourner dans la zone de notification ; démarrage avec la session en option.
- **Moteur** : binaire `aurod` (crate `crates/daemon`), sans interface. L'application de bureau le lance s'il ne tourne pas ; `auro` est le client en ligne de commande, et le lance de même, détaché de la console, avec son journal (`aurod.log`) à côté de sa base. `auro direct` exécute une tâche avec la bibliothèque du moteur, sans `aurod`, pour les bancs et le dépannage.
- **Sans interface** : l'option `--headless` lance le moteur seul, pour un NAS ou un serveur, avec une image Docker et une unité `systemd` utilisateur. L'interface web est servie par le moteur lui-même.
- **Windows** : installateur signé et version portable.
- **Linux** : AppImage et paquet `.deb` ; Flatpak à étudier, car son bac à sable complique la messagerie native avec le navigateur.
- **Android** : F-Droid et APK signé.

## 5. Modèle de données

Tout l'état durable tient dans une base SQLite unique, dont le moteur est le seul écrivain. Elle contient 14 tables ; c'est elle qui rend possible la reprise après un arrêt brutal (N-04, N-05).

### 5.1 Conventions

- Mode WAL, clés étrangères activées, `synchronous = NORMAL`.
- Identifiants : UUID v7 en texte (triables par date de création), générés par le moteur.
- Dates : millisecondes Unix en UTC, entier.
- Tailles et débits : octets et octets par seconde, entiers de 64 bits.
- Les chemins sont stockés tels que le système les fournit ; sur Android, `dest_dir` peut être une URI du sélecteur de stockage.
- Aucun secret en base : mots de passe, jetons et cookies sensibles sont rangés dans le coffre du système (Gestionnaire d'identification sous Windows, Secret Service sous Linux, Keystore sous Android) ; la base ne garde qu'une référence.

### 5.2 Configuration, files et planification

```sql
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA synchronous = NORMAL;

CREATE TABLE schema_migrations (
  version    INTEGER PRIMARY KEY,
  applied_at INTEGER NOT NULL
);

CREATE TABLE settings (
  key        TEXT PRIMARY KEY,
  value_json TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE schedules (
  id         TEXT PRIMARY KEY,
  name       TEXT NOT NULL,
  rules_json TEXT NOT NULL,   -- plages horaires, jours, action de fin
  enabled    INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE queues (
  id              TEXT PRIMARY KEY,
  name            TEXT NOT NULL UNIQUE,
  position        INTEGER NOT NULL,
  max_active      INTEGER NOT NULL DEFAULT 3 CHECK (max_active >= 1),
  speed_limit_bps INTEGER CHECK (speed_limit_bps IS NULL OR speed_limit_bps > 0),
  schedule_id     TEXT REFERENCES schedules(id) ON DELETE SET NULL,
  created_at      INTEGER NOT NULL
);

CREATE TABLE rules (            -- rangement et renommage
  id           TEXT PRIMARY KEY,
  position     INTEGER NOT NULL,
  enabled      INTEGER NOT NULL DEFAULT 1,
  match_json   TEXT NOT NULL,   -- type, domaine, expression régulière
  actions_json TEXT NOT NULL    -- dossier cible, modèle de nom, commande
);
```

### 5.3 Tâches, sources et segments

```sql
CREATE TABLE tasks (
  id              TEXT PRIMARY KEY,
  queue_id        TEXT NOT NULL REFERENCES queues(id),
  kind            TEXT NOT NULL CHECK (kind IN ('http','ftp','torrent','media')),
  state           TEXT NOT NULL CHECK (state IN ('queued','active','paused','retry_wait',
                    'verifying','post_processing','seeding','completed','failed','cancelled')),
  priority        INTEGER NOT NULL DEFAULT 0,
  position        INTEGER NOT NULL,
  display_name    TEXT NOT NULL,
  dest_dir        TEXT NOT NULL,
  file_name       TEXT,
  total_bytes     INTEGER,            -- NULL tant que la taille est inconnue
  done_bytes      INTEGER NOT NULL DEFAULT 0,
  max_connections INTEGER CHECK (max_connections BETWEEN 1 AND 32),  -- NULL = automatique
  speed_limit_bps INTEGER,
  checksum_algo   TEXT CHECK (checksum_algo IN ('sha256','sha1','md5')),
  checksum_value  TEXT,
  pause_reason    TEXT CHECK (pause_reason IN ('user','schedule','disk_full','network','file_changed')),
  error_code      TEXT,
  error_detail    TEXT,
  retry_count     INTEGER NOT NULL DEFAULT 0,
  next_retry_at   INTEGER,
  options_json    TEXT NOT NULL DEFAULT '{}',
  created_at      INTEGER NOT NULL,
  started_at      INTEGER,
  finished_at     INTEGER,
  updated_at      INTEGER NOT NULL
);

CREATE TABLE sources (
  id             TEXT PRIMARY KEY,
  task_id        TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  url            TEXT NOT NULL,
  rank           INTEGER NOT NULL DEFAULT 0,   -- ordre de préférence
  state          TEXT NOT NULL DEFAULT 'unknown' CHECK (state IN ('unknown','ok','slow','failed')),
  accepts_ranges INTEGER CHECK (accepts_ranges IN (0,1)),
  etag           TEXT,
  last_modified  TEXT,
  content_length INTEGER,
  avg_bps        INTEGER,
  fail_count     INTEGER NOT NULL DEFAULT 0,
  last_error     TEXT,
  UNIQUE (task_id, url)
);

CREATE TABLE segments (
  task_id       TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  idx           INTEGER NOT NULL,             -- croît à chaque redécoupage, jamais réutilisé
  start_offset  INTEGER NOT NULL,
  end_offset    INTEGER NOT NULL,             -- inclus
  written_bytes INTEGER NOT NULL DEFAULT 0,
  state         TEXT NOT NULL CHECK (state IN ('pending','active','done','failed')),
  source_id     TEXT REFERENCES sources(id) ON DELETE SET NULL,
  attempts      INTEGER NOT NULL DEFAULT 0,
  hash          BLOB,                         -- empreinte du segment si elle est connue
  PRIMARY KEY (task_id, idx),
  CHECK (end_offset >= start_offset),
  CHECK (written_bytes <= end_offset - start_offset + 1)
);

CREATE TABLE task_headers (
  task_id    TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  name       TEXT NOT NULL,
  value      TEXT,
  secret_ref TEXT,                            -- référence au coffre du système
  PRIMARY KEY (task_id, name),
  CHECK ((value IS NULL) <> (secret_ref IS NULL))
);
```

### 5.4 Torrent, vidéo, historique et journal

```sql
CREATE TABLE task_files (          -- tâches à plusieurs fichiers (torrent, lot)
  task_id    TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  file_index INTEGER NOT NULL,
  path       TEXT NOT NULL,
  length     INTEGER NOT NULL,
  selected   INTEGER NOT NULL DEFAULT 1,
  done_bytes INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (task_id, file_index)
);

CREATE TABLE torrents (
  task_id           TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
  info_hash         TEXT NOT NULL,
  magnet            TEXT,
  metainfo          BLOB,
  uploaded_bytes    INTEGER NOT NULL DEFAULT 0,
  ratio_limit       REAL,
  seed_time_limit_s INTEGER
);

CREATE TABLE media_jobs (
  task_id         TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
  page_url        TEXT NOT NULL,
  extractor       TEXT,
  format_selector TEXT NOT NULL,
  merge_container TEXT,
  subtitles_json  TEXT
);

CREATE TABLE history (              -- survit à la suppression de la tâche
  id          TEXT PRIMARY KEY,
  task_id     TEXT,
  display_name TEXT NOT NULL,
  final_path  TEXT,
  total_bytes INTEGER,
  source_host TEXT,
  final_state TEXT NOT NULL CHECK (final_state IN ('completed','failed','cancelled')),
  sha256      TEXT,
  finished_at INTEGER NOT NULL
);

CREATE TABLE event_log (
  id      INTEGER PRIMARY KEY AUTOINCREMENT,
  task_id TEXT,
  ts      INTEGER NOT NULL,
  level   TEXT NOT NULL CHECK (level IN ('debug','info','warn','error')),
  code    TEXT NOT NULL,
  message TEXT NOT NULL
);

CREATE INDEX idx_tasks_queue_state ON tasks (queue_id, state, priority DESC, position);
CREATE INDEX idx_segments_state    ON segments (task_id, state);
CREATE INDEX idx_sources_task      ON sources (task_id, rank);
CREATE INDEX idx_history_finished  ON history (finished_at DESC);
CREATE INDEX idx_event_log_task    ON event_log (task_id, ts);
```

### 5.5 États d'une tâche

| État | Sens | Passe à |
| --- | --- | --- |
| `queued` | Ajoutée, attend une place dans sa file | `active`, `paused`, `cancelled` |
| `active` | Des connexions transfèrent des données | `paused`, `retry_wait`, `verifying`, `seeding`, `failed`, `cancelled` |
| `paused` | Arrêtée, avec une cause dans `pause_reason` | `queued`, `cancelled` |
| `retry_wait` | Échec temporaire, nouvelle tentative à `next_retry_at` | `active`, `paused`, `failed`, `cancelled` |
| `verifying` | Vérification de l'empreinte du fichier complet | `post_processing`, `completed`, `failed` |
| `post_processing` | Rangement, renommage, extraction, commande externe | `completed`, `failed` |
| `seeding` | Torrent terminé, partage en cours | `completed`, `cancelled` |
| `completed` | Fichier final en place | état terminal |
| `failed` | Échec définitif, cause dans `error_code` | `queued` (relance manuelle) |
| `cancelled` | Annulée par l'utilisateur | état terminal |

### 5.6 Règles de cohérence

1. **Ordre d'écriture.** Les octets sont d'abord écrits et vidés sur disque (`fdatasync`), puis seulement le compteur `written_bytes` est enregistré. Après un arrêt brutal, la base n'annonce jamais plus que ce qui est réellement sur le disque.
2. **Couverture exacte.** Les segments d'une tâche couvrent `[0, total_bytes − 1]` sans trou ni chevauchement. Le moteur le vérifie au démarrage et après chaque redécoupage, dans la même transaction que la modification. Une tâche de taille nulle n'a aucun segment ; une tâche de taille inconnue (`total_bytes` NULL) est téléchargée en un seul flux, sans segments, et la règle ne s'applique pas. La taille d'une tâche et ses segments initiaux sont enregistrés dans la même transaction : une taille connue implique des segments, et un flux unique de taille connue en a un seul.
3. **Compteur dérivé.** `tasks.done_bytes` est la somme des `written_bytes` ; il est recalculé au chargement, la somme faisant foi.
4. **Fin de vie.** À la fin d'une tâche, la ligne `history` est écrite avant la purge des segments et des sources, dans une seule transaction.
5. **Migrations.** Numérotées, appliquées dans l'ordre et dans une transaction, après copie du fichier de base ; une base plus récente que le moteur empêche le démarrage au lieu d'être modifiée.
6. **Rétention.** `event_log` est purgé au-delà de 30 jours ou de 10 000 lignes ; la durée de l'historique est un réglage.

## 6. Protocole du moteur

Le moteur parle JSON-RPC 2.0, quel que soit le client ou le transport : application de bureau, ligne de commande, extension, interface web ou application Android. Un client qui sait appeler `task.add` fonctionne donc partout, et l'API publique est la même que celle utilisée par l'interface officielle (principe 4).

### 6.1 Principes

- **JSON-RPC 2.0** : requêtes avec `id`, notifications sans `id`, envois groupés (batch) acceptés.
- **Paramètres nommés** : `params` est toujours un objet, jamais un tableau.
- **Noms de méthodes** : `espace.verbe`, en minuscules et casse mixte (`task.add`, `queue.update`).
- **Dates et tailles** : millisecondes Unix en UTC, octets, octets par seconde, tous en entiers.
- **Évolution** : ajouter un champ optionnel ou une méthode ne casse rien ; supprimer ou renommer exige une nouvelle version majeure du protocole.
- **Idempotence** : `task.add` accepte une `idempotencyKey` ; rejouer le même appel ne crée pas de doublon (utile pour l'extension qui réessaie).

### 6.2 Transports et authentification

| Transport | Usage | Authentification |
| --- | --- | --- |
| Socket Unix (Linux) ou canal nommé (Windows) | Application de bureau et CLI sur la même machine | Droits du système de fichiers : seul l'utilisateur propriétaire y accède |
| Appel direct dans le processus (FFI) | Android, où le moteur vit dans l'application | Aucune : même processus |
| WebSocket sur `127.0.0.1` | Interface web, outils de développement | Jeton porteur dans l'en-tête `Authorization` ou dans le premier message |
| WebSocket sur le réseau, en TLS | Moteur distant (NAS, serveur) | Jeton porteur, TLS obligatoire dès que l'écoute n'est pas locale |
| HTTP `POST /rpc` | Scripts simples sans notifications | Jeton porteur |
| Messagerie native du navigateur | Extension, via un petit programme relais | Jeton à portée limitée (voir 6.9) |

Deux protections s'ajoutent pour toute écoute TCP : le moteur vérifie les en-têtes `Host` et `Origin` et refuse ceux qu'il ne reconnaît pas, ce qui bloque les attaques depuis une page web quelconque visant le port local ; et il n'accepte jamais de connexion sans jeton, même depuis `127.0.0.1`.

Transport local, tel que mis en œuvre (phase 2) :
- **Découpage des messages** : un message JSON par ligne, dans les deux sens, terminé par `\n`. Un message de plus de 1 Mio ferme la connexion. Le WebSocket portera les mêmes messages, un par trame.
- **Windows** : canal nommé `\\.\pipe\aurodownloader-<utilisateur>`. Son descripteur de sécurité n'autorise que l'identifiant (SID) de l'utilisateur qui a lancé le moteur, sans droit hérité. Le canal refuse les clients distants. Un second moteur ne peut pas prendre le nom tant que le premier tourne.
- **Linux** : socket `$XDG_RUNTIME_DIR/aurodownloader/aurod.sock`, ou `/tmp/aurodownloader-<utilisateur>/aurod.sock` sans `XDG_RUNTIME_DIR`. Le dossier est en `0700` et le socket en `0600`. Un socket laissé par un moteur arrêté brutalement est remplacé ; un socket qui répond empêche un second moteur de démarrer.
- **Processus** : le binaire `aurod` sert ce point d'accès. Il s'arrête proprement sur Ctrl-C, sur SIGTERM ou par `engine.shutdown`.

### 6.3 Ouverture de session

Le premier appel d'un client est `engine.hello`. Il annonce la plage de versions du protocole qu'il comprend ; le moteur répond avec la version retenue, ses capacités et le numéro de révision courant.

```json
--> {"jsonrpc":"2.0","id":1,"method":"engine.hello","params":{
      "client":{"name":"desktop","version":"1.0.0"},
      "protocol":{"min":"1.0","max":"1.2"}}}
<-- {"jsonrpc":"2.0","id":1,"result":{
      "engine":{"version":"1.0.3","platform":"linux"},
      "protocol":"1.1",
      "capabilities":["http3","torrent","media","linkscan"],
      "rev":1042}}
```

Aucune plage commune : le moteur répond par l'erreur `PROTOCOL_UNSUPPORTED` et ferme la connexion.

### 6.4 Méthodes

| Méthode | Paramètres principaux | Résultat |
| --- | --- | --- |
| `engine.hello` | `client`, `protocol` | Version retenue, capacités, `rev` |
| `engine.stats` | aucun | Débit global, tâches actives, espace disque |
| `engine.shutdown` | `graceful` | Arrêt après sauvegarde de l'état |
| `task.add` | `sources[]`, `queueId`, `destDir`, `fileName`, `maxConnections`, `speedLimitBps`, `checksum`, `headers`, `startPaused`, `idempotencyKey` | `taskId` |
| `task.addBatch` | `items[]` de paramètres `task.add` | `taskIds[]` |
| `task.get` | `taskId`, `include` (`segments`, `sources`, `files`) | Tâche complète |
| `task.list` | `queueId`, `states[]`, `limit`, `cursor` | `items[]`, `nextCursor`, `rev` |
| `task.pause` / `task.resume` | `taskIds[]` | Liste des tâches modifiées |
| `task.cancel` | `taskIds[]`, `deleteFiles` | Liste des tâches modifiées |
| `task.remove` | `taskIds[]`, `deleteFiles` | Nombre de tâches retirées |
| `task.update` | `taskId`, `maxConnections`, `speedLimitBps`, `destDir`, `fileName`, `checksum` | Tâche modifiée |
| `task.move` | `taskIds[]`, `queueId`, `position`, `priority` | Liste des tâches modifiées |
| `task.addSource` | `taskId`, `url`, `rank` | `sourceId` |
| `task.setFiles` | `taskId`, `selected[]` (torrent) | Tâche modifiée |
| `queue.list` / `queue.create` / `queue.update` / `queue.delete` | `name`, `maxActive`, `speedLimitBps`, `scheduleId` | File ou liste de files |
| `limits.get` / `limits.set` | `downloadBps`, `uploadBps` | Limites globales |
| `schedule.list` / `schedule.set` | `rules` | Planification |
| `linkscan.run` | `text` ou `pageUrl`, `filters` | `links[]` avec nom, taille, type |
| `media.probe` | `pageUrl` | `formats[]` disponibles |
| `history.list` / `history.clear` | `query`, `before`, `limit` | Entrées d'historique |
| `settings.get` / `settings.set` | `keys[]` ou `values` | Réglages |
| `events.subscribe` / `events.unsubscribe` | `topics[]` (`tasks`, `queues`, `progress`), `progressIntervalMs` | Abonnement actif et `rev` au moment de l'abonnement |
| `sync.snapshot` | aucun | État complet et `rev` |

### 6.5 Notifications du moteur

Un client abonné reçoit ces notifications, sans y répondre. Chacune porte la révision `rev` du moteur au moment de l'événement.

| Notification | Contenu |
| --- | --- |
| `task.added` | Tâche complète |
| `task.updated` | `taskId` et seulement les champs modifiés |
| `task.stateChanged` | `taskId`, ancien et nouvel état, `pauseReason` ou `errorCode` |
| `task.progress` | Lot de `{taskId, doneBytes, totalBytes, bps, connections, etaS}`, regroupé selon `progressIntervalMs` (500 ms par défaut) |
| `task.removed` | `taskId` |
| `queue.updated` | File modifiée |
| `engine.stats` | Débit global, toutes les secondes |
| `engine.warning` | `code`, `message` (disque presque plein, nouvelle version de yt-dlp disponible ; `EVENTS_LOST` : client trop lent, des notifications ont été perdues, appeler `sync.snapshot`) |

Sujets d'abonnement : `tasks` (`task.added`, `task.stateChanged`, `task.updated`, `task.removed`), `queues` (`queue.updated`), `progress` (`task.progress`). Dans `task.progress`, `doneBytes` compte les octets reçus, pas forcément vidés sur disque. Le moteur mesure l'avancement toutes les 500 ms : un intervalle plus court n'apporte rien de plus.

État de la mise en œuvre (phase 2) :
- **Méthodes servies** : `engine.hello`, `engine.shutdown`, `task.add`, `task.get`, `task.list`, `task.pause`, `task.resume`, `task.cancel`, `task.remove`, `task.update`, `queue.list`, `queue.create`, `queue.update`, `limits.get`, `limits.set`, `events.subscribe`, `events.unsubscribe` et `sync.snapshot`. Les autres répondent « méthode inconnue » jusqu'à leur arrivée.
- **Paramètres refusés** : un paramètre prévu mais pas encore pris en charge est refusé par `CAPABILITY_MISSING`, et non ignoré en silence. Cela concerne :
  - dans `task.add` : `fileName`, `checksum`, `headers`, `idempotencyKey` et plusieurs sources ;
  - dans `task.update` : `maxConnections`, `destDir`, `fileName` et `checksum` (seul `speedLimitBps` y est pris en charge) ;
  - pour les files : `scheduleId` ;
  - dans `limits.set` : `uploadBps`, le débit montant ne servant qu'aux torrents (`limits.get` le renvoie à `null`).
- **Limites de débit** (`speedLimitBps`, `downloadBps`) : un entier d'au moins 1, en octets par seconde.
  - Dans `task.update`, `queue.update` et `limits.set`, `null` retire la limite et un paramètre absent la laisse telle quelle.
  - Une nouvelle limite s'applique aussitôt aux tâches en cours (section 7.6).
  - `task.update` annonce `task.updated`, avec `taskId` et les seuls champs modifiés.
  - Un changement de la limite globale n'est pas annoncé : aucune notification n'est prévue pour lui.
- **Nom et taille d'une tâche** (depuis le 2026-11-07) : `task.added` les ignore encore,
  la préparation les apprend (section 7.1). Après chaque préparation, le moteur annonce
  `task.updated` avec `fileName`, `displayName` et `totalBytes`, et une nouvelle révision.
  L'annonce peut suivre la fin d'une tâche rapide. Un client qui ouvre le fichier d'une
  tâche prend ce `fileName`, qui peut différer du nom de l'adresse (« nom (1).bin »).
- **Retrait** (`task.remove`, depuis le 2026-11-07) :
  - seules les tâches finies (`completed`, `failed`, `cancelled`) se retirent ; les autres
    sont refusées par `INVALID_STATE` et s'annulent d'abord ;
  - la ligne `history` reste ; une tâche en échec ou annulée, qui n'en a pas encore, reçoit
    la sienne au retrait, dans la même transaction (section 5.6, règle 4) ;
  - avec `deleteFiles`, le fichier est supprimé avant la tâche : le fichier final d'une
    tâche terminée, le `.part` des autres. Si la suppression échoue, la tâche reste ;
  - chaque retrait est annoncé par `task.removed` (sujet `tasks`), avec une nouvelle
    révision ;
  - le résultat porte `removed`, le nombre de tâches retirées, en plus de `taskIds` et
    `errors`, comme les autres actions groupées.
- **Résultat d'une action groupée** : `task.pause`, `task.resume` et `task.cancel` renvoient `taskIds[]`, les tâches acceptées, et `errors[]`, chaque refus avec `taskId`, `code` et `message`. Une tâche refusée n'empêche pas les autres.

### 6.6 Exemple d'échange

```json
--> {"jsonrpc":"2.0","id":7,"method":"task.add","params":{
      "sources":[{"url":"https://example.org/image.iso"}],
      "queueId":"018f3c2e-7a10-7b4c-9d21-5f1e8a90b3c4",
      "destDir":"/home/alice/Téléchargements",
      "checksum":{"algo":"sha256","value":"9f86d0…"},
      "idempotencyKey":"ext-4c1a9e"}}
<-- {"jsonrpc":"2.0","id":7,"result":{"taskId":"018f3c31-02d4-7e88-a6b0-3c7d9e12f450"}}
<-- {"jsonrpc":"2.0","method":"task.stateChanged","params":{
      "rev":1043,"taskId":"018f3c31-02d4-7e88-a6b0-3c7d9e12f450",
      "from":"queued","to":"active"}}
<-- {"jsonrpc":"2.0","method":"task.progress","params":{"rev":1043,"items":[
      {"taskId":"018f3c31-02d4-7e88-a6b0-3c7d9e12f450","doneBytes":52428800,
       "totalBytes":1073741824,"bps":38000000,"connections":8,"etaS":27}]}}
```

### 6.7 Erreurs de protocole

Les erreurs suivent JSON-RPC : `code` numérique, `message` lisible, et `data.code` en texte stable sur lequel les clients peuvent s'appuyer.

| Code | `data.code` | Cas |
| --- | --- | --- |
| -32700 à -32600 | (standard JSON-RPC) | Message mal formé, méthode inconnue, paramètres invalides |
| -32001 | `UNAUTHORIZED` | Jeton absent, invalide ou sans la portée requise |
| -32002 | `PROTOCOL_UNSUPPORTED` | Aucune version de protocole commune |
| -32010 | `TASK_NOT_FOUND` | Identifiant de tâche inconnu |
| -32011 | `INVALID_STATE` | Transition interdite depuis l'état actuel de la tâche |
| -32012 | `QUEUE_NOT_FOUND` | Identifiant de file inconnu |
| -32020 | `URL_INVALID` | Adresse non reconnue ou schéma non pris en charge |
| -32021 | `DEST_NOT_WRITABLE` | Dossier de destination inaccessible en écriture |
| -32022 | `INSUFFICIENT_SPACE` | Espace disque insuffisant pour la taille annoncée |
| -32030 | `CAPABILITY_MISSING` | Fonction non disponible dans ce moteur (torrent, vidéo) |

`INSUFFICIENT_SPACE` n'est pas encore renvoyé : `task.add` ne connaît pas la taille, que seule la préparation apprend (section 7.1). Un manque de place met la tâche en pause, cause `disk_full`, code `DISK_FULL` (sections 6.8 et 7.6 bis).

### 6.8 Codes d'erreur d'une tâche

Quand une tâche échoue, `error_code` indique la cause. Le moteur décide seul s'il réessaie, selon la colonne « Réessai ».

| `error_code` | Réessai | Signification |
| --- | --- | --- |
| `NET_UNREACHABLE` | oui | Réseau coupé ou hôte injoignable |
| `NET_TIMEOUT` | oui | Délai dépassé |
| `HTTP_STATUS` | selon le statut | Réessai pour 408, 429 et 5xx ; échec définitif pour 401, 403, 404 et 410 |
| `TLS_INVALID_CERT` | non | Certificat refusé |
| `TLS_UNAVAILABLE` | non | HTTPS impossible : la vérification des certificats n'est pas initialisée (sous Android, tant que l'application ne l'a pas fait, section 7.5) |
| `RANGE_UNSUPPORTED` | non bloquant | Bascule en un seul flux, la reprise sera impossible |
| `FILE_CHANGED` | non | Le fichier distant a changé ; la tâche attend la décision de l'utilisateur |
| `DISK_FULL` | après libération | Plus d'espace ; la tâche reprend quand l'espace revient |
| `DEST_PERMISSION` | non | Écriture refusée dans le dossier |
| `DISK_IO` | non | Autre erreur de lecture, d'écriture ou de renommage sur le disque |
| `CHECKSUM_MISMATCH` | non | L'empreinte du fichier final diffère de celle attendue |
| `INTERNAL` | non | Erreur du moteur lui-même (base illisible, incohérence, panique d'une exécution) ; détail dans `error_detail`. La tâche ne repart qu'à la demande de l'utilisateur |
| `EXTRACTOR_FAILED` | non | yt-dlp n'a pas pu résoudre la page |

### 6.9 Portées des jetons

| Portée | Autorise | Typiquement donnée à |
| --- | --- | --- |
| `read` | Méthodes `*.list`, `*.get`, `engine.stats`, abonnements | Un tableau de bord en lecture seule |
| `add` | `task.add`, `task.addBatch`, `media.probe`, `linkscan.run` | L'extension navigateur, un partage Android |
| `control` | Tout ce que fait `add`, plus `task.*` et `queue.*` | L'application de bureau, la CLI, l'interface web |
| `admin` | Tout, dont `settings.set`, `engine.shutdown` et la gestion des jetons | Le propriétaire de la machine |

### 6.10 Garanties de livraison

- Les messages d'une même connexion arrivent dans l'ordre.
- Les changements d'état ne sont jamais abandonnés ; seules les notifications `task.progress` peuvent être regroupées ou sautées si le client est lent.
- La révision `rev` augmente de 1 à chaque changement d'état ; les notifications de progression portent la révision courante sans l'augmenter, si bien qu'en sauter une ne crée aucun trou. Un client qui constate un trou dans les révisions, ou qui se reconnecte, appelle `sync.snapshot` et repart de cet état complet. La révision repart de 0 à chaque démarrage du moteur.
- Pour ne rien manquer, un client s'abonne d'abord, puis appelle `sync.snapshot`. Il ignore ensuite les notifications de révision inférieure ou égale à celle de l'instantané.
- Le moteur ne suppose jamais qu'un client est présent : les téléchargements continuent sans interface ouverte.

## 7. Algorithmes clés

Les mécanismes ci-dessous décident de la vitesse et de la fiabilité du moteur. Les valeurs données sont des points de départ à régler par benchmark, pas des vérités.

### 7.1 Préparation d'une tâche

1. Le moteur envoie une requête `HEAD`. Si elle annonce `Accept-Ranges: bytes` et une taille (`Content-Length`), il s'arrête là ; `Accept-Ranges: none` suffit aussi, pour un seul flux. Sinon, ou si le serveur refuse le `HEAD`, il envoie un `GET` avec `Range: bytes=0-0` et lit `Content-Range` dans la réponse, sans lire le corps.
2. Une réponse `206` avec `Content-Range` prouve que `Range` est accepté, et donne la taille totale. Une réponse `200` signifie que le serveur l'ignore : tâche en un seul flux (F-04). Une réponse `416` avec `Content-Range: bytes */0` désigne un fichier vide. Un corps compressé (`Content-Encoding` autre que `identity`) rend la taille inconnue : un seul flux. Chaque requête de segment vérifie de toute façon son `206`. Une redirection n'est suivie que vers HTTP ou HTTPS, dix fois au plus ; sinon la tâche échoue avec `HTTP_STATUS`, sans nouvelle tentative.
3. Le moteur mémorise `ETag`, `Last-Modified` et la taille dans `sources`, pour valider la reprise.
4. Les requêtes par plage demandent `Accept-Encoding: identity`, car une compression ferait dérailler les décalages d'octets.
5. Il crée le fichier `.part`, vérifie l'espace libre (F-18, section 7.6 bis) et découpe la plage `[0, taille − 1]` en segments initiaux.

Nom du fichier (depuis le 2026-11-06) : le moteur prend un nom libre dans le dossier de destination, puis crée aussitôt son `.part`, vide, avant d'enregistrer le nom. La création refuse un fichier existant ; si une autre tâche vient de prendre le même nom, il essaie le suivant (`nom (1).ext`, `nom (2).ext`…). Est libre un nom dont ni le fichier ni le `.part` n'existent, et qu'aucune autre tâche du dossier n'a enregistré, sauf les tâches terminées ou annulées. Sous Windows, la comparaison ignore la casse. Ainsi, deux tâches n'écrivent jamais dans le même `.part`, même préparées en même temps, ni quand le `.part` de l'une manque. Un arrêt brutal entre la création du `.part` et l'enregistrement du nom laisse un `.part` vide, que la tâche ne reprend pas : elle prendra un autre nom.

### 7.2 Segmentation dynamique

Les segments initiaux sont au nombre de `N = min(connexions cibles, taille / taille minimale d'un segment)`. Ensuite, chaque connexion qui se libère reçoit du travail selon cette règle :

```text
quand une connexion est libre :
  s = un segment 'pending'
  si aucun :
    a = le segment 'active' qui a le plus d'octets restants (R)
    si R >= 2 × taille minimale :
      milieu = a.début_restant + R / 2
      dans une transaction :
        a.end_offset = milieu − 1
        s = nouveau segment [milieu, ancienne fin], état 'active'
      la connexion qui servait a s'arrête à la nouvelle borne
    sinon : la connexion reste libre (fin de tâche proche)
  démarrer la lecture de s avec  Range: bytes=(s.start + s.written)-(s.end)
```

La taille minimale d'un segment est de 1 Mio au départ : en dessous, le coût d'une connexion supplémentaire dépasse le gain. Le nouveau segment réutilise une connexion déjà ouverte (HTTP keep-alive ou flux HTTP/2) pour éviter une négociation TLS de plus.

Le milieu se calcule sur les octets déjà reçus par la connexion, y compris ceux qui ne sont pas encore enregistrés. La connexion du segment coupé recule d'abord sa borne : elle n'accepte plus rien au-delà. Ensuite seulement, la coupe est enregistrée, dans une transaction qui vérifie la couverture. Un arrêt entre les deux laisse en base l'ancien découpage, qui reste juste. Si la connexion a dépassé le milieu entre la lecture et la coupe, le calcul est refait.

### 7.3 Nombre de connexions automatique

Quand l'utilisateur ne fixe pas `maxConnections`, le moteur ajuste le nombre de connexions par tâche selon une règle de montée rapide tant qu'elle paie et de réduction rapide :

- Départ à 4 connexions.
- Toutes les 2 secondes, si le débit total a progressé d'au moins 10 % depuis la dernière augmentation, il double le nombre de connexions, jusqu'à 16 (plafond automatique ; 32 au maximum en réglage manuel). Depuis 4, le plafond est atteint en deux fenêtres.
- Si le gain est inférieur, il cesse d'augmenter.
- Sur une réponse 429 ou 503, ou une connexion refusée, il divise le nombre de connexions par deux et mémorise pour cet hôte, pendant la session, un plafond égal à la valeur atteinte moins une.

Cette boucle est volontairement proche du démarrage de TCP (le nombre double à chaque fenêtre tant que le débit suit) : prudente quand le serveur résiste, rapide quand il répond. Une première version ajoutait une seule connexion par fenêtre ; elle laissait le mode automatique loin derrière sur les téléchargements courts face à un serveur qui bride chaque connexion (`docs/MESURES.md`).

Précisions de mise en œuvre :
- La première mesure double toujours le nombre de connexions, car il n'y a pas encore de débit de référence. Une fenêtre sans aucun octet reçu ne permet aucune conclusion : elle ne change rien.
- Chaque connexion ajoutée prend la moitié du plus gros segment en cours (section 7.2).
- Quand une connexion reçoit un 429 ou un 503, ou voit sa connexion refusée, alors que d'autres connexions tournent encore, la tâche n'échoue pas. Le segment de cette connexion attend son tour à partir de ses octets enregistrés, et les autres connexions continuent.
- Si c'est la dernière connexion de la tâche, l'échec suit la section 7.8 (nouvelle tentative, `Retry-After`).
- Le plafond par hôte vit en mémoire, pour la durée du processus du moteur.
- Un nombre de connexions fixé par l'utilisateur (`maxConnections`) n'est jamais ajusté.

### 7.4 Reprise après arrêt

Au démarrage, pour chaque tâche `active` ou `retry_wait` :

1. Le moteur ouvre le `.part` et vérifie que sa taille couvre tous les octets que `segments` déclare écrits. Sinon, les segments concernés repassent à `pending`, avec `written_bytes` remis à zéro.
2. Il vérifie les invariants de couverture (section 5.6).
3. Il rouvre les connexions avec `Range` et `If-Range` portant l'`ETag` mémorisé (ou la date `Last-Modified`).
4. Réponse `206` : le fichier n'a pas changé, le transfert reprend là où il s'était arrêté. Réponse `200` : le fichier distant a changé, la tâche passe en pause avec `pause_reason = file_changed` (F-13).
5. Sans `ETag` ni `Last-Modified`, le moteur compare la taille et signale une validation faible ; une empreinte fournie par l'utilisateur reste alors la seule garantie.
6. Une tâche sans plages, ou de taille inconnue, ne reprend pas en cours de route : elle repart du début.
7. Si le `.part` a disparu alors que tous les segments sont terminés et que le fichier final existe avec la bonne taille, l'arrêt a eu lieu entre le renommage et l'enregistrement de la fin : la tâche est terminée sans nouveau téléchargement. Pour une tâche de taille inconnue, la présence du fichier final suffit.

Le planificateur reprend ces tâches avant les autres de leur file. Un arrêt demandé du moteur (`engine.shutdown`) arrête chaque tâche en cours après son point de contrôle, puis la remet en `queued` (`active` → `paused` → `queued`), pour qu'elle reparte au démarrage suivant. Un arrêt brutal entre ces deux changements laisse la tâche en pause : aucune donnée n'est perdue, mais elle attend une reprise manuelle.

### 7.5 Écriture sur disque

- Chaque connexion écrit à sa position dans le fichier (écriture positionnée), avec un tampon de 256 Kio à 1 Mio, sans verrou partagé entre segments.
  - Le tampon est écrit dès qu'il est plein, ou quand il garde des octets depuis plus de 500 ms.
  - Sans ce délai, une connexion lente ou bridée (section 7.6) garderait des secondes d'octets en mémoire : les points de contrôle ne les enregistreraient pas, et l'avancement affiché avancerait par sauts.
- Le moteur vide les données sur disque (`fdatasync`) à la fin de chaque segment et à chaque point de contrôle, avant d'enregistrer les compteurs (règle 1 de la section 5.6). Les vidages d'un même fichier sont groupés : un seul à la fois. Une demande faite pendant un vidage en cours attend le suivant, qui couvre toutes les demandes en attente, car un vidage commencé avant la demande ne suffit pas. Des vidages simultanés du même fichier multiplient le temps système et l'attente disque sous Linux (`docs/MESURES.md`, relevé du 2026-10-16). Au point de contrôle, les compteurs sont relevés avant le vidage : les octets écrits pendant le vidage attendent le point suivant. Un compteur enregistré ne recule jamais, sauf à la reprise (section 7.4, étape 1).
- Un point de contrôle a lieu toutes les 2 s, et aussi dès que les connexions ont remis 32 Mio au système depuis le précédent ; l'intervalle repart alors de lui.
  - Sans ce seuil, un téléchargement rapide reçoit tout avant son premier vidage, puis attend que le disque écrive tout le fichier : la réception et l'écriture se suivent au lieu de se recouvrir. Mesuré sous WSL, 512 Mio arrivent en 1,4 s, puis le vidage final dure 2,8 s (`docs/MESURES.md`, 2026-11-06).
  - Un téléchargement lent garde un vidage toutes les 2 s, pas plus.
  - Après un arrêt brutal, il reste aussi moins à retélécharger.
- À la fin, le `.part` est vidé, fermé, puis renommé en nom final ; un fichier déjà présent sous ce nom n'est jamais écrasé.
- Sous Android, l'écriture passe par le sélecteur de stockage du système. Je n'ai pas vérifié si les descripteurs de fichier qu'il fournit autorisent les écritures positionnées sur tous les appareils et versions ; c'est un point à valider dans le premier prototype Android, avec une solution de repli (fichier temporaire dans le stockage de l'application, déplacé à la fin).
- Essai de phase 1, premier relevé (2026-10-15, Galaxy S25 Ultra, Android 16, moteur compilé avec le NDK pour l'API 26) :
  - **Stockage interne de l'application** : tous les tests du moteur passent sur l'appareil, sauf les deux tests HTTPS, ignorés sous Android. Une ISO de 756 Mio téléchargée en HTTP, tuée deux fois puis reprise, garde l'empreinte publiée.
  - **HTTPS** : `rustls-platform-verifier` exige sous Android une initialisation par l'application (JNI et `Context`). Sans elle, il panique au premier certificat et refuse toute autorité ajoutée. Le moteur refuse donc HTTPS sous Android par l'erreur `TLS_UNAVAILABLE` (section 6.8), même après une redirection. L'application Android devra faire cette initialisation au démarrage du moteur (phase 4).
  - **Sélecteur de stockage, dossier « Téléchargements »** (fournisseur `com.android.providers.downloads.documents`, outil `experiments/android-saf`) : tous les essais passent.
    - Le descripteur obtenu en `"rw"` est un fichier ordinaire du stockage partagé, et `lseek` et `ftruncate` fonctionnent.
    - 48 Mio écrits par `pwrite` depuis quatre fils, dans le désordre, à 675 Mio/s, puis `fdatasync` en 44 ms.
    - La relecture est exacte, par `pread` comme par flux.
    - Après la mort du processus, la permission persistante rouvre le fichier, dont le contenu est intact, et l'écriture des 16 Mio restants le complète. L'écriture positionnée par ce sélecteur est donc possible sur cet appareil.
  - **Sélecteur de stockage, Google Drive** (fournisseur `com.google.android.apps.docs.storage`) :
    - L'ouverture en `"rw"` est refusée (« Unsupported mode: rw »), donc toute écriture positionnée est impossible.
    - Le repli fonctionne. La copie finale d'un seul flux (`openOutputStream`) écrit 64 Mio à 476 Mio/s, et la relecture est exacte.
    - Le moteur devra donc savoir qu'un emplacement refuse `"rw"` et basculer alors sur le fichier temporaire interne, copié à la fin. Il lui faudra aussi l'espace pour ce fichier dans le stockage de l'application.
  - Restent à essayer : la carte SD, d'autres appareils et d'autres versions d'Android (phase 4).

### 7.6 Limitation de débit

Trois seaux de jetons s'emboîtent : global, par file, par tâche. Avant chaque lecture réseau, une connexion demande des octets aux trois, et attend le plus restrictif. Les seaux se rechargent toutes les 10 ms. Le changement d'une limite agit immédiatement, sans couper les connexions (F-15).

Précisions de mise en œuvre (module `limit`) :
- **Lecture puis paiement.** Le client HTTP rend les octets par morceaux, dont la taille ne se choisit pas. La connexion attend donc qu'aucun de ses seaux ne soit en dette, lit un morceau, puis le paie aux trois seaux. Un seau peut ainsi passer en dette d'un morceau par connexion, et la lecture suivante attend qu'elle soit remboursée. Le débit moyen est tenu.
- **Rechargement.** Il se calcule en continu, selon le temps écoulé depuis le dernier passage : c'est la limite d'un rechargement toutes les 10 ms, sans minuterie.
- **Réserve.** Un seau inemployé accumule au plus 100 ms de son débit. La minuterie de Windows avance par pas d'environ 15 ms ; avec une réserve plus courte, un réveil en retard ferait perdre du débit.
- **Changement de limite.** Il réveille les connexions qui attendent. Une dette reste due, sauf si la limite disparaît.
- **Coût.** Un seau sans limite ne coûte qu'une lecture atomique par morceau.
- **Où vivent les seaux.** Le planificateur tient le seau global, un seau par file et un par tâche en cours ; chaque exécution reçoit les trois. Le mode `direct` de la CLI n'en a aucun.
- **Où vivent les limites.** La limite globale est enregistrée dans `settings` (clé `limits.downloadBps`) ; celles des files et des tâches, dans leur colonne `speed_limit_bps`. Une limite vaut au moins 1 octet par seconde ; `null` la retire.

### 7.6 bis Espace disque (F-18)

- **Avant le départ.** Le moteur vérifie qu'il reste de quoi finir le fichier, plus une réserve de 64 Mio laissée libre (section 7.1, étape 5). Pour une tâche de taille inconnue, seule la réserve compte.
  - La place qu'il reste à prendre est la taille totale moins la place que le `.part` occupe déjà. Sous Unix, on compte ses blocs, car un fichier creux occupe moins que sa longueur ; ailleurs, sa longueur.
  - Sans préallocation, cette estimation est prudente sous Windows et optimiste sous Linux. Le contrôle à l'écriture reste donc nécessaire.
- **Pendant le téléchargement.** À chaque point de contrôle, le moteur vérifie que la réserve est encore libre. C'est le cas d'un autre programme, ou d'une autre tâche, qui remplit le disque.
- **À l'écriture.** Une écriture qui échoue faute de place (`ENOSPC`, `ERROR_DISK_FULL`) a le même effet.
- **Arrêt.** Dans tous ces cas, la tâche s'arrête comme sur une pause : chaque connexion vide ce qu'elle a écrit, et les compteurs sont enregistrés (section 5.6, règle 1). La tâche passe en pause, cause `disk_full`, code `DISK_FULL`, avec en détail la place nécessaire et la place libre.
- **Reprise.** Toutes les 30 s, le planificateur vérifie la place pour les tâches en pause `disk_full`. Quand il y a de quoi finir plus la réserve, il les remet en file.
- **Mesure impossible.** Si la place libre ne peut pas être mesurée, le moteur part quand même : un disque vraiment plein arrêtera la tâche proprement à l'écriture.
- **Essais.**
  - Un vrai disque plein est essayé sous Linux sur un `tmpfs` de 8 Mio, monté sans droits d'administrateur dans un espace de noms utilisateur. C'est le test ignoré `a_really_full_disk_loses_nothing`, lancé avec `AURO_FULL_DISK_DIR`.
  - Le 2026-11-06, l'écriture a échoué après 6 176 768 octets enregistrés. Chaque octet compté en base était exact sur disque, et la tâche a fini après libération de la place.
  - Sous Windows, le manque de place est simulé (mesure remplacée par les tests) ; l'échec réel d'écriture n'y est pas essayé.

### 7.7 Multi-miroirs

- Le moteur refuse un miroir dont la taille ou les validateurs diffèrent de la source de référence, sauf si une empreinte fournie permet de trancher à la fin.
- Chaque source a un débit moyen lissé. Les segments sont confiés de préférence aux sources les plus rapides.
- Une source dont le débit tombe sous le quart de la médiane passe à `slow` : elle ne reçoit plus de nouveaux segments, sans être coupée.
- Après 3 échecs consécutifs, elle passe à `failed` pour un délai croissant, puis est retestée.

### 7.8 Nouvelles tentatives

Après un échec temporaire, la tâche passe en `retry_wait` et le délai avant la nouvelle tentative est `min(plafond, base × 2^n) + aléa`, avec une base de 2 secondes et un plafond de 5 minutes. Un en-tête `Retry-After` prime sur ce calcul. Au bout de 10 échecs d'affilée (réglable), la tâche passe en `failed`.

### 7.9 Vérification d'intégrité

En flux unique, l'empreinte se calcule au fil de l'eau. Pour un fichier segmenté, elle se calcule à la fin, par une lecture séquentielle du fichier, pendant l'état `verifying`. Si des empreintes par segment existent (torrent, Metalink), chaque segment est contrôlé dès qu'il est terminé et seul le segment fautif est téléchargé de nouveau (F-12).

### 7.10 Paramètres par défaut

| Paramètre | Valeur de départ | Réglable |
| --- | --- | --- |
| Taille minimale d'un segment | 1 Mio | oui |
| Connexions au départ / plafond automatique | 4 / 16 | oui |
| Fenêtre de mesure du débit | 2 s | non |
| Point de contrôle de l'état | 2 s, ou 32 Mio reçus depuis le précédent | non |
| Délai de reprise (base / plafond) | 2 s / 5 min | oui |
| Échecs avant abandon | 10 | oui |
| Tampon d'écriture | 256 Kio à 1 Mio | non |
| Durée maximale d'octets en tampon | 500 ms | non |
| Réserve d'un seau de débit inemployé | 100 ms de son débit | non |
| Place laissée libre sur le disque de destination | 64 Mio | non |
| Vérification de la place pour les tâches en pause `disk_full` | 30 s | non |

## 8. Sécurité, vie privée et cadre légal

Un gestionnaire de téléchargement écrit sur le disque des fichiers venus d'ailleurs et ouvre un port réseau local : ce sont les deux surfaces d'attaque à traiter en priorité. Les parades ci-dessous sont des exigences de conception, vérifiées par des tests dédiés (section 9).

### 8.1 Menaces et parades

| Menace | Exemple | Parade |
| --- | --- | --- |
| Attaque depuis une page web vers le port local | Une page ouverte dans le navigateur envoie des commandes au moteur (CSRF, DNS rebinding) | Jeton obligatoire même en local, contrôle des en-têtes `Host` et `Origin`, aucun CORS permissif |
| Nom de fichier piégé | `Content-Disposition` contenant `../../` ou un nom réservé du système | Nettoyage du nom, résolution du chemin final, refus de tout chemin qui sort du dossier de destination |
| Redirection dangereuse | Un lien HTTPS renvoie vers `file://` ou vers une adresse interne | Liste blanche de schémas, rétrogradation HTTPS vers HTTP signalée à l'utilisateur |
| Requêtes vers le réseau interne (SSRF) | Un client distant demande au moteur d'un NAS de récupérer une adresse du réseau local | Adresses privées, de boucle locale et de lien local refusées pour les jetons distants, sauf réglage explicite |
| Fichier malveillant | Un exécutable téléchargé | Aucune ouverture ni exécution automatique ; avertissement sur les extensions exécutables ; commande externe (F-34) pour brancher un antivirus |
| Fuite de secrets | Mot de passe ou cookie dans la base ou les journaux | Secrets dans le coffre du système, masqués dans les journaux, adresses réduites au nom d'hôte par défaut |
| Plugin malveillant | Un plugin qui lit des fichiers ou envoie des données | Exécution dans un bac à sable, permissions déclarées (domaines réseau, dossiers) et acceptées par l'utilisateur |
| Mise à jour compromise | Faux installateur, faux yt-dlp | Mises à jour signées ; yt-dlp téléchargé depuis la source du projet avec contrôle de l'empreinte qu'elle publie |
| Exposition de l'adresse IP en torrent | Les pairs voient l'adresse de l'utilisateur | Option de liaison à une interface réseau précise (VPN) ; arrêt du torrent si cette interface disparaît |

Aléa cryptographique : TLS repose sur l'aléa du système (`getrandom` sous Linux et Android, le générateur du système sous Windows), sur lequel s'amorce aws-lc, le fournisseur cryptographique de rustls. Depuis le 2026-11-06, aws-lc est compilé sans sa source d'appoint, l'entropie de gigue du processeur (`AWS_LC_SYS_NO_JITTER_ENTROPY=1`, dans `.cargo/config.toml`). Cette source coûtait 56 à 60 ms de processeur au premier tirage de chaque processus (`docs/MESURES.md`). Elle sert surtout aux certifications FIPS, que le projet ne vise pas. L'aléa du système est celui qu'utilisent par défaut OpenSSL, GnuTLS et ring.

### 8.2 Vie privée

- Aucune télémétrie, aucun compte, aucune requête vers un serveur du projet en dehors de la vérification de mises à jour, désactivable.
- Un mode « sans historique » n'écrit ni `history` ni `event_log`.
- Les journaux et rapports de diagnostic remplacent les adresses complètes par le nom d'hôte, sauf demande contraire.
- L'extension navigateur ne lit que les requêtes de téléchargement et les pages que l'utilisateur lui désigne ; elle ne transmet rien ailleurs qu'au moteur local.

### 8.3 Cadre légal

Ce qui suit n'est pas un avis juridique ; un juriste doit le valider avant la première publication.

- **L'outil est légal, l'usage dépend du contenu.** Le projet ne doit pas faciliter le contournement de mesures techniques de protection (DRM), que prohibent la plupart des législations, dont la directive européenne 2001/29/CE et le DMCA américain. La v1 exclut donc ce périmètre (section 1) et le projet ne livre aucun plugin de ce type.
- **Composants externes.** yt-dlp et ffmpeg sont appelés comme processus séparés, sans être liés au programme, ce qui garde la licence du projet indépendante de la leur. Un contrôle automatique des licences (de type `cargo-deny`) vérifie à chaque build que chaque dépendance est compatible avec la GPL-3.0.
- **Nom et marques.** Le projet porte un nom original ; il n'utilise ni les noms, ni les logos, ni le code d'IDM, JDownloader, FDM ou d'autres produits.
- **Boutiques d'applications.** Les règles de Google Play limitent les applications qui téléchargent des contenus de plateformes en dehors de leurs conditions d'utilisation ; je ne les ai pas relues pour ce document. La distribution visée en priorité est F-Droid et un APK signé, avec l'étude d'une version Play sans module vidéo, à confirmer après lecture des règles en vigueur.

## 9. Tests, risques et feuille de route

Le projet repose sur un principe simple : aucune version ne sort si un test de reprise ou d'intégrité échoue. La feuille de route place ces tests, et les points techniques les plus incertains, avant les fonctions de confort.

### 9.1 Plan de tests

| Niveau | Contenu | Bloquant pour une version |
| --- | --- | --- |
| Unitaires | Segmentation (tests à propriétés : après un nombre aléatoire de redécoupages, la couverture reste exacte), délais de reprise, seaux de jetons, lecture des en-têtes | Oui |
| Intégration | Serveur de test local qui simule `Range` accepté ou refusé, `ETag` qui change, coupures, lenteur, réponses 429 et 503, redirections, noms de fichiers piégés | Oui |
| Reprise sous arrêt brutal | Tuer le moteur à des instants aléatoires pendant un téléchargement de 1 Go, puis vérifier l'empreinte du fichier final ; 1 000 cycles (objectif de la section 1) | Oui |
| Performance | Mesures face à aria2, à nombre de connexions égal, sur serveur local bridé (HTTP et HTTPS) et sur serveurs réels. Bloquent : un débit inférieur de plus de 5 % à celui d'aria2, et un coût processeur par octet (processus et vidage sur disque) au-dessus du seuil de N-01. La porte se mesure sous Linux en intégration continue ; Windows se mesure à part, avec et sans exclusion antivirus du dossier de téléchargement | Oui |
| Sécurité | Cas de la section 8.1 (traversée de dossier, `Host` et `Origin` falsifiés, SSRF), fuzzing des lecteurs d'en-têtes, d'URL et de fichiers `.torrent` | Oui |
| Contrat du protocole | Échanges de référence validés par des schémas JSON ; un client de la version précédente doit toujours fonctionner | Oui |
| Plateformes | Intégration continue sur Windows, Linux et émulateur Android | Oui |
| Accessibilité | Parcours au clavier et avec les lecteurs d'écran NVDA (Windows), Orca (Linux) et TalkBack (Android) | Pour la v1 seulement |

Le banc de reprise sous arrêt brutal (`crates/cli/tests/crash_resume.rs`) tue le processus à un instant aléatoire et relance la tâche. Après chaque arrêt, il vérifie les règles 1 à 4 de la section 5.6 sur la base et le `.part`, puis compare chaque fichier terminé octet par octet au contenu servi. Une version courte tourne à chaque intégration continue ; la version de la porte de phase 1 se règle par variables d'environnement. Tuer le processus laisse intact le cache du système : une coupure de la machine elle-même demande un banc séparé (machine virtuelle coupée net, ou système de fichiers qui simule la perte du cache), à prévoir.

Le banc de reprise du moteur autonome (`crates/daemon/tests/crash_daemon.rs`, phase 2) fait de même avec `aurod`. Plusieurs tâches tournent à la fois et le banc en ajoute par le protocole pour en garder toujours quatre en cours. Un cycle sur cinq finit par un arrêt demandé (`engine.shutdown`) au lieu d'un arrêt brutal. Les contrôles sont communs aux deux bancs (`testing::Checker`). Campagne du 2026-11-06 sous Windows 11, sans aucun défaut : 150 lancements (graine 1793925077934193901), 120 arrêts brutaux et 30 arrêts demandés, 73 fichiers de 4 Mio terminés et vérifiés octet par octet, en 4 min 8 s. Aucun arrêt n'y tombe sur un découpage redécoupé, car chaque tâche garde le nombre de connexions fixé à l'ajout : le redécoupage sous arrêt brutal reste couvert par le banc de la phase 1.

Version de la porte de phase 1, passée le 2026-10-15 sous Windows 11, sans aucun défaut. Le banc a fait 1 000 lancements sur un fichier de 1 Gio, en 1 h 41 (graine 1792094471405815701) :
- 845 arrêts brutaux, dont 456 sur un découpage redécoupé ;
- 782 reprises avec des octets déjà enregistrés ;
- 155 fichiers terminés et vérifiés octet par octet.

Le banc de performance (`crates/cli/tests/perf_vs_aria2.rs`, ignoré par défaut) compare `auro` et `aria2c` à nombre de connexions égal : serveur local sans bridage, en HTTP puis en HTTPS ; serveur local bridé par connexion ; et un serveur réel au choix. Chaque mesure va jusqu'au fichier sur disque. `auro` vide ses données (règle 1 de la section 5.6), mais pas `aria2c` ni `curl` : le banc vide donc lui-même le fichier après chaque outil, et compte ce temps. Le banc relève trois temps processeur :
- **Celui du processus de l'outil, plus celui que le banc passe à vider sur disque ce que l'outil a laissé en mémoire.** Il fait la porte de N-01 (depuis le 2026-11-06, second choix du jour). `auro` paie ses vidages dans son processus (règle 1), aria2 les laisse au système : compter le vidage pour les deux les compare à armes égales. Ce que le système a déjà écrit de lui-même pendant le téléchargement n'y est pas compté, ce qui avantage encore un peu aria2.
- **Celui du seul processus de l'outil.** Il a fait la porte le 2026-11-06, avant le choix précédent. Il n'est plus qu'indicatif : il avantage les outils qui ne vident pas leurs données, et à 16 connexions le rapport `auro` / aria2 oscille de 0,93 à 1,07 d'un passage à l'autre autour de la parité, l'écart restant étant le vidage qu'`auro` paie seul (`docs/MESURES.md`).
- **Celui de toute la machine pendant la mesure, moins celui du banc et de son serveur de test.** Il compte ce que le système fait pour un outil hors de son processus, comme l'écriture différée sur disque ou l'analyse par l'antivirus. C'était la mesure de la porte jusqu'au 2026-11-06. Elle n'est plus qu'indicative : sur la machine virtuelle WSL d'un poste de travail, elle varie de ±30 % d'un passage à l'autre, même binaire et même aria2 (`docs/MESURES.md`).

Le banc alterne les outils d'un passage à l'autre et retient la médiane de chaque grandeur. Il relève le débit, les trois temps processeur et les rapports des coûts par octet entre `auro` et `aria2c`. Il doit tourner sur une machine au repos. Une validation périodique sur une machine Linux native, ou en intégration continue dédiée, est prévue. Le serveur HTTPS présente un certificat signé par une autorité créée pour le banc. `auro` la reçoit par `--ca-cert`, qui l'ajoute aux racines du système sans affaiblir la validation (N-07). `aria2c` la reçoit par `--ca-certificate`, sauf sous Windows : sa version Windows passe par la pile TLS du système, qui ignore cette option. Il y tourne donc sans vérification du certificat (`--check-certificate=false`). Cette vérification n'a lieu qu'une fois par connexion : l'écart qu'elle crée sur un long transfert est négligeable, et il joue en faveur d'aria2. Les relevés datés sont consignés dans `docs/MESURES.md`, avec la machine, le système et la présence d'un antivirus sur le dossier de téléchargement. Le seuil absolu de N-01 n'a de sens que sur la machine de référence nommée dans le relevé.

### 9.2 Risques principaux

| Risque | Conséquence | Réponse prévue |
| --- | --- | --- |
| Limites d'Android sur les services en arrière-plan | Les téléchargements longs peuvent être interrompus par le système, selon la version | Étudier dès la phase 1 les services au premier plan et les transferts lancés par l'utilisateur proposés par Android ; ma connaissance de leurs durées maximales n'est pas vérifiée, donc à tester sur appareils réels |
| Écriture positionnée via le sélecteur de stockage Android | Segmentation impossible ou plus lente sur certains appareils | Prototype en phase 1 ; repli par fichier temporaire interne (section 7.5) |
| yt-dlp est écrit en Python | Intégration lourde sur Android | Évaluer un interpréteur embarqué ; sinon limiter la vidéo à Windows et Linux en v1 |
| Capture dans le navigateur (Manifest V3 sous Chrome) | Moins de possibilités d'intercepter les téléchargements | Tester la capture dès la phase 3 ; s'appuyer sur l'API de téléchargements du navigateur plutôt que sur l'interception de requêtes |
| Faux positifs d'antivirus sur Windows | Blocage de l'installateur, perte de confiance | Signature du code, builds reproductibles, publication des empreintes |
| Étendue du projet | Livraison jamais atteinte | Les priorités M/S/C (section 2) découpent la v1 ; tout ajout passe par un arbitrage écrit |
| Dépendances torrent | Poids, licence ou maintenance d'une bibliothèque | Isoler le torrent derrière une interface interne, pour changer de bibliothèque sans toucher au reste |

### 9.3 Feuille de route

Les phases s'enchaînent dans l'ordre ; chaque porte (losange) est un critère à satisfaire avant de passer à la suite. Aucune durée n'est fixée : elle dépend du temps que l'équipe pourra y consacrer.

**Le moteur prouve sa fiabilité avant l'interface et les fonctions avancées.** Chaque porte est à franchir avant la phase suivante ; les phases s'enchaînent de haut en bas, sans dates.

| Phase | Contenu | Porte à franchir avant la suivante |
| --- | --- | --- |
| 1. Moteur minimal et essais de faisabilité | Moteur segmenté, reprise, SQLite, CLI, essai Android | 1 000 reprises sans corruption, écart avec aria2 sous 5 %, essai Android validé |
| 2. Application de bureau et files | Interface Windows et Linux, files, limites, planificateur | Utilisation quotidienne en interne sans aucune perte de données |
| 3. Extension navigateur | Firefox, Chrome, Edge, relais natif, jeton limité à l'ajout | Capture fiable sur Firefox, Chrome et Edge, y compris sous Manifest V3 |
| 4. Application Android | Application native, service au premier plan, partage, stockage | Téléchargements longs menés à terme sur des appareils réels de plusieurs versions d'Android |
| 5. Vidéo et torrent | yt-dlp et ffmpeg, HLS, DASH, torrent et magnet | Tests d'intégration vidéo et torrent réussis, licences des dépendances vérifiées |
| 6. Fonctions avancées | Miroirs, analyse de liens, plugins, interface web distante | Revue de sécurité de l'API distante et des plugins |
| 7. Version 1 | Installateurs signés, mises à jour, documentation, accessibilité | Tous les tests bloquants du plan de la section 9.1 réussis |

En phase 1, la ligne de commande appelle le moteur comme une bibliothèque ; l'API JSON-RPC (section 6) arrive en phase 2, avec l'application de bureau.

Les deux points les plus incertains, l'écriture sur le stockage Android et la capture dans le navigateur, sont testés dans les phases 1 et 3, avant que le reste du produit ne dépende d'eux.

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
| F-07 | Proxy HTTP et SOCKS5, avec détection des réglages du système | S. Fait le 2026-11-12 (phase 7, étape 3, décisions de l'utilisateur) : modes système (par défaut ; réglages Internet sous Windows, variables d'environnement ailleurs), aucun ou manuel (`http`, `https`, `socks5`, `socks5h`, exceptions, identifiant, mot de passe au coffre) ; méthodes `proxy.get` et `proxy.set` ; tout le HTTP le suit (yt-dlp par son environnement), jamais les torrents ; les connexions limitées au réseau public ne suivent qu'un SOCKS5, interrogé avec une adresse résolue et vérifiée par le moteur, et sortent sinon en direct |
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
| N-11 | Licence | Logiciel libre | GPL-3.0 ou ultérieure pour le moteur et les clients (décision de l'utilisateur du 2026-11-09 ; texte dans `LICENSE`, champ `license` des paquets Cargo) |
| N-12 | Observabilité | Journaux | Journaux structurés avec niveaux et rotation ; export de diagnostic sans adresses ni identifiants. Fait le 2026-11-12 (phase 7, étape 2, décisions de l'utilisateur) : événements en lignes JSON (`tracing`), dans `logs/` à côté de la base, 5 fichiers de 5 Mio au plus ; niveau `info`, réglage `logs.level` sans redémarrer, variable `AURO_LOG` qui l'impose ; seuls les modules du projet y écrivent (les bibliothèques, librqbit compris, peuvent nommer des pairs) ; jamais une adresse complète ni un en-tête, un hôte au plus ; `engine.diagnostic`, `auro diagnostic --save`, « Exporter un diagnostic… » du bureau, contenu montré avant l'enregistrement |
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
| Torrent | Bibliothèque Rust ou `libtorrent`, derrière une interface interne | À trancher par un prototype ; l'interface permet d'en changer. Prototype du 2026-11-09 (`experiments/torrent/README.md`) : librqbit 9.1.0 face à libtorrent 2.1.1 ; ni l'une ni l'autre ne garantit la règle 1 (5.6) après une coupure, que le module `torrent` devra assurer lui-même. Décision de l'utilisateur du 2026-11-09 : librqbit ; acceptés pour la v1 : pas de chiffrement du protocole, ni sources web, ni torrents v2 ; la liaison à une interface sous Windows sera ajoutée par le projet |
| Vidéo | yt-dlp et ffmpeg en processus externes | Extracteurs maintenus par d'autres, licences séparées, mises à jour indépendantes (F-25) |
| Interface de bureau | Tauri 2 (choisi le 2026-11-06, après deux prototypes comparés sous Windows et Linux : `experiments/desktop/README.md`) | Une seule page web pour le bureau et l'interface distante (F-39), servie par l'hôte de chacune ; client du moteur en Rust, repris du moteur. Qt Quick était plus léger (mémoire, processeur fenêtre ouverte, démarrage), mais aurait demandé une seconde interface |
| Interface Android | Kotlin avec Jetpack Compose, moteur chargé comme bibliothèque native | Partage, services et stockage sont des API Android natives |
| Extension | WebExtension (Chrome, Edge, Firefox) et programme relais natif | Seule voie propre vers le moteur local |
| Plugins | WebAssembly, moteur wasmi (interpréteur) | Bac à sable, plusieurs langages possibles. Prototype du 2026-11-12 (`experiments/plugins/README.md`) : wasmi 2.0 face à wasmtime 50 sur le même plugin d'essai ; les deux tiennent le bac à sable (calcul, mémoire, pile, domaines), wasmtime est 6 fois plus rapide mais ajoute 11 Mo, 96 crates, environ 250 ms de compilation par plugin et produit du code machine. Décision de l'utilisateur du 2026-11-12 : wasmi ; interface en WebAssembly de base et messages JSON, qui laisserait changer de moteur |

### 4.4 Android

- Le moteur est compilé comme bibliothèque native et appelé directement, avec les mêmes messages JSON-RPC que partout ailleurs.
- Un service au premier plan héberge le moteur et affiche une notification avec la progression, la pause et la reprise.
- Le menu « Partager » d'Android et les liens ouverts depuis d'autres applications appellent `task.add`.
- Le dossier de destination est choisi par l'utilisateur dans le sélecteur de stockage, avec une permission conservée entre les sessions. Révisé le 2026-11-08 (décision de l'utilisateur) : par défaut, `Download/AuroDownloader` dans le dossier public des téléchargements, écrit par des chemins ordinaires (voir la mise en œuvre ci-dessous) ; le sélecteur viendra ensuite pour les autres emplacements (carte SD, Google Drive), avec le repli de la section 7.5.
- Options : Wi-Fi seulement, respect de l'économiseur de batterie, reprise au redémarrage de l'appareil si l'utilisateur l'autorise.
- L'application peut aussi servir de simple télécommande d'un moteur distant (un NAS, par exemple), sans moteur local.
- Les contraintes d'Android sur les tâches longues sont le principal risque de cette plateforme (section 9.2).
- Mise en œuvre (phase 4, depuis le 2026-11-08) :
  - Bibliothèque native `auro-android` (`crates/android`, `libauro_android.so`, arm64, API 26) : le moteur tourne dans le processus de l'application, avec son propre runtime ; l'application lui parle par la classe Kotlin `org.aurodownloader.engine.NativeEngine` : `start(context, base)`, `send(message)`, `stop()`, et `onMessage(message)` en retour, un message JSON-RPC par appel, comme sur le canal local (même session, portée complète).
  - `start` initialise d'abord la vérification des certificats d'Android (`rustls-platform-verifier`, dont la partie Kotlin vient du dépôt Maven du projet rustls, à la version de `Cargo.lock`) : HTTPS est permis dès lors (section 7.5).
  - Application : `apps/android` (Gradle, Kotlin) ; la construction compile la bibliothèque par Cargo et le NDK. Base dans le stockage privé de l'application ; téléchargements, en attendant le sélecteur de stockage, dans le dossier de l'application (`Android/data/org.aurodownloader/files/Download`).
  - Essai du 2026-11-08 sur une tablette Teclast T70 (Android 15) : HTTPS, empreinte voisine, 82 Mo en 4 s ; ISO Debian de 756 Mio, application tuée au bout de 8 s, reprise seule au démarrage suivant et empreinte publiée.
  - Dossier des téléchargements (étape 2, 2026-11-08) : `Download/AuroDownloader` dans le dossier public, créé et réglé (`downloads.dir`) au premier démarrage, ou s'il a disparu ; un dossier choisi ensuite n'est pas remplacé. Depuis Android 11, une application y crée, écrit et renomme ses fichiers par leur chemin, sans permission : le moteur y travaille comme sur un disque ordinaire (écritures positionnées en parallèle, réservation de la place, renommage final), essai du même jour sur la tablette. Android 8 à 10 : permission d'écriture (`maxSdkVersion` 29) et `requestLegacyExternalStorage`, pas encore essayés sur un appareil. À la fin de chaque téléchargement, l'application demande l'indexation du fichier (`MediaScannerConnection`), sans laquelle il manque aux recherches de l'application Fichiers.
  - Fichiers d'autres applications : un fichier déposé par adb dans le dossier est vu, et le moteur en évite le nom (`README (1)`). Pour un fichier ordinaire (non multimédia) d'une autre application, que la vue limitée d'Android peut cacher, ce n'est pas vérifié : le renommage final pourrait ne pas le voir. Le sous-dossier propre à l'application rend le cas rare ; à vérifier.
  - Interface (étape 3, 2026-11-08) : Kotlin et Jetpack Compose (Material 3, couleurs du système à partir d'Android 12, thème clair ou sombre), en français et en anglais, mêmes libellés que l'application de bureau. Liste des téléchargements (les plus récents en tête ; avancement, état, débit et temps restant ; pause ou reprise d'un geste), détails (fichier, dossier, tailles, débit, empreinte avec son origine ; pause, reprise ou relance, ouverture du fichier terminé, annulation, retéléchargement, retrait), ajout par adresse, réglages (dossier, limite de débit globale, empreintes voisines, réservation de la place). Un lien partagé depuis une autre application (« Partager », première adresse http(s) du texte) ou ouvert avec elle (types de fichiers seulement, pour ne pas se proposer comme navigateur) est ajouté. Un fichier terminé s'ouvre avec l'application choisie par Android (`FileProvider`), jamais un programme ni un paquet d'installation (`.apk`…, section 8.1). L'état suit le protocole comme la page du bureau : instantané, notifications plus récentes, rechargement si une révision manque (section 6.10). Lecteur d'écran : chaque ligne se lit d'un seul tenant, chaque bouton nomme son téléchargement ; essai réel avec TalkBack à faire.
  - Arrière-plan (étape 4, 2026-11-09) : section 7.15.
  - Torrents (phase 5, étape 5e, 2026-11-10 ; décisions de l'utilisateur : le périmètre du bureau, ajout direct de ce qu'une autre application confie, partage seulement application à l'écran ou téléchargement protégé, pas de liaison à une interface) :
    - Ajout : la boîte « Ajouter » prend aussi un lien magnet, et « Choisir un fichier .torrent… » ouvre le sélecteur du système (tous types : les fournisseurs ne connaissent pas tous `application/x-bittorrent`). `ShareActivity` reçoit les liens magnet (schéma `magnet:` ouvert, ou texte partagé), les fichiers `.torrent` ouverts (`content:` ou `file:` de type `application/x-bittorrent`, depuis Fichiers ou les téléchargements du navigateur) ou partagés (`EXTRA_STREAM`), et une adresse `http(s)` d'un `.torrent` (type annoncé ou chemin en `.torrent`) ; ils s'ajoutent d'office, comme un lien partagé, et Android demande quelle application les prend. L'application lit le `.torrent` (16 Mio au plus, premier octet `d`, sinon refus nommé) et l'envoie en base64 ; une adresse `http(s)` est lue par l'application elle-même, comme l'extension le fait.
    - Liste et détails : état « Partage (ratio x) », débit d'envoi, « Envoyé aux autres pairs » avec le ratio, « Arrêter le partage » ; fichiers du torrent cochés, modifiables tant que le torrent n'est ni reçu ni annulé (`task.setFiles`, au moins un) ; « Ouvrir » seulement pour un torrent à un fichier. Les fichiers d'un torrent sont indexés (MediaStore) dès le partage, ou à la fin.
    - Réglages, rubrique BitTorrent : partage jusqu'à un ratio (0, 0,5, 1, 2, 3, sans limite) et limite d'envoi (100 Kio/s à 5 Mio/s). Pas d'interface réseau : le VPN d'Android s'applique à toute l'application (« VPN permanent », « Bloquer les connexions sans VPN »).
    - Corrections trouvées par l'essai (valent aussi pour HTTP) : « Annuler le téléchargement » efface le `.part` (`deleteFiles`), et « Retirer de la liste » aussi pour une tâche en échec ou annulée (un fichier terminé reste), comme sur le bureau ; les refus s'affichent aussi dans les détails et les réglages.
    - Essai du 2026-11-10 sur le téléphone (Android 17, Wi-Fi sans frais) : `.torrent` Debian lu depuis son adresse, netinst 13.7 de 756 Mio reçu à 33 Mio/s, empreinte juste, indexé ; lien magnet ouvert comme par un navigateur, métadonnées reçues, pause, choix des fichiers (refus d'un choix vide affiché), annulation (`.part` effacé), retrait ; ratio 0 et envoi à 100 Kio/s réglés depuis l'application. Le partage sur Android (état « Partage ») et l'ouverture d'un `.torrent` depuis l'application Fichiers ne sont pas encore essayés sur un appareil.

### 4.5 Extension navigateur

1. L'utilisateur lance un téléchargement ; l'extension le détecte, l'annule dans le navigateur et relève l'adresse, les en-têtes utiles et les cookies nécessaires.
2. Elle transmet le tout au programme relais, installé avec l'application et déclaré au navigateur.
3. Le relais appelle `task.add` sur le point d'accès local, après s'être limité à la portée `add` à l'ouverture de session (section 6.9 ; décision du 2026-11-07 : pas de jeton sur le canal local, que le système réserve déjà à l'utilisateur).
4. Le moteur répond ; l'extension affiche la confirmation. Si le moteur n'est pas lancé, le relais le démarre.

Les cookies sont rangés dans le coffre du système, jamais dans la base (section 5.1).

Mise en œuvre (phase 3, décisions du 2026-11-07 : Firefox, Chrome et Edge ensemble ; capture de tout ce qui dépasse une taille) :
- **Relais** : `auro-relay` (paquet `auro-cli`), hôte de messagerie native `org.aurodownloader.relay`, installé à côté d'`aurod`. Messages JSON préfixés de leur longueur sur 4 octets (ordre de la machine), au plus 1 Mio. Il se connecte au point d'accès local par défaut avec la portée `add` (section 6.9), et démarre `aurod` s'il ne répond pas. Sous Windows, ce moteur est lancé hors du groupe de processus (job) dans lequel le navigateur range son hôte, pour lui survivre ; si le job l'interdit, il part dedans.
  - `{"type": "ping"}` : `{"ok": true, "version"}`, sans le moteur.
  - `{"type": "add", "url", "requestId", "paused", "headers"}` : `task.add` sans `destDir` (dossier des téléchargements du moteur), `requestId` en `idempotencyKey`, `headers` (objet, facultatif) transmis tels quels. Réponse `{"ok": true, "taskId"}`.
  - Depuis l'étape 5d (2026-11-10) : `add` prend un lien magnet dans `url`, ou un fichier `.torrent` en base64 dans `torrent` à la place de `url` (les deux ensemble : `BAD_MESSAGE`) ; `{"type": "probe", "pageUrl"}` appelle `media.probe` et rend `{"ok": true, "title", "durationS", "formats", "subtitles"}`, chaque format réduit à `formatId`, `ext`, `vcodec`, `acodec`, `height`, `filesize`, `filesizeApprox` et `drm` ; `{"type": "video", "pageUrl", "format", "subtitles", "title", "requestId"}` appelle `task.add` avec `media`. Un message reçu fait au plus 24 Mio (un `.torrent` de 16 Mio en base64) ; une réponse de plus de 1 Mio, que le navigateur refuserait, devient le refus `REPLY_TOO_LARGE`.
  - Un champ `id` est renvoyé tel quel. Refus : `{"ok": false, "code", "message"}`, `code` étant celui du moteur, ou `ENGINE_UNAVAILABLE`, `BAD_MESSAGE`, `UNKNOWN_TYPE`.
- **Déclaration aux navigateurs** :
  - Windows (installateur) : clés `HKCU\Software\Google\Chrome\NativeMessagingHosts\org.aurodownloader.relay`, `…\Microsoft\Edge\…` et `…\Mozilla\…`, vers `relay-chromium.json` et `relay-firefox.json` du dossier du programme ; retirées à la désinstallation si elles désignent ce dossier.
  - Linux (`.deb`) : `/etc/opt/chrome/`, `/etc/chromium/`, `/etc/opt/edge/native-messaging-hosts/` et `/usr/lib/mozilla/native-messaging-hosts/`. L'AppImage ne déclare rien : son chemin change à chaque lancement.
  - Identifiants admis : `chrome-extension://blpdooihcodaljmhaldncnkgchpaaheo/` (clé publique inscrite dans le manifeste de l'extension ; un identifiant de boutique s'y ajoutera) et `extension@aurodownloader.org` (Firefox).
- **Extension** (`extension/`, Manifest V3, un seul manifeste pour les trois navigateurs) :
  - Elle observe les téléchargements créés par le navigateur. Elle capture par défaut tout ce qui dépasse 1 Mio ; une taille inconnue est capturée. Restent au navigateur : les adresses non HTTP (`blob:`, `data:`), la navigation privée (sauf réglage), les sites exclus et leurs sous-domaines, et un téléchargement qui suit de moins de 3 s un clic avec la touche Alt.
  - Le téléchargement du navigateur n'est annulé et effacé qu'une fois l'ajout accepté par le moteur ; si le relais manque ou refuse, le navigateur le garde. Un badge sur l'icône le dit (« ✓ » ou « ! »), et les options montrent le dernier envoi.
  - Menu contextuel « Télécharger avec AuroDownloader » sur les liens, images, vidéos et sons, quels que soient les réglages.
  - Cookies et en-têtes de la page (étape 5, depuis le 2026-11-07) : avec l'adresse, l'extension transmet `Referer`, `User-Agent` et `Cookie`.
    - `Referer` : la page d'origine, comme le navigateur l'enverrait lui-même (politique `strict-origin-when-cross-origin`) : l'adresse entière vers la même origine, l'origine seule vers une autre, rien de HTTPS vers HTTP.
    - `Cookie` : les cookies du navigateur pour l'adresse (`cookies.getAll`, chemins les plus longs d'abord), pris dans le magasin du téléchargement (conteneur Firefox, navigation privée).
    - Permissions `cookies` et accès à tous les sites ; Firefox peut laisser ce dernier à accorder, depuis les options.
    - Une option coupe cet envoi (« Transmettre les cookies et la page d'origine », active par défaut).
    - Le moteur range `Cookie` dans le coffre du système (section 5.1) et l'efface quand la tâche est finie.
  - Options : capture oui ou non, taille minimale, sites exclus, navigation privée, cookies ; vérification de la liaison avec le relais.
  - **Torrents et vidéo** (phase 5, étape 5d, 2026-11-10 ; décisions de l'utilisateur : une fenêtre avec le choix de la qualité, le clic sur un lien magnet capté par une option active par défaut, le `.torrent` lu par l'extension) :
    - Lien magnet : un clic (bouton principal, sans touche de modification) sur un lien `magnet:?` d'une page est confié au moteur (`content.js`, puis l'arrière-plan), sans cookies ni page (le moteur les refuse pour un torrent). Avec Alt, l'option coupée, ou si AuroDownloader est injoignable (`RELAY_MISSING`, `ENGINE_UNAVAILABLE`), le lien suit son chemin habituel (programme associé du système) ; un refus du moteur (torrent déjà dans la liste, lien invalide) n'est que signalé (« ! » sur l'icône, « Dernier envoi » des options), décision de l'utilisateur après l'essai du 2026-11-10. Option « Confier les liens magnet à AuroDownloader », active par défaut. Le menu contextuel « Télécharger avec AuroDownloader » prend aussi les liens magnet.
    - Fichier `.torrent` : reconnu par son type (`application/x-bittorrent`), son nom ou le chemin de son adresse, quelle que soit la taille minimale, après les autres règles (sites exclus, Alt, navigation privée). Le téléchargement du navigateur est mis en pause ; l'extension lit le fichier elle-même (`fetch` avec les cookies du navigateur pour ce site), vérifie qu'il commence comme un dictionnaire bencode et fait au plus 16 Mio, puis l'envoie en base64 : le moteur en fait un torrent. Accepté, le téléchargement du navigateur est annulé et effacé (un `.torrent` déjà fini reste où le navigateur l'a mis) ; sinon il reprend.
    - Vidéo de la page : l'icône de l'extension ouvre une fenêtre (`popup.html`) qui décrit la page ouverte (`probe`) : titre, durée, qualités (les règles de l'application de bureau : « Meilleure qualité », chaque hauteur, « Son seul », avec une taille estimée), sous-titres par langue, non cochés. « Télécharger » l'ajoute dans le dossier du moteur. Messages nommés : outils absents (à installer depuis l'application), vidéo protégée (DRM), aucune vidéo, AuroDownloader injoignable. Le menu contextuel « Télécharger la vidéo de cette page avec AuroDownloader » (page ou vidéo) l'ajoute en meilleure qualité, sans sous-titres. Les réglages s'ouvrent depuis la fenêtre. Permission `activeTab` ajoutée, pour lire l'adresse de l'onglet sous Firefox.
    - Les menus contextuels sont recréés à chaque démarrage de l'arrière-plan, pas seulement à l'installation (l'essai sous Edge a montré une extension mise à jour sans son entrée vidéo).
    - Essai réel fait par l'utilisateur le 2026-11-10 sous Edge (essais 10 à 17 de `docs/ESSAI-EXTENSION.md`, application installée mise à jour) : validé. Firefox et Chrome repasseront à la porte de la phase 5 (décision de l'utilisateur).
- **Pas encore** : cookies partitionnés (CHIPS) et posés pendant une redirection, détection automatique des vidéos (bouton sur la vidéo ; la fenêtre de l'extension, depuis l'étape 5d, ne cherche que sur demande), cookies de la page pour la vidéo (le moteur n'en transmet pas à yt-dlp), magasin de cookies d'un conteneur Firefox ou de la navigation privée pour la lecture d'un `.torrent` (l'extension lit avec ceux du magasin par défaut), liens magnet ouverts par un script de la page plutôt que par un clic, publication dans les boutiques (Firefox n'installe une extension non signée que temporairement), Brave, Vivaldi et Opera (CCleaner Browser, autre navigateur Chromium, a passé les essais de la phase 3 le 2026-11-08 avec la même extension et le même relais). Un autre gestionnaire intégré au navigateur (IDM) peut intercepter un téléchargement avant l'extension.

### 4.6 Exécution et distribution

- **Bureau** : l'application démarre le moteur s'il n'est pas lancé et le laisse tourner dans la zone de notification ; démarrage avec la session en option. Un téléchargement ajouté par un autre client (extension, ligne de commande) fait apparaître la fenêtre au premier plan, même repliée dans la zone de notification (préférence de l'interface, activée par défaut, depuis le 2026-11-08) : l'utilisateur voit tout de suite que le téléchargement a été pris.
- **Moteur** : binaire `aurod` (crate `crates/daemon`), sans interface. L'application de bureau le lance s'il ne tourne pas ; `auro` est le client en ligne de commande, et le lance de même, détaché de la console, avec son journal (`aurod.log`) à côté de sa base. `auro direct` exécute une tâche avec la bibliothèque du moteur, sans `aurod`, pour les bancs et le dépannage.
- **Application de bureau, torrents** (phase 5, étape 5b, 2026-11-09 ; décisions de l'utilisateur : association en option désactivée par défaut, choix des fichiers dans les Propriétés) ; essai de l'interface fait par l'utilisateur le même jour sur la variante d'essai (netinst Debian glissé sur la fenêtre, Propriétés, rubrique BitTorrent, suppression), validé :
  - **Ajout** : la boîte « Ajouter » prend des liens magnet et des fichiers `.torrent` choisis ou glissés sur la fenêtre (lus par l'application, 16 Mio au plus, envoyés en base64). Un lien magnet ou un `.torrent` donné à l'application (« Ouvrir avec… », association, relance) préremplit la boîte, que l'utilisateur confirme : rien n'est ajouté d'office. Pas d'empreinte pour un torrent.
  - **Liste** : l'état « Partage » donne le ratio, la colonne Débit l'envoi ; un torrent en file sans son interface réseau est « En attente de l'interface réseau ».
  - **Propriétés** : pairs, octets envoyés et ratio ; fichiers du torrent avec des cases à cocher (`task.setFiles`) dès que les métadonnées sont connues ; « Arrêter le partage » termine la tâche. Ni « Déplacer/Renommer », ni « Actualiser l'adresse », ni empreinte pour un torrent.
  - **Partage et suppression** : un torrent qui partage a ses fichiers complets ; « Annuler le téléchargement » n'est pas proposé pendant le partage, et « Supprimer » ne retire ses fichiers que si la case « Supprimer aussi les fichiers terminés » est cochée, comme pour un téléchargement terminé (sinon ils auraient été effacés).
  - **Options, rubrique BitTorrent** : partage jusqu'à un ratio (0 : ne pas partager ; 0,5, 1, 2, 3, ou sans limite), limite d'envoi, interface réseau choisie dans la liste de `torrent.interfaces`, avec son état, UPnP, et l'association.
  - **Association** (désactivée par défaut) : AuroDownloader devient le programme de l'utilisateur pour les liens magnet et les `.torrent`, en gardant le précédent, rétabli quand l'option est décochée. Windows : clés de `HKCU\Software\Classes`, précédent sous `HKCU\Software\AuroDownloader\Associations` ; un choix fait dans les paramètres de Windows passe avant. Linux : `mimeapps.list` de l'utilisateur et une entrée de bureau cachée (`aurodownloader-liens.desktop`). La désinstallation Windows la retire (`aurodownloader --unregister-links`), pas une mise à jour ; sous Linux, la désinstallation du paquet ne la retire pas.
  - La page suit aussi les notifications numérotées `engine.torrentInterface`, `media.tools` et `engine.warning` (hors `EVENTS_LOST`) : avant, elle y voyait un trou de révision et rechargeait tout l'instantané.
- **Application de bureau, vidéo** (phase 5, étape 5c, 2026-11-09 ; décisions de l'utilisateur : une boîte « Vidéo » à part, une liste de qualités simple avec le détail de tous les formats) ; essai de l'interface fait par l'utilisateur le même jour sur la variante d'essai (outils installés depuis la boîte, vidéo Framatube avec son sous-titre anglais), validé :
  - **Boîte « Ajouter une vidéo »** (bouton « Vidéo », présent si le moteur annonce la capacité `media`) : adresse de la page, « Analyser » (`media.probe`), puis titre, durée et site ; qualité : « Meilleure qualité » (sélecteur du moteur), chaque hauteur d'image (`bv*[height<=H]+ba/b[height<=H]`) et « Son seul » (`ba`), avec une taille estimée (image et meilleur son) ; « Tous les formats » montre le tableau des formats, dont un se choisit précisément (une image seule reçoit le meilleur son, `<id>+ba`) ; conteneur ; sous-titres cochés par langue (noms des langues dans celle de l'interface). Les formats protégés (DRM) sont montrés grisés, jamais choisis ; une vidéo entièrement protégée n'est pas proposée. Une adresse changée après l'analyse est analysée de nouveau avant l'ajout.
  - **Outils** : sans yt-dlp ni ffmpeg, la boîte dit pourquoi et propose de les installer (sources, vérification, près de 400 Mo installés) ; l'installation ne part que sur ce clic, qui vaut accord (section 7.17), puis la page est analysée. Les Options ont une rubrique « Vidéo » : versions et origine des outils, installation ou réinstallation, et la mise à jour automatique de yt-dlp (`media.autoUpdate`).
  - **Suivi** : l'état « Finition » pendant l'assemblage ; un avertissement `SUBTITLES_MISSING` ou `YTDLP_UPDATE_FAILED` s'affiche dans la barre d'erreur, `YTDLP_UPDATED` est seulement annoncé aux lecteurs d'écran. Codes `TOOLS_MISSING` et `MERGE_FAILED` nommés.
  - **Propriétés** : l'adresse est celle de la page ; ni « Déplacer/Renommer », ni « Actualiser l'adresse », ni empreinte pour une vidéo, ce que le moteur refuse aussi désormais (section 6.5).
- **Application de bureau, miroirs** (phase 6, étape 1c, 2026-11-11 ; décisions de l'utilisateur : pas de retrait d'un miroir pour l'instant, source de chaque segment montrée) ; essai de l'interface fait par l'utilisateur le même jour sur la variante d'essai (image d'installation Debian 13.7 sur trois miroirs officiels et un faux, l'image ARM, écarté ; miroir ajouté depuis les Propriétés, refus dans « Ajouter »), validé :
  - **Ajout** : la boîte « Ajouter » a un champ « Miroirs (facultatif) », un par ligne, pour une seule adresse HTTP (refusé avec plusieurs adresses, un lien magnet ou un `.torrent`, une adresse qui n'est pas `http(s)://`, une adresse donnée deux fois, plus de 16 sources). Les miroirs partent avec la référence dans `sources[]` de `task.add`.
  - **Propriétés** : section « Sources » quand la tâche en a plusieurs : « Principale » ou « Miroir N » (rang), adresse, état (« À vérifier », « Bonne », « Lente », « Écartée », avec la raison d'un refus ou du dernier échec, texte du moteur) et débit moyen. Champ « Ajouter un miroir » (`task.addSource`) pour un fichier HTTP pas encore fini ni annulé ; un refus du moteur s'affiche dans la barre d'erreur. Le tableau « Segments et connexions » gagne une colonne « Source » (`sourceId` des segments reçus), montrée seulement s'il y a plusieurs sources. Aucun retrait de miroir : le moteur écarte d'office les mauvais et ne donne plus de segments aux lents.
- **Application de bureau, analyse de liens** (phase 6, étape 2b, 2026-11-11 ; section 7.18) : bouton « Liens » (si le moteur annonce `linkscan`) et boîte « Analyser des liens ». Une seule adresse qui n'a pas l'air d'un fichier est une page lue par le moteur ; tout autre contenu est un texte dont on relève les adresses ; « Importer une liste (texte, CSV)… » lit un fichier choisi (4 Mio au plus, toute extension) et l'analyse comme un texte. Filtres (extensions, taille minimale et maximale, expression régulière sur l'adresse, « Montrer aussi les pages et les liens sans extension ») appliqués à chaque frappe ; tailles lues par lots de 50, 500 au plus par analyse ; cases à cocher, « Tout cocher » et « Tout décocher » sur les liens montrés ; 1 000 lignes affichées au plus, tous les liens cochés ajoutés (`task.addBatch`, 1 000 par appel) dans le dossier des téléchargements. Les liens refusés restent dans la boîte, seuls cochés, avec la raison du premier refus.
- **Application de bureau, Metalink** (phase 6, étape 1d, 2026-11-11 ; essai de l'interface fait par l'utilisateur le même jour sur la variante d'essai, validé : Metalink local de plusieurs fichiers avec choix et fichiers écartés, refus d'une empreinte ou d'un miroir à côté, Metalink openSUSE par son adresse, formats 4 et 3) : la boîte « Ajouter » prend des fichiers `.meta4` et `.metalink` choisis, glissés sur la fenêtre ou ouverts avec l'application (« Ouvrir avec… », pas d'association), et des adresses de Metalink collées parmi les adresses. Chaque Metalink montre ses fichiers à cocher (tous cochés) et ceux que le moteur écarte, avec la raison ; une adresse de Metalink est lue d'abord, et revient dans la boîte pour le choix si elle décrit plusieurs fichiers ou en écarte. Ni empreinte ni miroir à donner à côté d'un Metalink. Un fichier refusé laisse son Metalink dans la boîte, ce fichier seul coché.
- **Ligne de commande, torrents et vidéo** (phase 5, étape 5a, 2026-11-09 ; décisions de l'utilisateur : groupe `auro video`, commande générique `auro config`) :
  - `auro add` et `auro get` prennent aussi un lien magnet ou un fichier `.torrent` local (envoyé en base64, 16 Mio au plus) ; `auro files <tâche> [--select 0,2-4]` montre les fichiers d'un torrent et choisit ceux à télécharger ; `auro limit --upload` règle la limite d'envoi.
  - `auro video tools [--install [--force]]` montre ou installe yt-dlp et ffmpeg (la commande vaut accord de l'utilisateur) ; `auro video info <page>` décrit la page (formats, sous-titres) ; `auro video add` et `auro video get <page> [-f format] [--container] [--subs fr,en]` ajoutent une vidéo, `get` la suivant jusqu'au bout.
  - `auro config [clé] [valeur]` lit et change tous les réglages du protocole ; la valeur est tapée (`true`, `oui`, un nombre, `none` pour retirer la valeur, sinon un texte), le moteur vérifie le type attendu.
  - Le suivi (`get`, `watch`) compte la fusion d'une vidéo dans le téléchargement, s'arrête au partage d'un torrent (reçu en entier), montre les pairs et l'envoi d'un torrent, et dit les sous-titres manquants. `auro list` marque les torrents et les vidéos.
- **Sans interface** : l'option `--headless` lance le moteur seul, pour un NAS ou un serveur, avec une image Docker et une unité `systemd` utilisateur. L'interface web est servie par le moteur lui-même.
- **Windows** : installateur signé et version portable.
- **Linux** : AppImage et paquet `.deb` ; Flatpak à étudier, car son bac à sable complique la messagerie native avec le navigateur.
- **Premiers paquets** (décisions du 2026-11-07, phase 2), construits par `apps/desktop/bundle.ps1` et `bundle.sh` ; non signés, la signature vient avec la version 1 (phase 7) :
  - Windows : installateur NSIS seul (le MSI, qui s'installe pour toute la machine, viendra avec la version 1), pour l'utilisateur courant, sans élévation. Dossier par défaut : `%LOCALAPPDATA%\AuroDownloader`, choix du modèle de Tauri ; c'est aussi le dossier des données du moteur, et chacun n'y touche qu'à ses propres fichiers.
  - Linux : `.deb` et AppImage.
  - Contenu : l'application, `aurod` et `auro`, côte à côte ; l'application lance le `aurod` voisin.
  - Avant d'installer, de mettre à jour ou de désinstaller : l'application est fermée (sinon elle relancerait le moteur), puis le moteur arrêté proprement (`auro engine stop`) ; il n'est forcé que s'il tourne encore depuis le dossier du programme.
  - Désinstallation : la base, son journal et les préférences sont conservés ; une case « supprimer les données », décochée par défaut, les efface avec le cache de la page. Les fichiers téléchargés ne sont jamais touchés. Le démarrage avec la session est retiré.
- **Android** : F-Droid et APK signé.

## 5. Modèle de données

Tout l'état durable tient dans une base SQLite unique, dont le moteur est le seul écrivain. Elle contient 19 tables (14 à l'origine, plus `file_moves` et `site_logins` depuis le 2026-11-08, `remote_tokens`, `remote_sessions` et `name_claims` depuis le 2026-11-11) ; c'est elle qui rend possible la reprise après un arrêt brutal (N-04, N-05).

### 5.1 Conventions

- Mode WAL, clés étrangères activées, `synchronous = NORMAL`.
- Identifiants : UUID v7 en texte (triables par date de création), générés par le moteur.
- Dates : millisecondes Unix en UTC, entier.
- Tailles et débits : octets et octets par seconde, entiers de 64 bits.
- Les chemins sont stockés tels que le système les fournit ; sur Android, `dest_dir` peut être une URI du sélecteur de stockage.
- Aucun secret en base : mots de passe, jetons et cookies sensibles sont rangés dans le coffre du système (Gestionnaire d'identification sous Windows, Secret Service sous Linux, Keystore sous Android) ; la base ne garde qu'une référence.
  - Mise en œuvre (module `secrets`, depuis le 2026-11-07) : les en-têtes `Cookie`, `Authorization` et `Proxy-Authorization` d'une tâche vont au coffre ; `task_headers.secret_ref` en garde la référence. Depuis le 2026-11-08, les mots de passe des identifiants de sites aussi (`site_logins.secret_ref`, section 7.14).
  - Windows : identifiants génériques de l'utilisateur, gardés sur la machine (`CRED_PERSIST_LOCAL_MACHINE`), nommés `AuroDownloader:<espace>:<référence>:<n>` ; un secret de plus de 2 560 octets est réparti sur plusieurs.
  - Linux : élément de la collection par défaut, reconnu par ses attributs (`application`, `namespace`, `id`). Le moteur ne déverrouille jamais un trousseau : verrouillé, c'est une erreur temporaire. Sans Secret Service à la première utilisation (serveur sans session graphique, WSL), le moteur garde les secrets en mémoire, perdus à son arrêt, et le dit dans son journal. Décision du 2026-11-07, à confirmer.
  - Android : la mémoire, jusqu'au Keystore (phase 4).
  - Chaque base a son espace de noms (`vault.namespace`) : un moteur d'essai ne touche pas aux secrets du moteur réel.
  - Ordre : le secret est écrit avant sa référence, et la référence effacée avant le secret ; au démarrage, le moteur efface les secrets de son espace sans référence en base (arrêt brutal entre les deux).
  - Fin de vie : les secrets d'une tâche sont effacés quand elle se termine (`completed`) ou quand elle est retirée. Une tâche annulée garde les siens jusqu'à son retrait.
  - Un secret absent du coffre au départ d'une tâche (coffre en mémoire après un redémarrage) : l'en-tête n'est pas envoyé, et le serveur dira s'il en avait besoin. Un coffre qui ne répond pas : échec temporaire `SECRET_UNAVAILABLE` (section 6.8).

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
-- Clés en usage : "limits.downloadBps" (F-15) ; "downloads.dir" (depuis le 2026-11-07) :
-- dossier des ajouts sans destDir, chemin absolu ou null (dossier « Téléchargements »
-- du système) ; "vault.namespace" (depuis le 2026-11-07, interne, absent de
-- settings.get) : espace de noms des secrets de cette base dans le coffre (section 5.1) ;
-- "downloads.preallocate" (depuis le 2026-11-08) : réserver la place des fichiers au
-- départ, booléen, vrai s'il est absent (section 7.6 bis) ;
-- "downloads.neighborChecksum" (depuis le 2026-11-08) : chercher l'empreinte d'un fichier
-- dans ses fichiers voisins, booléen, vrai s'il est absent (section 7.9).

CREATE TABLE schedules (
  id         TEXT PRIMARY KEY,
  name       TEXT NOT NULL,
  rules_json TEXT NOT NULL,   -- plages horaires, jours, action de fin
  enabled    INTEGER NOT NULL DEFAULT 1
);
-- rules_json (depuis le 2026-11-07, format du protocole, section 7.11) :
--   {"windows": [{"days": [1,2,3,4,5], "start": "23:00", "end": "07:00"}],
--    "onDone": "none" | "sleep" | "shutdown"}

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
  checksum_source TEXT,          -- fichier voisin où le moteur a lu l'empreinte ; NULL : celle de
                                 -- l'utilisateur (section 7.9, migration 5, depuis le 2026-11-08)
  public_only     INTEGER NOT NULL DEFAULT 0 CHECK (public_only IN (0,1)),  -- ajoutée par jeton :
                                 -- réseau public seulement (section 7.19, migration 8)
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
-- Migration 0002 (2026-11-07) : clé d'idempotence de task.add (section 6.1), unique
-- parmi les tâches présentes ; retirer la tâche la libère.
ALTER TABLE tasks ADD COLUMN idempotency_key TEXT;
CREATE UNIQUE INDEX tasks_idempotency_key ON tasks (idempotency_key)
    WHERE idempotency_key IS NOT NULL;
-- Migration 0003 (2026-11-08) : « Déplacer/Renommer » (section 7.12). Nom choisi par
-- l'utilisateur, que la préparation prend au lieu de celui du serveur ; NULL : celui du
-- serveur. Il survit au retéléchargement.
ALTER TABLE tasks ADD COLUMN requested_name TEXT;

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

-- Migration 0003 (2026-11-08) : journal d'un déplacement de fichier en cours (section
-- 7.12), écrit avant de toucher au disque ; repris au démarrage après un arrêt.
CREATE TABLE file_moves (
  task_id    TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
  from_path  TEXT NOT NULL,
  to_path    TEXT NOT NULL,
  temp_path  TEXT,              -- copie vers un autre volume, avant son renommage
  dest_dir   TEXT NOT NULL,     -- emplacement de la tâche une fois le fichier déplacé
  file_name  TEXT NOT NULL,
  phase      TEXT NOT NULL CHECK (phase IN ('moving', 'moved')),
  started_at INTEGER NOT NULL
);

-- Migration 0004 (2026-11-08) : identifiants de sites (section 7.14).
CREATE TABLE site_logins (
  id         TEXT PRIMARY KEY,
  site       TEXT NOT NULL UNIQUE,  -- [protocole://]hôte[:port][chemin], normalisé
  username   TEXT NOT NULL,
  secret_ref TEXT NOT NULL,         -- mot de passe, au coffre
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
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

-- Migration 0006 (2026-11-09, section 7.17, étape 4c) : un flux direct d'une vidéo se
-- télécharge comme une tâche HTTP interne, rattachée à sa vidéo ; le planificateur ne la
-- lance jamais, les listes, les comptes et l'historique ne la montrent pas.
ALTER TABLE tasks ADD COLUMN parent_id TEXT REFERENCES tasks(id) ON DELETE CASCADE;
CREATE INDEX tasks_parent ON tasks(parent_id) WHERE parent_id IS NOT NULL;
-- Flux retenus d'une vidéo, dans l'ordre de la fusion. `child_id` : sa tâche HTTP interne
-- (flux direct) ; NULL pour un flux fragmenté (yt-dlp). `done` : fichier du flux complet
-- et vidé sur disque. `merge_container` de media_jobs devient le conteneur retenu.
CREATE TABLE media_streams (
  task_id   TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  position  INTEGER NOT NULL,
  format_id TEXT NOT NULL,
  protocol  TEXT NOT NULL,
  ext       TEXT NOT NULL,
  has_video INTEGER NOT NULL CHECK (has_video IN (0, 1)),
  has_audio INTEGER NOT NULL CHECK (has_audio IN (0, 1)),
  file_name TEXT NOT NULL,
  size      INTEGER,
  child_id  TEXT REFERENCES tasks(id) ON DELETE SET NULL,
  done      INTEGER NOT NULL DEFAULT 0 CHECK (done IN (0, 1)),
  PRIMARY KEY (task_id, position)
);
-- Migration 0007 (2026-11-09, section 7.17, étape 4e, F-24) : un sous-titre est une ligne
-- de media_streams après les flux, avec sa langue ; téléchargé par une tâche HTTP interne
-- comme un flux direct, gardé à côté de la vidéo, jamais fusionné. NULL : flux à fusionner.
ALTER TABLE media_streams ADD COLUMN language TEXT;

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

-- Migration 0008 (2026-11-11, section 7.19, F-39) : jetons et sessions de l'interface
-- distante. Empreinte SHA-256 seulement, jamais le jeton ni la valeur du cookie.
CREATE TABLE remote_tokens (
  id           TEXT PRIMARY KEY,
  name         TEXT NOT NULL,
  scope        TEXT NOT NULL CHECK (scope IN ('read','add','control')),  -- jamais admin
  digest       BLOB NOT NULL UNIQUE,
  created_at   INTEGER NOT NULL,
  last_used_at INTEGER
);
CREATE TABLE remote_sessions (
  digest     BLOB PRIMARY KEY,
  token_id   TEXT NOT NULL REFERENCES remote_tokens (id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
);
CREATE INDEX remote_sessions_by_token ON remote_sessions (token_id);

-- Migration 0009 (2026-11-11, section 5.6, règle 7) : noms réservés sur le disque, pas
-- encore enregistrés dans leur tâche ; effacés avec l'enregistrement, repris au démarrage.
CREATE TABLE name_claims (
  dir        TEXT NOT NULL,
  name       TEXT NOT NULL,
  kind       TEXT NOT NULL CHECK (kind IN ('part','dir','file')),
  task_id    TEXT NOT NULL,          -- sans clé étrangère : survit au retrait de la tâche
  created_at INTEGER NOT NULL,
  PRIMARY KEY (dir, name)
);
CREATE INDEX name_claims_by_task ON name_claims (task_id);

-- Migration 0010 (2026-11-12, section 7.20, F-41) : plugins d'hébergeurs installés ;
-- manifeste, empreinte SHA-256 du fichier (gardé sous plugins/<empreinte>.wasm à côté de
-- la base, revérifié à chaque chargement) et permissions acceptées (listes JSON).
CREATE TABLE plugins (
  id           TEXT PRIMARY KEY,
  name         TEXT NOT NULL,
  version      TEXT NOT NULL,
  author       TEXT,
  description  TEXT,
  digest       TEXT NOT NULL,
  sites        TEXT NOT NULL,
  fetch        TEXT NOT NULL,
  enabled      INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
  installed_at INTEGER NOT NULL
);
-- Tâche dont le lien vient d'un plugin : page d'origine et plugin, pour un lien neuf.
ALTER TABLE tasks ADD COLUMN plugin_page TEXT;
ALTER TABLE tasks ADD COLUMN plugin_id TEXT;

-- Migration 0011 (2026-11-16, section 7.21, F-32 et F-34) : règles de rangement. Nom
-- d'une règle ; tâche ajoutée sans dossier choisi : le dossier des téléchargements
-- d'alors, base des dossiers relatifs des règles (NULL : dossier choisi ou déplacé) ;
-- règle retenue à la préparation (sa commande de fin s'exécute à la fin).
ALTER TABLE rules ADD COLUMN name TEXT NOT NULL DEFAULT '';
ALTER TABLE tasks ADD COLUMN rule_base TEXT;
ALTER TABLE tasks ADD COLUMN rule_id TEXT;

-- Migration 0012 (2026-11-17, section 7.9, F-12) : empreintes de morceaux d'un Metalink,
-- mises bout à bout en hexadécimal ; tours de reprise depuis la dernière relance (3 au
-- plus) et morceaux retéléchargés en tout ; morceaux faux envoyés par une source (à 2,
-- elle est écartée pour sa tâche).
CREATE TABLE task_pieces (
  task_id TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
  algo    TEXT NOT NULL CHECK (algo IN ('sha256', 'sha1', 'md5')),
  length  INTEGER NOT NULL CHECK (length > 0),
  hashes  TEXT NOT NULL,
  rounds  INTEGER NOT NULL DEFAULT 0,
  redone  INTEGER NOT NULL DEFAULT 0
);
ALTER TABLE sources ADD COLUMN bad_pieces INTEGER NOT NULL DEFAULT 0;

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
| `active` | Des connexions transfèrent des données | `paused`, `retry_wait`, `verifying`, `seeding`, `post_processing` (fusion des flux d'une vidéo, depuis le 2026-11-09, section 7.17), `failed`, `cancelled` |
| `paused` | Arrêtée, avec une cause dans `pause_reason` | `queued`, `cancelled` |
| `retry_wait` | Échec temporaire, nouvelle tentative à `next_retry_at` | `active`, `paused`, `failed`, `cancelled` |
| `verifying` | Vérification de l'empreinte du fichier complet | `active` (morceaux faux à télécharger de nouveau, section 7.9, depuis le 2026-11-17), `paused`, `post_processing`, `completed`, `failed` |
| `post_processing` | Rangement, renommage, extraction, commande externe | `completed`, `failed`, `seeding` (torrent dont la commande de fin a réussi, section 7.21, depuis le 2026-11-16) |

Une tâche `post_processing` interrompue par un arrêt du moteur reprend au démarrage : sa fusion recommence (section 7.17), ou sa commande de fin (section 7.21). Pause et annulation n'y font rien : le fichier est fait, la fusion ou la commande va jusqu'au bout (ou jusqu'à son délai).
| `seeding` | Torrent terminé, partage en cours | `completed`, `cancelled` |
| `completed` | Fichier final en place | état terminal |
| `failed` | Échec définitif, cause dans `error_code` | `queued` (relance manuelle) |
| `cancelled` | Annulée par l'utilisateur | état terminal |

`verifying` → `paused` (depuis le 2026-11-08) : un arrêt demandé pendant le calcul de l'empreinte, ou l'arrêt du moteur, interrompt le calcul ; la tâche en pause le refait depuis le début à sa reprise (section 7.9). Une annulation passe par cette pause, comme pour `active`.

Retéléchargement (`task.restart`, depuis le 2026-11-07) : hors de cette table, qui décrit le cours d'un téléchargement, une tâche `completed`, `failed`, `cancelled`, `paused` (quelle qu'en soit la cause, `file_changed` compris : c'est le « repartir de zéro » de F-13) ou `retry_wait` repart à zéro en `queued`. Sa préparation est oubliée (nom, taille, segments, validateurs des sources), comme ses erreurs et ses tentatives ; son adresse, sa file, ses réglages et ses en-têtes restent. Une tâche `queued` ou `active` est refusée (`INVALID_STATE`).

### 5.6 Règles de cohérence

1. **Ordre d'écriture.** Les octets sont d'abord écrits et vidés sur disque (`fdatasync`), puis seulement le compteur `written_bytes` est enregistré. Après un arrêt brutal, la base n'annonce jamais plus que ce qui est réellement sur le disque.
2. **Couverture exacte.** Les segments d'une tâche couvrent `[0, total_bytes − 1]` sans trou ni chevauchement. Le moteur le vérifie au démarrage et après chaque redécoupage, dans la même transaction que la modification. Une tâche de taille nulle n'a aucun segment ; une tâche de taille inconnue (`total_bytes` NULL) est téléchargée en un seul flux, sans segments, et la règle ne s'applique pas. La taille d'une tâche et ses segments initiaux sont enregistrés dans la même transaction : une taille connue implique des segments, et un flux unique de taille connue en a un seul.
3. **Compteur dérivé.** `tasks.done_bytes` est la somme des `written_bytes` ; il est recalculé au chargement, la somme faisant foi.
4. **Fin de vie.** À la fin d'une tâche, la ligne `history` est écrite avant la purge des segments, dans une seule transaction. Les sources (l'adresse) restent jusqu'au retrait de la tâche, pour la retélécharger (depuis le 2026-11-07 ; elles étaient purgées avec les segments auparavant, et une tâche terminée avant cette date ne peut pas être retéléchargée).
5. **Migrations.** Numérotées, appliquées dans l'ordre et dans une transaction, après copie du fichier de base ; une base plus récente que le moteur empêche le démarrage au lieu d'être modifiée.
. **Noms réservés** (depuis le 2026-11-11). Un nom de fichier se prend en créant sur le disque ce qu'il désignera (`.part` vide, dossier vide d'un torrent à plusieurs fichiers, `.part` d'un sous-titre écrit par le moteur), avant de l'enregistrer dans la tâche, pour que deux tâches ne finissent jamais au même endroit. La réservation est d'abord notée dans `name_claims`, puis créée sur le disque, et effacée dans la transaction qui enregistre le nom (préparation d'une tâche HTTP ou d'un torrent, résolution d'une vidéo, sous-titre). Au démarrage (moteur, ou mode `direct` de la CLI), avant toute exécution, chaque réservation restante désigne ce qu'un arrêt brutal a laissé sans l'enregistrer : effacé (un `.part` seulement s'il est vide, un dossier seulement s'il est vide), puis la réservation. Après la création, le nom est vérifié de nouveau (fichier final apparu, nom enregistré par une autre tâche entre-temps) : pris, il est défait et le nom suivant essayé. Trouvé par le banc des vidéos le 2026-11-11 (un `.part` orphelin, la vidéo sous le nom suivant) ; la fenêtre existait aussi pour les tâches HTTP et les torrents. Module `claims`, `store::claims` ; les bancs `crash_resume` et `crash_daemon` vérifient aussi qu'aucun fichier sans tâche ne reste (`Checker::assert_no_strays`).

## 6. Protocole du moteur

Le moteur parle JSON-RPC 2.0, quel que soit le client ou le transport : application de bureau, ligne de commande, extension, interface web ou application Android. Un client qui sait appeler `task.add` fonctionne donc partout, et l'API publique est la même que celle utilisée par l'interface officielle (principe 4).

### 6.1 Principes

- **JSON-RPC 2.0** : requêtes avec `id`, notifications sans `id`, envois groupés (batch) acceptés.
- **Paramètres nommés** : `params` est toujours un objet, jamais un tableau.
- **Noms de méthodes** : `espace.verbe`, en minuscules et casse mixte (`task.add`, `queue.update`).
- **Dates et tailles** : millisecondes Unix en UTC, octets, octets par seconde, tous en entiers.
- **Évolution** : ajouter un champ optionnel ou une méthode ne casse rien ; supprimer ou renommer exige une nouvelle version majeure du protocole. Les schémas JSON de chaque méthode et notification sont dans `docs/protocol` (depuis le 2026-11-12, phase 7) ; ceux de la version 1 y sont figés (`docs/protocol/v1`), et un test refuse tout changement qui casserait un client de cette version. Un client ignore un champ qu'il ne connaît pas et traite une valeur inconnue (nouvel état, nouveau code d'erreur) comme une valeur générique : une réponse peut gagner des valeurs.
- **Idempotence** : `task.add` accepte une `idempotencyKey` ; rejouer le même appel ne crée pas de doublon (utile pour l'extension qui réessaie). Mise en œuvre (2026-11-07) : une clé de 1 à 256 octets ; un ajout dont la clé est déjà celle d'une tâche présente rend cette tâche, sans rien changer ni annoncer, même si ses autres paramètres diffèrent ; la clé est libérée quand la tâche est retirée.

### 6.2 Transports et authentification

| Transport | Usage | Authentification |
| --- | --- | --- |
| Socket Unix (Linux) ou canal nommé (Windows) | Application de bureau et CLI sur la même machine | Droits du système de fichiers : seul l'utilisateur propriétaire y accède. Le client vérifie aussi, avant d'écrire, que le canal (Windows) ou le dossier du socket (Linux) appartient à l'utilisateur et lui est réservé : un autre compte qui aurait pris le nom le premier ne reçoit rien (revue de sécurité, 2026-11-12) |
| Appel direct dans le processus (FFI) | Android, où le moteur vit dans l'application | Aucune : même processus |
| WebSocket sur `127.0.0.1` | Interface web, outils de développement | Jeton porteur dans l'en-tête `Authorization` ou dans le premier message |
| WebSocket sur le réseau, en TLS | Moteur distant (NAS, serveur) | Jeton porteur, TLS obligatoire dès que l'écoute n'est pas locale |
| HTTP `POST /rpc` | Scripts simples sans notifications | Jeton porteur |
| Messagerie native du navigateur | Extension, via un petit programme relais | Le relais passe par le canal local et s'y limite à la portée `add` (voir 6.9) |

Deux protections s'ajoutent pour toute écoute TCP : le moteur vérifie les en-têtes `Host` et `Origin` et refuse ceux qu'il ne reconnaît pas, ce qui bloque les attaques depuis une page web quelconque visant le port local ; et il n'accepte jamais de connexion sans jeton, même depuis `127.0.0.1`.

Transport local, tel que mis en œuvre (phase 2) :
- **Découpage des messages** : un message JSON par ligne, dans les deux sens, terminé par `\n`. Un message reçu de plus de 24 Mio ferme la connexion (1 Mio jusqu'au 2026-11-10 : un `.torrent` de plus de 768 Kio environ, envoyé en base64 par `task.add`, ne passait pas, alors que le bureau et la CLI en acceptent de 16 Mio ; décision de l'utilisateur pendant l'étape 5d). Le WebSocket portera les mêmes messages, un par trame.
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

Capacités annoncées par ce moteur : `torrent` depuis le 2026-11-09 (section 7.16) ; `media` sous Windows et Linux (section 7.17) ; `linkscan` et `metalink` depuis le 2026-11-11 (sections 7.18 et 7.7) ; avant, aucune.

Aucune plage commune : le moteur répond par l'erreur `PROTOCOL_UNSUPPORTED` et ferme la connexion.

Paramètre facultatif `scope` (depuis le 2026-11-07) : la portée que le client garde pour cette connexion (section 6.9). Elle ne peut que se restreindre ; une portée plus large, ou de côté, est refusée par `UNAUTHORIZED`. La réponse donne la portée de la connexion dans `scope`.

### 6.4 Méthodes

| Méthode | Paramètres principaux | Résultat |
| --- | --- | --- |
| `engine.hello` | `client`, `protocol`, `scope` | Version retenue, capacités, `rev`, `scope` |
| `engine.stats` | aucun | Débit global, tâches actives, espace disque |
| `engine.shutdown` | `graceful` | Arrêt après sauvegarde de l'état |
| `task.add` | `sources[]`, `queueId`, `destDir`, `subDir`, `fileName`, `maxConnections`, `speedLimitBps`, `checksum`, `headers`, `startPaused`, `idempotencyKey`, `plugin` (faux : l'adresse telle quelle même pour un site d'un plugin, section 7.20) | `taskId` |
| `task.addBatch` | `items[]` de paramètres `task.add` (1 000 au plus) | `tasks[]` (`index`, `taskId`), `errors[]` (`index`, `code`, `message`) |
| `task.get` | `taskId`, `include` (`segments`, `sources`, `files`) | Tâche complète ; pour une tâche venue d'un plugin, `plugin` : `page`, `pluginId`, `name` (null si le plugin a été retiré) (section 7.20, depuis le 2026-11-12) ; pour une tâche aux empreintes de morceaux, `pieces` : `count`, `length`, `algo`, `redone` (morceaux retéléchargés), `rounds` (tours depuis la dernière relance) ; chaque source a `badPieces` (section 7.9, depuis le 2026-11-17) |
| `task.list` | `queueId`, `states[]`, `limit`, `cursor` | `items[]`, `nextCursor`, `rev` |
| `task.pause` / `task.resume` | `taskIds[]` | Liste des tâches modifiées |
| `task.cancel` | `taskIds[]`, `deleteFiles` | Liste des tâches modifiées |
| `task.remove` | `taskIds[]`, `deleteFiles` | Nombre de tâches retirées |
| `task.restart` | `taskIds[]`, `deleteFiles` | Tâches reparties de zéro, et refus (`errors`) |
| `task.update` | `taskId`, `maxConnections`, `speedLimitBps`, `destDir`, `fileName`, `url`, `checksum` | Tâche modifiée |
| `task.move` | `taskIds[]`, `queueId`, `position`, `priority` | Liste des tâches modifiées |
| `task.addSource` | `taskId`, `url`, `rank` | `sourceId` |
| `task.setFiles` | `taskId`, `selected[]` (torrent) | Tâche modifiée |
| `metalink.read` | `data` (base64) ou `url` | Fichiers décrits, fichiers écartés (section 7.7) |
| `metalink.add` | `data` ou `url`, `select[]`, `destDir`, `subDir`, `queueId`, `maxConnections`, `speedLimitBps`, `startPaused`, `idempotencyKey` | `tasks[]`, `errors[]` |
| `queue.list` / `queue.create` / `queue.update` / `queue.delete` | `name`, `maxActive`, `speedLimitBps`, `scheduleId` | File ou liste de files |
| `limits.get` / `limits.set` | `downloadBps`, `uploadBps` | Limites globales |
| `schedule.list` / `schedule.set` / `schedule.delete` | `scheduleId`, `name`, `enabled`, `rules` | Planification, avec son ouverture (section 7.11) |
| `login.list` / `login.set` / `login.delete` | `loginId`, `site`, `username`, `password` | Identifiants de sites, sans mot de passe (section 7.14, depuis le 2026-11-08) |
| `token.create` / `token.list` / `token.revoke` | `name`, `scope` (`read`, `add` ou `control`, défaut `control`) ; `tokenId` | Jetons de l'interface distante : `id`, `name`, `scope`, `createdAt`, `lastUsedAt` ; `token` (le jeton lui-même) seulement dans la réponse de `token.create` ; `revoked`. Portée `admin`, donc canal local seulement (section 7.19, depuis le 2026-11-11) |
| `remote.status` | aucun | État de l'interface distante : `enabled`, `listen`, `listening`, `error` (pourquoi elle est fermée alors qu'activée) ; depuis l'étape 3b : `tls`, `certificate` (`fingerprint` SHA-256 en hexadécimal par paires, `expiresAt`, `generated`, `names`, ou null), `urls` (adresses où ouvrir la page), `hosts` et `tlsCert` (réglages de l'écoute en place) ; depuis l'étape 3d : `addresses` (`interface`, `address` : adresses des interfaces où ouvrir l'écoute, hors boucle locale et adresses locales au lien). Portée `admin` (section 7.19, depuis le 2026-11-11) |
| `plugin.inspect` | `data` (fichier `.wasm` en base64, 8 Mio au plus) | Lit et vérifie un plugin sans l'installer : `pluginId`, `name`, `version`, `author`, `description`, `sites`, `fetch`, `digest` (SHA-256), `size`. Portée `admin` (section 7.20, depuis le 2026-11-12) |
| `plugin.install` | `data`, `digest` (l'empreinte que l'utilisateur a acceptée) | Installe le plugin, ou remplace la version installée sous le même identifiant ; refusé si `digest` n'est pas celle du fichier. Rend le plugin : champs de `plugin.inspect` sans `size`, plus `enabled`, `installedAt`. Portée `admin` |
| `plugin.list`, `plugin.remove`, `plugin.enable` | aucun ; `pluginId` ; `pluginId`, `enabled` | Plugins installés (`plugins`) ; retrait, avec son fichier ; activation. Portée `admin` |
| `plugin.match` | `url` | `plugin` : `{pluginId, name}` du plugin activé qui prend en charge cette page, ou null. Portée `add` |
| `plugin.resolve` | `url` | Lance le plugin sur la page : `resolutionId` (gardée 10 minutes), `pluginId`, `pluginName`, `page`, `links` (`url`, `name`, `size`), `logs` ; jamais les cookies du site. Portée `add` |
| `plugin.add` | `resolutionId`, `links` (rangs), `destDir`, `subDir`, `queueId`, `speedLimitBps`, `startPaused` | Une tâche par lien choisi (réseau public seulement, `Referer` et cookies du site, page retenue pour un lien neuf), chacun ajouté ou refusé pour lui-même : `tasks` (`index`, `taskId`), `errors` (`index`, `code`, `message`). Portée `add` |
| `engine.cancelEndAction` | aucun | `cancelled` : une action de fin annoncée a été annulée |
| `proxy.get`, `proxy.set` | aucun ; `mode` (`system`, `none`, `manual`), `url`, `exceptions`, `username`, `password` (absent : gardé ; `null` : retiré ; texte : rangé au coffre) | Proxy des connexions HTTP(S) (F-07, phase 7, depuis le 2026-11-12) : `mode`, `url`, `exceptions`, `username`, `hasPassword` ; jamais le mot de passe. Des réglages refusés ne changent rien ; appliqués aux connexions suivantes, celles en cours finissent par l'ancien chemin. Les torrents n'y passent jamais ; une connexion limitée au réseau public (jeton, plugin) ne suit qu'un proxy SOCKS5, que le moteur interroge avec l'adresse qu'il a résolue et vérifiée, et sort sinon en direct. Portée `admin` |
| `rules.list`, `rules.set` | aucun ; `rules[]` (`ruleId`, `name`, `enabled`, `match` : `categories`, `extensions`, `domains`, `urlRegex`, `nameRegex` ; `actions` : `folder`, `rename`, `command` : `program`, `args`, `timeoutS`) | Règles de rangement et commandes de fin, dans leur ordre (F-32, F-34, section 7.21, depuis le 2026-11-16) : `rules`. `rules.set` remplace toute la liste, vérifiée en entier avant d'en rien enregistrer (`INVALID_PARAMS`). `rules.list` : portée `read`, sans les commandes pour une connexion à jeton (`hasCommand` seulement) ; `rules.set` : portée `admin`, donc canal local seulement |
| `engine.diagnostic` | `maxLogBytes` (2 Mio au plus, et par défaut) | Diagnostic sans adresses ni identifiants (N-12, phase 7, depuis le 2026-11-12) : `report` (version, système, réglages sans chemins ni noms, tâches comptées par état, sorte, code d'erreur et cause de pause, files, planifications, plugins, interface distante et outils sans adresse) et `logs`, les dernières lignes des journaux, nettoyées : adresses réduites à leur protocole, hôtes, chemins (coupés après un dossier connu : `<données>…`), nom de l'utilisateur et jetons retirés. Le client le montre avant de l'enregistrer en archive (`rapport.json`, `journaux.jsonl`). Portée `admin` |
| `engine.setNetwork` | `usable` (booléen) | `usable` : le réseau convient, ou non, aux téléchargements (section 7.15, depuis le 2026-11-09) |
| `linkscan.run` | `text` ou `pageUrl` | `links[]` (`url`, `name`, `ext`, `kind`), `pageUrl` après redirections |
| `linkscan.probe` | `urls[]` (50 au plus) | `items[]` (`url`, `size`, `name`, `kind`, ou `error`) |
| `media.probe` | `pageUrl` (`http` ou `https`) | `title`, `durationS`, `extractor`, `thumbnail`, `subtitles[]` (langues que le moteur sait télécharger, depuis l'étape 4e), `formats[]` : `formatId`, `ext`, `vcodec` et `acodec` (`null` : absent), `width`, `height`, `fps`, `tbr` (kbit/s), `filesize`, `filesizeApprox`, `protocol`, `drm`, `note` (section 7.17, depuis le 2026-11-09) |
| `media.tools` | aucun | État des outils de la vidéo : `ytdlp` et `ffmpeg` (`version`, `origin` : `download` ou `system`, ou `null`), `ready`, `installing`, `error` (section 7.17, depuis le 2026-11-09) |
| `media.installTools` | `force` (booléen, faux par défaut : seulement ce qui manque) | Lance l'installation en arrière-plan et rend l'état aussitôt ; la fin s'annonce par `media.tools`. `CAPABILITY_MISSING` hors de Windows et Linux (section 7.17) |
| `history.list` / `history.clear` | `query`, `before`, `limit`, `cursor` | Entrées d'historique ; nombre d'entrées effacées (section 6.5, depuis le 2026-11-08) |
| `settings.get` / `settings.set` | `keys[]` ou `values` | Réglages (`values`, par clé) ; une connexion à jeton ne voit pas les réglages `remote.*` (étape 3c) ; `logs.level` (`error`, `warn`, `info` par défaut, `debug`) : niveau des journaux, appliqué sans redémarrer (N-12, depuis le 2026-11-12) |
| (paramètre) `subDir` | de `task.add` (donc de `task.addBatch`) et `metalink.add`, depuis l'étape 3c : sous-dossier créé sous la destination, parties séparées par `/` (8 au plus, sans `.` ni `..`, chacune un nom de fichier valide tel quel) ; pour tout téléchargement (HTTP, torrent, vidéo) ; une connexion à jeton reste sous le dossier des téléchargements (`DEST_FORBIDDEN`, liens compris) | |
| `events.subscribe` / `events.unsubscribe` | `topics[]` (`tasks`, `queues`, `progress`), `progressIntervalMs` | Abonnement actif et `rev` au moment de l'abonnement |
| `sync.snapshot` | aucun | État complet (`tasks`, `queues`, `limits`, `settings`, `schedules`, `endAction`) et `rev` |

### 6.5 Notifications du moteur

Un client abonné reçoit ces notifications, sans y répondre. Chacune porte la révision `rev` du moteur au moment de l'événement.

| Notification | Contenu |
| --- | --- |
| `task.added` | Tâche complète |
| `task.updated` | `taskId` et seulement les champs modifiés |
| `task.stateChanged` | `taskId`, ancien et nouvel état, `pauseReason` ou `errorCode` |
| `task.progress` | Lot de `{taskId, doneBytes, totalBytes, bps, connections, etaS}`, regroupé selon `progressIntervalMs` (500 ms par défaut) ; pendant `verifying`, `verifiedBytes` en plus : octets du fichier déjà lus par le calcul de l'empreinte (depuis le 2026-11-08) |
| `task.removed` | `taskId` |
| `queue.updated` | File modifiée |
| `limits.updated` | Limites globales, comme `limits.get` (depuis le 2026-11-07) |
| `settings.updated` | `values` : les réglages changés, comme `settings.get` (depuis le 2026-11-07) |
| `engine.network` | `usable` : le réseau convient, ou non, aux téléchargements (section 7.15, depuis le 2026-11-09 ; sujet `queues`). `sync.snapshot` porte aussi `networkUsable` |
| `engine.torrentInterface` | `up` : l'interface réseau des torrents (`torrent.interface`) est là, ou non (section 7.16, depuis le 2026-11-09 ; sujet `queues`). `sync.snapshot` porte aussi `torrentInterfaceUp` |
| `schedule.updated` | Planification créée, modifiée, ou dont l'ouverture vient de changer, comme `schedule.list` la décrit (depuis le 2026-11-07) |
| `schedule.removed` | `scheduleId` (depuis le 2026-11-07) |
| `engine.endAction` | `pending` : action de fin annoncée (`queueId`, `action`, `at`), ou `null` quand elle est exécutée ou annulée ; `error` : refus du système (depuis le 2026-11-07) |
| `engine.stats` | Débit global, toutes les secondes |
| `engine.warning` | `code`, `message`, `taskId` facultatif (la tâche concernée) (disque presque plein, nouvelle version de yt-dlp disponible ; `SUBTITLES_MISSING` : vidéo terminée sans certains sous-titres ; `EVENTS_LOST` : client trop lent, des notifications ont été perdues, appeler `sync.snapshot`) |

Sujets d'abonnement : `tasks` (`task.added`, `task.stateChanged`, `task.updated`, `task.removed`), `queues` (`queue.updated`, `limits.updated`, `settings.updated`, `schedule.updated`, `schedule.removed`, `engine.endAction`), `progress` (`task.progress`). Dans `task.progress`, `doneBytes` compte les octets reçus, pas forcément vidés sur disque ; `bps` est mesuré sur les 6 dernières secondes, entre les instants où les octets ont augmenté, et décroît jusqu'à zéro quand rien n'arrive plus ; `etaS` en découle (depuis le 2026-11-07 ; d'un envoi à l'autre auparavant, ce qui donnait, avec des octets arrivés par blocs, tantôt zéro, tantôt le double du vrai débit). Le moteur mesure l'avancement toutes les 500 ms : un intervalle plus court n'apporte rien de plus.

État de la mise en œuvre (phase 2) :
- **Méthodes servies** : `engine.hello`, `engine.shutdown`, `engine.cancelEndAction`, `engine.setNetwork`, `task.add`, `task.addSource` (depuis le 2026-11-10), `metalink.read` et `metalink.add` (depuis le 2026-11-11), `task.addBatch`, `linkscan.run` et `linkscan.probe` (depuis le 2026-11-11, section 7.18), `task.get`, `task.list`, `task.pause`, `task.resume`, `task.cancel`, `task.remove`, `task.restart`, `task.update`, `queue.list`, `queue.create`, `queue.update`, `limits.get`, `limits.set`, `settings.get`, `settings.set`, `schedule.list`, `schedule.set`, `schedule.delete`, `login.list`, `login.set`, `login.delete`, `token.create`, `token.list` et `token.revoke` (depuis le 2026-11-11, section 7.19), `remote.status`, `plugin.inspect`, `plugin.install`, `plugin.list`, `plugin.remove`, `plugin.enable`, `plugin.match`, `plugin.resolve` et `plugin.add` (depuis le 2026-11-12, section 7.20), `engine.diagnostic`, `proxy.get` et `proxy.set` (phase 7, depuis le 2026-11-12), `rules.list` et `rules.set` (depuis le 2026-11-16, section 7.21), `history.list`, `history.clear`, `events.subscribe`, `events.unsubscribe` et `sync.snapshot`. Les autres répondent « méthode inconnue » jusqu'à leur arrivée.
- **Dossier des téléchargements** (décision du 2026-11-07) : c'est un réglage du moteur, `downloads.dir`, et non plus une préférence de l'application, pour que l'extension, la CLI et l'interface distante ajoutent au même endroit.
  - `task.add` sans `destDir` va dans ce dossier ; s'il n'est pas réglé, dans le dossier « Téléchargements » du système ; s'il n'y en a aucun (serveur sans session), le refus est `DEST_NOT_WRITABLE`.
  - `settings.get` rend tous les réglages pris en charge, avec la valeur retenue (le dossier du système si aucun n'est enregistré) ; `keys` est ignoré pour l'instant. `settings.set` prend `values` : les clés acceptées sont `downloads.dir`, un dossier existant en chemin absolu, ou `null` pour revenir à celui du système, `downloads.preallocate`, un booléen (section 7.6 bis, depuis le 2026-11-08), et `downloads.neighborChecksum`, un booléen (section 7.9, depuis le 2026-11-08) ; les valeurs sont vérifiées toutes avant d'en changer une ; une autre clé est refusée par `INVALID_PARAMS`, un dossier invalide par `DEST_NOT_WRITABLE`. La réponse est celle de `settings.get`.
  - Un `destDir` donné à `task.add` doit aussi être un dossier existant, en chemin absolu (`DEST_NOT_WRITABLE`).
  - La CLI garde le dossier courant par défaut (`-o .`), comme les outils en ligne de commande.
- **En-têtes d'une tâche** (`headers` de `task.add`, depuis le 2026-11-07) : un objet `{nom: valeur}` de textes, envoyé avec chaque requête de la tâche (préparation et segments).
  - Au plus 32 en-têtes, chacun d'au plus 32 Kio ; noms et valeurs valides en HTTP, sans doublon (casse ignorée).
  - Refusés, car fixés par le moteur ou propres à une connexion : `Host`, `Range`, `If-Range`, `If-Match`, `If-None-Match`, `If-Modified-Since`, `If-Unmodified-Since`, `Accept-Encoding`, `Content-Length`, `Transfer-Encoding`, `Connection`, `Keep-Alive`, `Proxy-Connection`, `TE`, `Trailer`, `Upgrade` et `Expect`.
  - Un refus répond `INVALID_PARAMS`, sans citer de valeur.
  - `Cookie`, `Authorization` et `Proxy-Authorization` vont au coffre du système (section 5.1) ; s'il refuse, `SECRET_UNAVAILABLE`.
  - Une redirection vers un autre hôte n'emporte pas ces trois en-têtes. Le moteur n'ajoute jamais de `Referer` de lui-même : avant le 2026-11-07, après une redirection, la bibliothèque HTTP y mettait l'adresse qui redirige, signature comprise, à l'intention de l'hôte suivant.
  - La portée `add` suffit (relais de l'extension, section 4.5). `task.get` ne rend pas les en-têtes.
- **Torrent** (depuis le 2026-11-09, section 7.16) :
  - `sources[0]` porte soit `url`, soit `torrent`, le contenu d'un fichier `.torrent` en base64, jamais les deux (`INVALID_PARAMS`). Un `url` en `magnet:` fait aussi une tâche torrent. La tâche a pour source son lien magnet, déduit du fichier s'il le faut.
  - Un fichier invalide, un lien magnet sans empreinte BitTorrent v1, ou le même torrent qu'une tâche non terminée : `INVALID_PARAMS`. `headers`, `checksum` et `maxConnections` ne s'appliquent pas à un torrent : `INVALID_PARAMS`.
  - Une tâche, telle que la décrivent `task.get`, `task.list`, `task.added` et `sync.snapshot`, porte `kind` : `http` ou `torrent`.
  - `task.update` refuse `checksum`, `destDir`, `fileName` et `url` pour un torrent (`INVALID_PARAMS`). Son `speedLimitBps` vaut à la prochaine reprise de la tâche : librqbit ne change pas la limite d'un torrent en cours.
  - `task.setFiles` (`taskId`, `selected[]`, les rangs des fichiers choisis ; portée `control`) : seulement pour un torrent préparé (fichiers connus), en file, en cours, en pause, en attente ou en échec ; au moins un fichier, rangs connus (`INVALID_PARAMS` sinon). Rend la tâche, avec sa nouvelle taille (celle des fichiers choisis) ; annoncé comme une préparation (`task.updated`). Une exécution en cours suit le nouveau choix.
  - `task.get` avec `include: ["files"]` ajoute `files[]` : `index`, `path` (chemin dans le torrent, composants nettoyés et séparés par `/`), `length`, `selected`, `doneBytes` (indication ; la taille du fichier pour un fichier choisi, une fois le torrent reçu en entier, depuis l'étape 5a). Vide pour une tâche HTTP.
  - `task.progress` porte, pour un torrent, `uploadedBytes` (cumulés) et `uploadBps`.
  - `limits.set` et `limits.get` : `uploadBps`, limite d'envoi globale ; annoncée par `limits.updated`, avec `downloadBps`. La limite globale de réception vaut aussi pour les torrents, par le limiteur de librqbit, à part de celle du HTTP.
  - Réglages (`settings.get`, `settings.set`) : `torrent.ratioLimit` (nombre positif ou nul, `null` pour sans limite ; 1 par défaut ; `settings.updated`) et `torrent.upnp` (booléen, faux par défaut ; vaut à la prochaine ouverture de la session).
  - `engine.setNetwork` accepte `metered` (booléen, faux par défaut) ; `engine.network` et `sync.snapshot` (`networkMetered`) le rendent (section 7.16).
  - Réglage `torrent.interface` (étape 3d, 2026-11-09) : nom de l'interface réseau des torrents (VPN), 1 à 256 caractères sans caractère de contrôle, ou `null` (aucune, par défaut) ; `settings.updated`. `sync.snapshot` porte `torrentInterfaceUp` (l'interface est là, ou aucune n'est demandée), et la notification `engine.torrentInterface` (`up`, sujet `queues`) en annonce chaque changement (section 7.16).
  - `torrent.interfaces` (portée `admin`, étape 5b, 2026-11-09) : `items`, les noms des interfaces réseau de la machine qui ont une adresse utilisable, hors boucle locale, triés ; pour choisir `torrent.interface`.
- **Vidéo, étape 4a** (2026-11-09, section 7.17) :
  - `media.tools` (portée `read`) et `media.installTools` (portée `admin` : elle télécharge et installe des programmes) ; notification `media.tools` (sujet `queues`), avec les mêmes champs que `media.tools` et `rev`, à chaque début, fin ou échec d'une installation.
  - Réglage `media.autoUpdate` (booléen, vrai par défaut ; `settings.updated`) : mise à jour automatique de yt-dlp.
  - Réglages `remote.enabled` et `remote.allowPrivate` (booléens, faux par défaut, depuis le 2026-11-11, section 7.19) : écoute de l'interface distante ; réseau interne permis aux connexions à jeton.
  - Réglage `remote.listen` (texte `adresse:port`, `127.0.0.1:7380` par défaut, null pour y revenir ; `settings.updated`, depuis le 2026-11-11) : écoute de l'interface distante, qui s'y déplace aussitôt.
  - Réglage `remote.hosts` (liste de noms DNS ou d'adresses IP, 32 au plus, null ou liste vide pour aucun ; `settings.updated`, étape 3b) : noms ajoutés de l'interface distante, acceptés dans `Host` et portés par le certificat fabriqué.
  - Réglages `remote.tlsCert` et `remote.tlsKey` (chemins absolus de fichiers PEM, donnés ensemble, ou tous deux null pour le certificat fabriqué par le moteur ; `settings.updated`, étape 3b) : certificat fourni de l'interface distante, lu à chaque ouverture de l'écoute ; la clé n'est jamais copiée en base.
  - Hors de Windows et Linux, les méthodes de la vidéo rendent `CAPABILITY_MISSING`.
- **Vidéo, étape 4b** (2026-11-09, section 7.17) : `media.probe` (portée `add`) ; erreurs `TOOLS_MISSING` (-32031) et `EXTRACTOR_FAILED` (-32032) ; `engine.warning` porte aussi `YTDLP_UPDATED` et `YTDLP_UPDATE_FAILED`, sujet `queues`.
- **Vidéo, étape 4c** (2026-11-09, section 7.17) : `task.add` accepte `media` à la place de `sources` : `pageUrl`, et facultatifs `format` (sélecteur de yt-dlp, 1 à 200 caractères ; `bv*+ba/b` par défaut), `container` (`mp4`, `mkv`, `webm`), `subtitles` (au plus 20 codes de langue), `title` (nom affiché en attendant la résolution). `media` et `sources` ensemble sont refusés ; sans outils, `TOOLS_MISSING`. La tâche porte `kind: media`. Capacité `media` dans `engine.hello` sous Windows et Linux.
- **Vidéo, étape 4e** (2026-11-09, section 7.17) : `engine.warning` porte `SUBTITLES_MISSING` quand une vidéo se termine sans certains des sous-titres demandés, avec le champ facultatif `taskId` (la tâche concernée ; absent pour un avertissement du moteur entier).
- **Analyse de liens, phase 6, étape 2a** (2026-11-11, section 7.18 ; portée `add`) : `linkscan.run` reçoit `text` (4 Mio au plus : texte collé, liste importée) ou `pageUrl` (page lue par le moteur) et rend `links[]` : `url` (absolue, sans ancre), `name` (nettoyé), `ext` (minuscules, ou `null`), `kind` (`file`, `page` ou `magnet`) ; pour une page, aussi `pageUrl`, son adresse après les redirections. Le paramètre `filters` de l'esquisse d'origine n'existe pas : les clients filtrent eux-mêmes (décision du 2026-11-11, section 7.18). `linkscan.probe` reçoit `urls[]` (1 à 50) et rend pour chacune `size` et `name` (ceux du serveur, `null` si inconnus) et `kind` (`page` si le serveur annonce `text/html` ou XHTML sans nom de fichier, `file` s'il annonce un autre type ou un nom par `Content-Disposition`, `null` s'il ne dit rien ; depuis le 2026-11-11), ou `error` (`code`, `message` : `URL_INVALID`, `SOURCE_UNREACHABLE`). `task.addBatch` reçoit `items[]` (1 à 1 000 paramètres de `task.add`) ; chaque élément est ajouté ou refusé pour lui-même. Une page injoignable : `SOURCE_UNREACHABLE` ; une réponse autre que 200 ou plus de 4 Mio : `INVALID_PARAMS`.
- **Metalink, phase 6, étape 1d** (2026-11-11, section 7.7 ; portée `add`, comme `task.add`) : `metalink.read` reçoit `data` (contenu du fichier en base64) ou `url` (adresse HTTP(S) que le moteur lit lui-même, 4 Mio au plus, sans en-têtes ni identifiants) et rend `format` (`"4"` ou `"3"`), `files[]` (`index`, `name`, `path` avec ses dossiers séparés par `/`, `size` ou `null`, `checksum` ou `null`, `urls[]` dans l'ordre retenu, la première étant la référence, `droppedUrls`) et `ignored[]` (`name`, `reason`) ; lu à une adresse, il rend aussi `data`, pour que l'ajout porte sur ce même document. `metalink.add` reçoit `data` ou `url` et crée une tâche HTTP par fichier choisi (`select[]`, tous sinon) : référence et miroirs, empreinte, nom, sous-dossiers créés sous `destDir` ; `idempotencyKey` devient `<clé>#<rang>` pour chaque fichier. Il rend `tasks[]` (`index`, `path`, `taskId`) et `errors[]` (`index`, `path`, `code`, `message`, le code qu'aurait eu `task.add`) : un fichier refusé n'empêche pas les autres. Un document illisible, sans fichier utilisable, ou une adresse qui ne rend pas de Metalink : `INVALID_PARAMS` ; une adresse injoignable : `SOURCE_UNREACHABLE` ; un rang inconnu dans `select` : `INVALID_PARAMS`, avant tout ajout.
- **Miroirs, phase 6, étape 1a** (2026-11-10, section 7.7) : `task.add` accepte plusieurs `sources` : la première est la référence, les suivantes des miroirs, toutes des adresses HTTP(S) (une même adresse deux fois, un lien magnet, un `.torrent` ou une vidéo avec miroirs : `INVALID_PARAMS`), 16 au plus. `task.addSource` (`taskId`, `url` ; portée `control`) rend `sourceId` ; un miroir d'un autre fichier est refusé par `SOURCE_MISMATCH`, un miroir injoignable par `SOURCE_UNREACHABLE`, une tâche terminée ou annulée par `INVALID_STATE`. `task.get` avec `include: ["sources"]` donne pour chaque source `sourceId`, `url`, `acceptsRanges`, `rank`, `state` (`unknown`, `ok`, `slow`, `failed`), `avgBps`, `failCount` et `lastError`.
- **Vidéo, étape 5c** (2026-11-09) : `task.update` refuse `checksum`, `destDir`, `fileName` et `url` pour une vidéo (`INVALID_PARAMS`), comme pour un torrent : l'empreinte d'un fichier assemblé sur place n'est publiée nulle part, ses sous-titres ne suivraient pas un déplacement, et l'adresse d'une vidéo est celle de sa page, relue à chaque exécution.
- **Paramètres refusés** : un paramètre prévu mais pas encore pris en charge est refusé par `CAPABILITY_MISSING`, et non ignoré en silence. Cela concerne :
  - dans `task.add` : rien depuis la phase 6 (plusieurs sources, des miroirs, depuis le 2026-11-10, section 7.7 ; `fileName` est pris en charge depuis le 2026-11-08, section 7.12 ; `checksum` depuis le 2026-11-08, section 7.9) ; dans `task.addSource`, `rank` ;
  - dans `task.update` : `maxConnections`. Sont pris en charge `speedLimitBps` ; `destDir` et `fileName` (section 7.12) ; `url` (section 7.13, depuis le 2026-11-08) ; `checksum` (section 7.9, depuis le 2026-11-08). Chacun de ces quatre groupes passe par un appel séparé (`INVALID_PARAMS` sinon) : chaque opération est vérifiée en entier avant de rien changer, mais pas leur combinaison ;
  - (`uploadBps` de `limits.set` était refusé jusqu'au 2026-11-09 ; il est pris en charge depuis, avec les torrents.)
- **Limites de débit** (`speedLimitBps`, `downloadBps`) : un entier d'au moins 1, en octets par seconde.
  - Dans `task.update`, `queue.update` et `limits.set`, `null` retire la limite et un paramètre absent la laisse telle quelle.
  - Une nouvelle limite s'applique aussitôt aux tâches en cours (section 7.6).
  - `task.update` annonce `task.updated`, avec `taskId` et les seuls champs modifiés.
  - Un changement de la limite globale est annoncé par `limits.updated`, sujet `queues`, avec une nouvelle révision (depuis le 2026-11-07 ; avant, rien ne l'annonçait). `sync.snapshot` porte les limites dans `limits`.
- **Nom et taille d'une tâche** (depuis le 2026-11-07) : `task.added` les ignore encore,
  la préparation les apprend (section 7.1). Après chaque préparation, le moteur annonce
  `task.updated` avec `fileName`, `displayName` et `totalBytes`, et une nouvelle révision ;
  avec `destDir` aussi depuis le 2026-11-17, puisqu'une règle de rangement choisit le
  dossier à la préparation (section 7.21).
  `displayName` est le nom affiché de la tâche : celui du fichier, ou le titre d'une vidéo
  (depuis le 2026-11-09, étape 5a ; avant, le nom du fichier même pour une vidéo).
  L'annonce peut suivre la fin d'une tâche rapide. Un client qui ouvre le fichier d'une
  tâche prend ce `fileName`, qui peut différer du nom de l'adresse (« nom (1).bin »).
- **Dates d'une tâche** (depuis le 2026-11-07) : une tâche, telle que la décrivent
  `task.get`, `task.list`, `task.added` et `sync.snapshot`, porte `createdAt` et
  `finishedAt` (millisecondes Unix, section 5.1). `finishedAt` vaut `null` tant que la
  tâche n'est pas terminée, en échec ou annulée, et redevient `null` quand elle repart.
  `task.stateChanged` ne la porte pas : un client la date à la réception, et l'instantané
  suivant donne celle du moteur.
- **Détail d'une tâche** (`task.get` avec `include`, depuis le 2026-11-07) :
  - `segments` ajoute `segments[]` : `idx`, `start` et `end` (octets, bornes incluses),
    `doneBytes` et `receiving`. Pour un segment qu'une connexion reçoit en ce moment
    (`receiving` vrai), `doneBytes` compte les octets reçus, sinon ceux vidés sur disque
    au dernier point de contrôle. Un segment en cours de réception porte aussi
    `sourceId`, la source qui l'envoie (miroirs, section 7.7 ; depuis le 2026-11-11) ;
    les autres ne l'ont pas. Un client qui veut suivre la carte des connexions
    rappelle `task.get` (une fois par seconde suffit) ;
  - `sources` ajoute `sources[]` : `url` et `acceptsRanges` (`null` avant la
    préparation). `acceptsRanges` faux signale un téléchargement sans reprise possible
    (F-04). L'adresse peut porter un jeton : un client l'affiche sans la journaliser
    (section 8.2) ;
  - une tâche terminée n'a plus de segments ; ses sources restent depuis le 2026-11-07
    (section 5.6, règle 4 ; ce paragraphe disait le contraire jusqu'au 2026-11-08) ;
  - `files` (torrents) : depuis le 2026-11-09, voir « Torrent » plus haut (avant :
    `CAPABILITY_MISSING`) ; une valeur inconnue : `INVALID_PARAMS`.
- **Retéléchargement** (`task.restart`, depuis le 2026-11-07 ; portée `control`) : section 5.5 pour les états.
  - Le `.part` est supprimé ; avec `deleteFiles`, le fichier final d'une tâche terminée aussi, et le nouveau téléchargement reprend son nom. Sans, l'ancien fichier reste (le moteur n'écrase jamais un fichier) et le nouveau prend un autre nom (« nom (1).ext »).
  - Annonce : `task.updated` avec `fileName`, `totalBytes`, `errorCode` et `errorDetail` à `null` et `doneBytes` à 0, puis `task.stateChanged` vers `queued`.
  - Une tâche sans adresse (terminée avant le 2026-11-07) est refusée par `INVALID_PARAMS`.
  - Les cookies d'une tâche terminée ont été effacés à sa fin (section 5.1) : un fichier qui demande d'être connecté se relance depuis le navigateur.
  - L'application : « Retélécharger… » dans le menu contextuel, avec une case « Remplacer le fichier déjà téléchargé », cochée par défaut (décision du 2026-11-07).
  - Un nom choisi par l'utilisateur (section 7.12) est gardé, contrairement au nom appris du serveur.
- **Déplacer/Renommer** (`task.update` avec `destDir` et `fileName`, depuis le 2026-11-08 ; portée `control`) : section 7.12.
  - Une tâche porte `moving`, vrai pendant une copie vers un autre volume.
  - Une tâche porte `publicOnly` (depuis le 2026-11-11, section 7.19) : ajoutée, ou modifiée (adresse, miroir), par une connexion à jeton ; elle ne joint que le réseau public, pour toute sa vie.
  - Annonce : `task.updated` avec `destDir`, `fileName`, `displayName` et `moving` ; avec `moveError` en plus si une copie échoue (la tâche est restée à sa place).
  - L'application : « Déplacer/Renommer… » dans le menu contextuel ; nom et dossier, l'erreur éventuelle dans la boîte.
- **Empreinte attendue** (`checksum` de `task.add` et de `task.update`, depuis le 2026-11-08 ; portée `add` pour l'ajout, `control` pour la modification) : section 7.9.
  - Forme : `{"algo": "sha256" | "sha1" | "md5", "value": "…"}`, valeur en hexadécimal (64, 40 ou 32 chiffres), majuscules acceptées et rendues en minuscules ; une forme fausse est refusée par `INVALID_PARAMS`.
  - Rendue, elle porte aussi `source` (depuis le 2026-11-08) : le nom du fichier voisin où le moteur l'a lue (« SHA256SUMS », « image.iso.sha256 »), ou `null` pour une empreinte donnée par l'utilisateur. `source` est ignoré à l'entrée : une empreinte donnée par `task.add` ou `task.update` est celle de l'utilisateur.
  - Une tâche, telle que la décrivent `task.get`, `task.list`, `task.added` et `sync.snapshot`, porte `checksum` : cet objet, ou `null`.
  - `task.update` avec `checksum` à `null` la retire. Refusé par `INVALID_STATE` pendant `verifying`, et pour une tâche terminée ou annulée. Annonce : `task.updated` avec `checksum`.
  - Empreinte trouvée dans un fichier voisin (section 7.9) : annoncée par le `task.updated` de la préparation (avec `fileName` et `totalBytes`), qui porte alors `checksum`. Un retéléchargement l'oublie ; son `task.updated` porte `checksum` (celle de l'utilisateur, ou `null`).
  - Les clients acceptent aussi « sha256:… », « sha1:… », « md5:… », ou la valeur seule, dont la longueur donne l'algorithme (`auro add --checksum`, `auro checksum`, champ « Empreinte attendue » de l'application), et envoient la forme du protocole.
- **Actualiser l'adresse** (`task.update` avec `url`, depuis le 2026-11-08 ; portée `control`) : section 7.13. Annonce : `task.updated` avec `displayName`. L'application : « Actualiser l'adresse… » ; elle relance une tâche en échec une fois l'adresse acceptée.
- **Historique** (`history.list`, `history.clear`, F-35, depuis le 2026-11-08) :
  - Une entrée par fin : une tâche terminée a la sienne à sa fin ; une tâche en échec ou annulée, à son retrait de la liste (section 5.6, règle 4), pour ne jamais être à la fois dans la liste et dans l'historique (décision du 2026-11-08, confirmée par l'utilisateur). Une tâche retéléchargée puis terminée de nouveau en a deux.
  - `history.list` rend `items[]`, les plus récentes d'abord : `historyId`, `taskId` (la tâche peut avoir été retirée), `displayName`, `finalPath` (tâche terminée ; il suit « Déplacer/Renommer »), `totalBytes`, `sourceHost` (le nom d'hôte, jamais l'adresse, section 8.2), `finalState`, `sha256` (empreinte SHA-256 vérifiée à la fin, section 7.9, sinon `null`), `finishedAt` (millisecondes Unix).
  - Paramètres : `query`, texte cherché dans le nom, le chemin et l'hôte, sans égard à la casse des lettres ASCII ; `before`, seulement les entrées finies avant cette date (millisecondes Unix) ; `limit` et `cursor`, comme `task.list` : `nextCursor` vaut `null` sur la dernière page.
  - `history.clear` efface tout, ou les entrées finies avant `before` ; il rend `removed`, leur nombre. Ni les tâches ni les fichiers ne sont touchés. Rien n'est annoncé : un client relit l'historique quand il l'affiche.
  - Clients : `auro history [texte] [--csv] [--clear]` ; dans l'application, le bouton « Historique » (recherche, « Exporter en CSV… », « Effacer l'historique… »). L'export de l'application sépare les champs par des points-virgules en français, des virgules en anglais, avec la marque d'ordre des octets UTF-8 (pour les tableurs) ; celui de la CLI, par des virgules (RFC 4180). Décision du 2026-11-08, confirmée par l'utilisateur.
  - Pas encore : la durée de conservation réglable (section 5.6, règle 6) et le mode « sans historique » (section 8.2).
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
| -32013 | `SCHEDULE_NOT_FOUND` | Identifiant de planification inconnu (depuis le 2026-11-07) |
| -32014 | `LOGIN_NOT_FOUND` | Identifiants de site inconnus (depuis le 2026-11-08, section 7.14) |
| -32020 | `URL_INVALID` | Adresse non reconnue ou schéma non pris en charge |
| -32021 | `DEST_NOT_WRITABLE` | Dossier de destination inaccessible en écriture |
| -32022 | `INSUFFICIENT_SPACE` | Espace disque insuffisant pour la taille annoncée |
| -32023 | `SECRET_UNAVAILABLE` | Le coffre du système a refusé un secret de `task.add` (depuis le 2026-11-07) |
| -32024 | `FILE_EXISTS` | Un fichier porte déjà ce nom à la destination (depuis le 2026-11-08, section 7.12) |
| -32025 | `FILE_MISSING` | Le fichier d'une tâche terminée n'existe plus : rien à déplacer (section 7.12) |
| -32026 | `MOVE_FAILED` | Le système a refusé le déplacement : fichier ouvert par un autre programme, droits (section 7.12) |
| -32027 | `SOURCE_MISMATCH` | La nouvelle adresse donne un fichier d'une autre taille (section 7.13) |
| -32028 | `SOURCE_UNREACHABLE` | La nouvelle adresse ne répond pas, ou refuse (section 7.13) |
| -32030 | `CAPABILITY_MISSING` | Fonction non disponible dans ce moteur (torrent, vidéo) |
| -32031 | `TOOLS_MISSING` | yt-dlp ou ffmpeg n'est pas installé : `media.installTools` (depuis le 2026-11-09, section 7.17) |
| -32032 | `EXTRACTOR_FAILED` | yt-dlp n'a pas pu lire la page ; message sans l'adresse (section 7.17) |
| -32029 | `DEST_FORBIDDEN` | Connexion à jeton : dossier hors de celui des téléchargements (section 7.19, depuis le 2026-11-11) |
| -32033 | `ADDRESS_FORBIDDEN` | Connexion à jeton : adresse du réseau interne (écrite telle quelle, ou résolue ainsi), sauf `remote.allowPrivate` (section 7.19, depuis le 2026-11-11) |
| -32034 | `PLUGIN_FAILED` | Un plugin s'est arrêté (limite de calcul, de mémoire, de pile), a rendu une sortie refusée, ou n'a trouvé aucun lien (section 7.20, depuis le 2026-11-12) |
| -32035 | `PLUGIN_NOT_FOUND` | Plugin inconnu, aucun plugin activé pour ce site, ou résolution expirée (section 7.20) |
| -32036 | `PLUGIN_CHOICE` | `task.add` d'une page dont le plugin trouve plusieurs liens : les choisir par `plugin.resolve` et `plugin.add` (section 7.20) |

`INSUFFICIENT_SPACE` n'est pas encore renvoyé : `task.add` ne connaît pas la taille, que seule la préparation apprend (section 7.1). Un manque de place met la tâche en pause, cause `disk_full`, code `DISK_FULL` (sections 6.8 et 7.6 bis).

### 6.8 Codes d'erreur d'une tâche

Quand une tâche échoue, `error_code` indique la cause. Le moteur décide seul s'il réessaie, selon la colonne « Réessai ».

| `error_code` | Réessai | Signification |
| --- | --- | --- |
| `NET_UNREACHABLE` | oui | Réseau coupé ou hôte injoignable |
| `NET_TIMEOUT` | oui | Délai dépassé |
| `HTTP_STATUS` | selon le statut | Réessai pour 408, 429 et 5xx ; échec définitif pour 401, 403, 404 et 410 |
| `TLS_INVALID_CERT` | non | Certificat refusé |
| `TLS_UNAVAILABLE` | non | HTTPS impossible : la vérification des certificats n'est pas initialisée (sous Android, si le moteur tourne sans l'application, qui l'initialise au démarrage ; section 7.5) |
| `RANGE_UNSUPPORTED` | non bloquant | Bascule en un seul flux, la reprise sera impossible |
| `FILE_CHANGED` | non | Le fichier distant a changé ; la tâche attend la décision de l'utilisateur |
| `DISK_FULL` | après libération | Plus d'espace ; la tâche reprend quand l'espace revient |
| `DEST_PERMISSION` | non | Écriture refusée dans le dossier |
| `DISK_IO` | non | Autre erreur de lecture, d'écriture ou de renommage sur le disque |
| `CHECKSUM_MISMATCH` | non | L'empreinte du fichier final diffère de celle attendue |
| `SECRET_UNAVAILABLE` | oui | Le coffre du système n'a pas rendu un secret de la tâche (trousseau verrouillé ; depuis le 2026-11-07) |
| `AUTH_REQUIRED` | non | Le serveur demande une identification (`401` avec `WWW-Authenticate`) et aucun identifiant de site ne convient (depuis le 2026-11-08, section 7.14) |
| `AUTH_FAILED` | non | Le serveur refuse les identifiants du site, ou demande Basic sur une connexion non chiffrée (section 7.14) |
| `INTERNAL` | non | Erreur du moteur lui-même (base illisible, incohérence, panique d'une exécution) ; détail dans `error_detail`. La tâche ne repart qu'à la demande de l'utilisateur |
| `TORRENT_SESSION` | oui | La session des torrents ne démarre pas (port d'écoute pris, réseau) ; depuis le 2026-11-09, section 7.16 |
| `TORRENT_FAILED` | non | Erreur du torrent lui-même : métadonnées invalides, refus de la bibliothèque, même torrent déjà en cours ; depuis le 2026-11-09, section 7.16 |
| `EXTRACTOR_FAILED` | non | yt-dlp n'a pas pu résoudre la page |
| `TOOLS_MISSING` | non | yt-dlp ou ffmpeg n'est pas installé (section 7.17, depuis le 2026-11-09) |
| `MERGE_FAILED` | non | ffmpeg n'a pas pu fusionner les flux, ou ffprobe refuse le fichier produit (section 7.17) |
| `ADDRESS_FORBIDDEN` | non | Tâche ajoutée ou modifiée par une connexion à jeton : une adresse (redirection ou miroir compris) mène au réseau interne (section 7.19, depuis le 2026-11-11) |
| `POSTPROCESS_FAILED` | non | La commande de fin de la règle a rendu un code non nul, dépassé son délai ou n'a pas pu être lancée ; le fichier reste en place, relancer la tâche relance la commande seule (section 7.21, depuis le 2026-11-16) |

### 6.9 Portées des jetons

| Portée | Autorise | Typiquement donnée à |
| --- | --- | --- |
| `read` | Méthodes `*.list`, `*.get`, `engine.stats`, abonnements | Un tableau de bord en lecture seule |
| `add` | `task.add`, `task.addBatch`, `media.probe`, `linkscan.run`, `linkscan.probe`, `metalink.read`, `metalink.add` | L'extension navigateur, un partage Android |
| `control` | Tout ce que fait `add`, plus `task.*` et `queue.*` | L'application de bureau, la CLI, l'interface web |
| `admin` | Tout, dont `settings.set`, `engine.shutdown` et la gestion des jetons | Le propriétaire de la machine |

Mise en œuvre (décision du 2026-11-07, phase 3) :
- **Canal local** : une connexion a d'emblée la portée `admin`, le système réservant le canal à l'utilisateur (section 6.2). Un client peut se restreindre à l'ouverture (`engine.hello`, paramètre `scope`), jamais s'élargir. Le relais de l'extension se limite ainsi à `add`. Aucun jeton : un programme qui pourrait lire celui du relais pourrait aussi bien se connecter lui-même au canal.
- **Jetons** : réservés aux transports réseau (WebSocket, HTTP), avec l'interface distante (F-39, phase 6). Depuis le 2026-11-11 (section 7.19) : un jeton a la portée `read`, `add` ou `control`, jamais `admin` (décision de l'utilisateur) ; une connexion à jeton passe par la poignée restreinte du moteur (dossier des téléchargements, réseau public), et se ferme quand son jeton est révoqué.
- **Portées exigées** :
  - aucune : `engine.hello` ;
  - `read` : `task.get`, `task.list`, `queue.list`, `limits.get`, `settings.get`, `schedule.list`, `history.list`, `events.subscribe`, `events.unsubscribe`, `sync.snapshot`, `engine.stats`, `media.tools` ;
  - `add` : `task.add`, `task.addBatch`, `media.probe`, `linkscan.run`, `linkscan.probe` (depuis le 2026-11-11), `metalink.read` et `metalink.add` (depuis le 2026-11-11) ;
  - `control` : les autres méthodes `task.*` et `queue.*`, `limits.set`, `engine.cancelEndAction`, `engine.setNetwork`, `history.clear` ;
  - `admin` : `engine.shutdown`, `settings.set`, `schedule.set`, `schedule.delete`, et `queue.create` ou `queue.update` qui portent `scheduleId` (une planification peut éteindre la machine) ; `login.list`, `login.set` et `login.delete` (identifiants de sites, section 7.14) ; `media.installTools` (installation de programmes, section 7.17) ; `token.create`, `token.list` et `token.revoke` (jetons, section 7.19).
- `read` et `add` ne se contiennent pas l'une l'autre ; `control` contient les deux ; `admin` contient tout.
- Refus : `UNAUTHORIZED`, avec la portée demandée et celle de la connexion.

### 6.10 Garanties de livraison

- Les messages d'une même connexion arrivent dans l'ordre.
- Les changements d'état ne sont jamais abandonnés ; seules les notifications `task.progress` peuvent être regroupées ou sautées si le client est lent.
- La révision `rev` augmente de 1 à chaque changement d'état ; les notifications de progression portent la révision courante sans l'augmenter, si bien qu'en sauter une ne crée aucun trou. Un client qui constate un trou dans les révisions, ou qui se reconnecte, appelle `sync.snapshot` et repart de cet état complet. La révision repart de 0 à chaque démarrage du moteur.
- Pour ne rien manquer, un client s'abonne d'abord, puis appelle `sync.snapshot`. Il ignore ensuite les notifications de révision inférieure ou égale à celle de l'instantané.
- Le moteur ne suppose jamais qu'un client est présent : les téléchargements continuent sans interface ouverte.

## 7. Algorithmes clés

Les mécanismes ci-dessous décident de la vitesse et de la fiabilité du moteur. Les valeurs données sont des points de départ à régler par benchmark, pas des vérités.

### 7.1 Préparation d'une tâche

1. Le moteur envoie une requête `HEAD`. Si elle annonce `Accept-Ranges: bytes`, une taille (`Content-Length`) et un validateur (`ETag` ou `Last-Modified`), il s'arrête là (le validateur est exigé depuis le 2026-11-09, décision de l'utilisateur : l'API de Wikimedia Commons annonce les plages sans validateur et les ignore) ; `Accept-Ranges: none` suffit aussi, pour un seul flux. Sinon, ou si le serveur refuse le `HEAD`, il envoie un `GET` avec `Range: bytes=0-0` et lit `Content-Range` dans la réponse, sans lire le corps.
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
- Sur une réponse 429 ou 503, ou une connexion refusée, il revient aux connexions que le serveur accepte, cesse d'augmenter, et mémorise ce nombre comme plafond pour cet hôte, pendant la session (règle du 2026-11-07, détaillée plus bas ; auparavant, il divisait par deux à chaque refus).

Cette boucle est volontairement proche du démarrage de TCP (le nombre double à chaque fenêtre tant que le débit suit) : prudente quand le serveur résiste, rapide quand il répond. Une première version ajoutait une seule connexion par fenêtre ; elle laissait le mode automatique loin derrière sur les téléchargements courts face à un serveur qui bride chaque connexion (`docs/MESURES.md`).

Précisions de mise en œuvre :
- La première mesure double toujours le nombre de connexions, car il n'y a pas encore de débit de référence. Une fenêtre sans aucun octet reçu ne permet aucune conclusion : elle ne change rien.
- Chaque connexion ajoutée prend la moitié du plus gros segment en cours (section 7.2).
- Quand une connexion reçoit un 429 ou un 503, ou voit sa connexion refusée, alors que d'autres connexions tournent encore, la tâche n'échoue pas. Le segment de cette connexion attend son tour à partir de ses octets enregistrés, et les autres connexions continuent.
- Refus du serveur (depuis le 2026-11-07) :
  - Une salve de refus (montée de 8 à 16 face à un serveur qui en accepte 8) ramène aux connexions encore actives, jamais sous le plus grand nombre que le serveur a accepté en même temps pendant l'exécution. Avant cette règle, chaque refus de la salve divisait par deux : 16, 8, 4, 2 puis 1 connexion, et la fin du téléchargement se faisait sur une seule (constaté le 2026-11-07).
  - Un serveur compte encore un moment une connexion qui vient de se fermer : la connexion qui la remplace peut être refusée. Ce refus ne fait pas non plus descendre sous ce nombre.
  - Après un refus, aucune connexion ne s'ouvre pendant 1 s, ou pendant le délai de `Retry-After` (au plus 30 s). Les refus qui arrivent pendant cette pause sont du même épisode.
  - Si trois épisodes de refus se suivent sans qu'aucune connexion ne finisse son segment entre eux, le serveur a baissé sa limite : le moteur descend aux connexions encore actives.
  - Un refus quand aucune autre connexion ne tourne est traité de même (pause, puis nouvel essai), jusqu'au troisième épisode.
- Au-delà, sur la dernière connexion de la tâche, l'échec suit la section 7.8 (nouvelle tentative, `Retry-After`).
- Le plafond par hôte vit en mémoire, pour la durée du processus du moteur.
- Un nombre de connexions fixé par l'utilisateur (`maxConnections`) n'est jamais ajusté.

### 7.4 Reprise après arrêt

Au démarrage, pour chaque tâche `active` ou `retry_wait` :

1. Le moteur ouvre le `.part` et vérifie que sa taille couvre tous les octets que `segments` déclare écrits. Sinon, les segments concernés repassent à `pending`, avec `written_bytes` remis à zéro.
2. Il vérifie les invariants de couverture (section 5.6).
3. Il rouvre les connexions avec `Range` et `If-Range` portant l'`ETag` mémorisé (ou la date `Last-Modified`). Seule une `ETag` forte et bien formée (entre guillemets) sert telle quelle. Une `ETag` sans guillemets laisse la place à la date ; sans date, elle part entre guillemets (`net::headers::if_range_validator`). Depuis le 2026-11-09 : `upload.wikimedia.org` envoie ses `ETag` sans guillemets mais n'accepte dans `If-Range` que la forme conforme. Renvoyée telle quelle, elle lui faisait rendre le fichier entier, que le moteur prenait pour un fichier changé.
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
  - **HTTPS** : `rustls-platform-verifier` exige sous Android une initialisation par l'application (JNI et `Context`). Sans elle, il panique au premier certificat et refuse toute autorité ajoutée. Le moteur refuse donc HTTPS sous Android par l'erreur `TLS_UNAVAILABLE` (section 6.8), même après une redirection. Depuis le 2026-11-08, l'application fait cette initialisation au démarrage du moteur (`auro-android`, section 4.4), puis l'annonce au moteur (`net::platform_tls_ready`) : HTTPS est permis ; sans elle (tests du moteur sur l'appareil), le refus demeure.
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
- **Délai de lecture.** Les 30 s sans octet reçu qui font échouer une lecture (`NET_TIMEOUT`) ne courent que pendant l'attente du serveur : de l'envoi de la requête à ses en-têtes, puis de chaque demande de morceau à son arrivée. L'attente d'un seau n'y compte pas. Avant le 2026-11-07, le délai du client HTTP courait aussi entre deux lectures : sous une limite basse partagée par plusieurs tâches, des connexions échouaient alors qu'aucun serveur n'était en cause.
- **Changement de limite.** Il réveille les connexions qui attendent. Une dette reste due, sauf si la limite disparaît.
- **Coût.** Un seau sans limite ne coûte qu'une lecture atomique par morceau.
- **Où vivent les seaux.** Le planificateur tient le seau global, un seau par file et un par tâche en cours ; chaque exécution reçoit les trois. Le mode `direct` de la CLI n'en a aucun.
- **Où vivent les limites.** La limite globale est enregistrée dans `settings` (clé `limits.downloadBps`) ; celles des files et des tâches, dans leur colonne `speed_limit_bps`. Une limite vaut au moins 1 octet par seconde ; `null` la retire.

### 7.6 bis Espace disque (F-18)

- **Avant le départ.** Le moteur vérifie qu'il reste de quoi finir le fichier, plus une réserve de 64 Mio laissée libre (section 7.1, étape 5). Pour une tâche de taille inconnue, seule la réserve compte.
  - La place qu'il reste à prendre est la taille totale moins la place que le `.part` occupe déjà. Sous Unix, on compte ses blocs, car un fichier creux occupe moins que sa longueur ; ailleurs, sa longueur.
  - Sans préallocation, cette estimation est prudente sous Windows et optimiste sous Linux. Le contrôle à l'écriture reste donc nécessaire.
- **Réservation de la place** (F-10, depuis le 2026-11-08 ; réglage `downloads.preallocate`, activé par défaut, décision de l'utilisateur du même jour) : après le contrôle précédent, le moteur réserve la place de tout le fichier, dont le `.part` prend sa taille finale.
  - Linux : `fallocate`, qui attribue les blocs sans écrire de zéros ; un système de fichiers qui ne le permet pas est laissé tel quel (pas de `posix_fallocate`, qui écrirait des zéros). Windows : seulement sur NTFS et ReFS, où allonger le fichier réserve ses clusters ; rien sur FAT et exFAT, où cela pourrait écrire des zéros pendant des minutes. Ailleurs : rien.
  - Un manque de place se voit alors avant le premier octet reçu : pause `disk_full`, comme plus bas, et rien n'est demandé au serveur. Il se voit aussi à chaque reprise, la réservation étant refaite (sans coût quand elle est déjà faite).
  - La réservation suit la reprise du `.part` (section 7.4, étape 2), qui juge les octets perdus d'après sa longueur ; les compteurs, eux, ne changent pas (section 5.6, règle 1).
  - Tâche de taille inconnue : rien à réserver.
  - Effet de bord : déplacer vers un autre volume un `.part` réservé copie toute sa taille (section 7.12).
  - Le contrôle à l'écriture reste nécessaire : sans réservation, ou si un autre programme prend la réserve.
- **Pendant le téléchargement.** À chaque point de contrôle, le moteur vérifie que la réserve est encore libre. C'est le cas d'un autre programme, ou d'une autre tâche, qui remplit le disque.
- **À l'écriture.** Une écriture qui échoue faute de place (`ENOSPC`, `ERROR_DISK_FULL`) a le même effet.
- **Arrêt.** Dans tous ces cas, la tâche s'arrête comme sur une pause : chaque connexion vide ce qu'elle a écrit, et les compteurs sont enregistrés (section 5.6, règle 1). La tâche passe en pause, cause `disk_full`, code `DISK_FULL`, avec en détail la place nécessaire et la place libre.
- **Reprise.** Toutes les 30 s, le planificateur vérifie la place pour les tâches en pause `disk_full`. Quand il y a de quoi finir plus la réserve, il les remet en file.
- **Mesure impossible.** Si la place libre ne peut pas être mesurée, le moteur part quand même : un disque vraiment plein arrêtera la tâche proprement à l'écriture.
- **Essais.**
  - Un vrai disque plein est essayé sous Linux sur un `tmpfs` de 8 Mio, monté sans droits d'administrateur dans un espace de noms utilisateur. C'est le test ignoré `a_really_full_disk_loses_nothing`, lancé avec `AURO_FULL_DISK_DIR`.
  - Le 2026-11-06, l'écriture a échoué après 6 176 768 octets enregistrés. Chaque octet compté en base était exact sur disque, et la tâche a fini après libération de la place.
  - Sous Windows, le manque de place est simulé (mesure remplacée par les tests) ; l'échec réel d'écriture n'y est pas essayé.
  - Avec la réservation (depuis le 2026-11-08), le même `tmpfs` voit le manque avant tout octet reçu : test ignoré `a_really_full_disk_is_seen_before_downloading`. Le test précédent tourne sans réservation, pour garder l'essai de l'échec à l'écriture.

### 7.7 Multi-miroirs

- Le moteur refuse un miroir dont la taille ou les validateurs diffèrent de la source de référence, sauf si une empreinte fournie permet de trancher à la fin.
- Chaque source a un débit moyen lissé. Les segments sont confiés de préférence aux sources les plus rapides.
- Une source dont le débit tombe sous le quart de la médiane passe à `slow` : elle ne reçoit plus de nouveaux segments, sans être coupée.
- Après 3 échecs consécutifs, elle passe à `failed` pour un délai croissant, puis est retestée.

Conception (phase 6, étape 1, décisions de l'utilisateur du 2026-11-10) ; elle précise les règles ci-dessus :
- **Sources d'une tâche HTTP** : la première adresse est la source de référence (rang 0) ; les suivantes sont des miroirs (rangs 1 et plus), 16 sources au plus. `task.add` les reçoit toutes dans `sources[]` ; `task.addSource` (`taskId`, `url`, portée `control`) en ajoute une à une tâche HTTP pas encore terminée. Une adresse déjà présente est refusée. Un lien magnet, un `.torrent` ou une vidéo n'a qu'une source.
- **Validation d'un miroir** (décision : taille et échantillons) : les validateurs (`ETag`, date) diffèrent presque toujours d'un serveur à l'autre et ne servent qu'à la reprise sur chaque source. Un miroir est accepté s'il accepte les plages, annonce la même taille que la référence, et rend les mêmes octets sur 3 morceaux de 16 Kio tirés au hasard (le fichier entier s'il est plus petit), comparés à ceux de la référence. Sinon il passe à `failed`, avec la raison dans `last_error`, et n'est jamais utilisé. Un miroir donné à `task.add` est validé à la première exécution, après la préparation de la référence ; un miroir ajouté par `task.addSource` à une tâche préparée est validé tout de suite (refus nommé à l'appel), sinon à la première exécution. Une empreinte connue vérifie le tout à la fin (section 7.9).
- **En-têtes de la tâche** (`Referer`, `Cookie`, `Authorization`) : seulement vers les sources du même hôte que la référence ; vers un autre hôte, le seul `User-Agent`. Les identifiants de sites (section 7.14) valent pour l'hôte de chaque source.
- **Changement d'un fichier distant** (décision) : une source dont le fichier a changé (`FILE_CHANGED`) passe à `failed` et ses segments vont aux autres ; la tâche ne se met en pause `file_changed` (F-13) que s'il ne reste aucune autre source utilisable.
- **Fin** : la tâche n'échoue que quand aucune source n'est plus utilisable ; le code d'échec est celui de la dernière erreur.
- **Interfaces** (décision) : CLI (`auro add <url> --mirror <url>…`, `auro mirror <tâche> <url>`) et application de bureau (champ « Miroirs » de la boîte « Ajouter », sources dans les Propriétés) ; Android et l'extension plus tard. Fichiers Metalink (`.meta4`, F-27) à la fin de l'étape.
- **Répartition et bascule** (étape 1b, 2026-11-10) :
  - Les miroirs déjà validés sont des sources dès le départ ; ceux à valider le sont pendant le téléchargement, et s'y joignent une fois acceptés. Une source validée puis écartée lors d'une exécution précédente est réessayée ; un miroir refusé à sa validation ne l'est plus.
  - Le nombre de connexions (automatique ou choisi) vaut pour chaque source, comme les connexions par serveur d'aria2 : un miroir accepté a les siennes. Une nouvelle connexion va à la source qui a le meilleur débit lissé par connexion ouverte ; une source pas encore mesurée compte pour la médiane des autres ; à égalité, la référence passe avant. Un segment libéré se coupe comme avant (section 7.2) : une source rapide reprend la moitié du plus gros segment en cours, souvent celui d'une source lente.
  - Débit lissé de chaque source, mesuré toutes les demi-secondes (au moins 0,2 s d'écart). Après 4 mesures, une source sous le quart de la médiane des autres passe à `slow` jusqu'à la fin de l'exécution. Le débit est enregistré (`avg_bps`) pendant l'exécution et à la fin.
  - Échec d'une connexion qui tient à sa source (réseau, serveur), alors qu'une autre source travaille (ni au repos, ni écartée) : définitif (fichier changé, refus du serveur, plages ignorées), la source est écartée pour l'exécution ; passager (coupure, délai), au bout de 3 d'affilée, elle se repose 30 s, deux fois plus à chaque repos, 10 min au plus. Le segment reprend sur les autres depuis ses octets enregistrés. La source passe à `failed` en base, avec la raison ; revenue de son repos, à `ok`. Sans autre source qui travaille, la règle d'une source unique s'applique (nouvel essai par le planificateur, pause, échec) : jamais d'attente sans fin.
  - Tests : partage entre deux sources, miroir changé écarté seul, référence changée (le miroir finit), toutes changées (pause `file_changed`), miroir qui coupe tout (repos), miroir lent (`slow`) ; banc `crash_daemon` : deux tâches sur trois avec un miroir, qui doit servir des segments.
- Mise en œuvre par sous-étapes : 1a. protocole, stockage, validation, CLI (faite le 2026-11-10 : module `mirrors`, `store::sources`) ; 1b. répartition des segments et bascule, bancs (faite le 2026-11-10) ; 1c. bureau (faite le 2026-11-11, essai de l'interface validé par l'utilisateur, section 4.6 ; `sourceId` des segments de `task.get`) ; 1d. Metalink (faite le 2026-11-11, ci-dessous ; essai réel de la CLI sur les Metalink 4 et 3 d'openSUSE, essai de l'interface validé par l'utilisateur). Étape 1 terminée.
- **Metalink** (F-27, étape 1d, décisions de l'utilisateur du 2026-11-11) :
  - Formats : Metalink 4 (`.meta4`, RFC 5854, espace de noms `urn:ietf:params:xml:ns:metalink`) et Metalink 3 (`.metalink`, `http://www.metalinker.org/` ou `version="3…"`), 4 Mio au plus. Lecture XML sans DTD (refusée), seules les entités prédéfinies et les références de caractère ; module `metalink` du moteur, seul à lire ces fichiers.
  - Une tâche HTTP par fichier décrit (256 au plus), le choix des fichiers fait par le client (`select`). Le nom vient d'un tiers : un chemin absolu, vide, avec `.` ou `..`, est écarté ; chaque composante passe par `net::filename::sanitize` ; les dossiers (16 niveaux au plus) sont créés sous le dossier de destination (`AddTask::sub_dir`). Deux fichiers au même chemin une fois nettoyés (casse ignorée sous Windows et Android), ou un chemin qui en prolonge un autre : le second est écarté.
  - Sources : les adresses HTTP(S) seulement, triées par priorité (version 4 : `priority`, 1 la meilleure ; version 3 : `preference`, 100 la meilleure), sans priorité après les autres, à égalité dans l'ordre du document ; le pays (`location`) est ignoré ; doublons retirés ; les 16 premières gardées, la première est la référence, les suivantes des miroirs validés comme les autres. Les torrents (`metaurl`, version 3 `type="bittorrent"`) et les autres protocoles sont ignorés ; un fichier sans adresse HTTP(S) est écarté.
  - Empreinte : la plus forte du fichier entier (SHA-256, puis SHA-1, puis MD5) devient celle de la tâche, vérifiée à la fin (section 7.9) ; une valeur mal formée écarte le fichier ; les autres algorithmes (SHA-512…) sont ignorés. Les empreintes par morceau (`pieces`) et les signatures sont ignorées en v1. La taille annoncée n'est que montrée.
  - Interfaces : CLI (`auro metalink <fichier|adresse>` montre le contenu ; `auro add` et `auro get` prennent un fichier `.meta4` ou `.metalink`, ou une adresse dont le chemin finit ainsi, avec `--select 0,2-4` ; `get` suit tous les fichiers) et bureau (section 4.6) ; l'extension et Android plus tard.
  - Tests : `metalink::tests` (formats, ordre, empreinte, noms, refus, fuzzing par proptest), `rpc::tests` (lecture à une adresse, ajout, sous-dossier, clé d'idempotence, refus), test de la CLI.

### 7.8 Nouvelles tentatives

Après un échec temporaire, la tâche passe en `retry_wait` et le délai avant la nouvelle tentative est `min(plafond, base × 2^n) + aléa`, avec une base de 2 secondes et un plafond de 5 minutes. Un en-tête `Retry-After` prime sur ce calcul. Au bout de 10 échecs d'affilée (réglable), la tâche passe en `failed`.

### 7.9 Vérification d'intégrité

Pour un fichier segmenté, l'empreinte se calcule à la fin, par une lecture séquentielle du fichier, pendant l'état `verifying`. En flux unique, la spécification prévoyait un calcul au fil de l'eau ; il n'est pas écrit (décision du 2026-11-08, confirmée par l'utilisateur le même jour) : un calcul en cours ne survit pas à un arrêt, et la lecture finale, à la vitesse du disque, coûte peu face au téléchargement. Le flux unique se vérifie donc comme le reste.

Déroulement (F-11, depuis le 2026-11-08) :

- **Ordre** : le `.part` complet est vidé sur disque et fermé, la tâche passe en `verifying` (le passage est annoncé par `task.stateChanged`), puis le moteur lit le fichier. Le `.part` ne prend son nom qu'après une empreinte juste ; la tâche passe alors en `completed`.
- **Empreinte différente** : la tâche passe en `failed`, code `CHECKSUM_MISMATCH`, avec dans `errorDetail` l'empreinte obtenue et celle attendue. Le fichier reste sous son nom provisoire (`.part`) : un fichier faux ne prend jamais le nom attendu (décision du 2026-11-08, confirmée par l'utilisateur le même jour). L'utilisateur corrige l'empreinte (`task.update`) et relance la tâche : le `.part`, complet, se vérifie de nouveau sans rien retélécharger, même d'un serveur sans plages (sauf taille inconnue, plus bas). Sinon, `task.restart` repart de zéro.
- **Arrêt pendant le calcul** : section 5.5 ; à la reprise, le calcul recommence depuis le début. Une tâche de taille inconnue (flux sans `Content-Length`) n'a pas de segments enregistrés : relancée après une pause ou une empreinte différente, elle se retélécharge.
- **Arrêt brutal pendant le calcul** : au démarrage, une tâche restée en `verifying` refait le calcul sur son `.part`. Si le `.part` a disparu et que le fichier final est là, l'arrêt a eu lieu entre le renommage, qui ne suit qu'une empreinte juste, et l'enregistrement de la fin : la tâche se termine. Si les deux manquent, elle passe en `failed`, code `DISK_IO`.
- **Empreinte changée en cours de route** : elle se relit au moment du calcul ; on peut la donner pendant le téléchargement.
- **Empreinte lue dans un fichier voisin** (depuis le 2026-11-08 ; réglage `downloads.neighborChecksum`) : quand la tâche n'a pas d'empreinte, le moteur la cherche à la préparation, une seule fois par téléchargement.
  - Fichiers essayés, à côté de l'adresse de la tâche (celle donnée à l'ajout, pas celle d'arrivée après les redirections, souvent signée ou sur un autre serveur), sans sa requête ni son fragment : `<fichier>.sha256` et `SHA256SUMS`, puis `<fichier>.sha1` et `SHA1SUMS`, puis `<fichier>.md5` et `MD5SUMS`. Tous en même temps, 10 s au plus en tout, 1 Mio au plus chacun ; la meilleure trouvaille dans cet ordre l'emporte.
  - Lecture : lignes `empreinte  nom`, `empreinte *nom` (GNU) et `SHA256 (nom) = empreinte` (BSD). Une ligne ne compte que si elle nomme exactement le fichier (`./` admis), d'après son adresse ou le nom proposé par le serveur ; une empreinte seule sur sa ligne seulement dans un fichier compagnon (`.sha256`). Deux empreintes différentes pour le même fichier : aucune.
  - Requêtes sans les en-têtes ni les identifiants de la tâche : un cookie ou un mot de passe ne suit pas une adresse que l'utilisateur n'a pas demandée. Un fichier voisin réservé aux membres n'est donc pas trouvé.
  - L'empreinte trouvée est enregistrée avec le nom du fichier voisin (`tasks.checksum_source`), avant la préparation : après un arrêt entre les deux, la recherche n'est pas refaite. Elle se vérifie ensuite comme celle de l'utilisateur, y compris une empreinte différente (`CHECKSUM_MISMATCH`, le `.part` gardé). L'utilisateur peut la remplacer ou la retirer (`task.update`) ; un retéléchargement l'oublie (le fichier a pu changer), mais garde celle de l'utilisateur.
  - L'application l'affiche avec son origine (« lue dans SHA256SUMS ») ; une case des options (« Enregistrement ») coupe la recherche.
  - Choix du 2026-11-08, confirmés par l'utilisateur le même jour : réglage activé par défaut ; requêtes sans cookies ni identifiants ; fichiers cherchés à côté de l'adresse de la tâche seulement ; empreinte voisine oubliée au retéléchargement.
- **Pas encore** : la vérification d'un fichier déjà terminé ; d'autres noms de listes (`sha256sum.txt`, `CHECKSUMS`) et SHA-512.

Si des empreintes par segment existent (torrent, Metalink), seul le segment fautif est téléchargé de nouveau (F-12). Les torrents le font déjà, morceau par morceau (section 7.16). Pour un Metalink qui publie des empreintes de morceaux (`pieces`), décisions de l'utilisateur du 2026-11-17 :
- **Moment** : à la fin, pendant `verifying`, en une seule lecture du `.part` (pas pendant le téléchargement).
- **Morceau faux** : seuls ses octets sont retéléchargés, de préférence depuis une autre source que celle qui l'a envoyé, puis le fichier est revérifié. Une source qui a envoyé 2 morceaux faux est écartée pour cette tâche, comme un miroir refusé (section 7.7).
- **Limite** : 3 tours « retéléchargement des morceaux faux, puis vérification » ; ensuite, échec `CHECKSUM_MISMATCH`, le `.part` gardé sous son nom provisoire, comme une empreinte du fichier différente.
- **Portée** : les empreintes de morceaux viennent seulement d'un Metalink, rien de nouveau dans `task.add` ni dans « Ajouter ». Les Propriétés montrent le nombre de morceaux, et combien ont été retéléchargés.
- **Mise en œuvre** (étape 5 de la phase 7, 2026-11-17) :
  - Lecture : `pieces` sous `file` (Metalink 4) ou `verification` (Metalink 3, rangs `piece` remis dans l'ordre, tous de 0 à n − 1). Les plus fortes (SHA-256, SHA-1, MD5) qui se lisent et dont le nombre convient à la taille annoncée ; sinon ignorées, sans écarter le fichier ni son empreinte entière. Enregistrées avec la tâche, dans la même transaction (table `task_pieces`, migration 12, section 5.2). Seulement par `metalink.add`.
  - Vérification : une seule lecture du `.part` donne l'empreinte de chaque morceau et, si la tâche en a une, celle du fichier. Des empreintes dont le nombre ne convient pas à la taille reçue sont ignorées (l'empreinte entière seule).
  - Tour de reprise, en une transaction : compte des tours et des morceaux, sources mises en cause, segments rouverts sur les morceaux faux (les octets autour restent, section 5.6, règle 2), passage de `verifying` à `active` (section 5.5). Le téléchargement reprend aussitôt, dans la même exécution, puis le fichier se vérifie de nouveau. Un arrêt avant ce tour laisse la tâche en `verifying` (la vérification se refait) ; après, en `active` avec ses morceaux à télécharger.
  - Source mise en cause : le moteur note la source de chaque segment quand une connexion le prend (`segments.source_id`). Un morceau faux compte contre la source qui l'a envoyé seule (`sources.bad_pieces`) ; un morceau venu de plusieurs sources ne met aucune d'elles en cause. Une nouvelle connexion va de préférence à une source qui a envoyé le moins de morceaux faux ; à 2, la source passe `failed` (« 2 morceaux faux envoyés : source écartée ») et ne sert plus tant qu'une autre le peut.
  - Échec : après 3 tours, ou sans plages pour redemander un morceau (serveur sans `Range`, taille inconnue), `CHECKSUM_MISMATCH` avec le nombre de morceaux faux ; le `.part` reste sous son nom provisoire. Une relance manuelle (passage à `queued`) redonne 3 tours, et ne télécharge que les morceaux faux. Un retéléchargement remet les comptes à zéro.
  - Affichage : `task.get` (section 6.5) ; les Propriétés montrent « 10 morceaux de 64 Kio (SHA-1), 3 retéléchargés ».
  - Tests : lecture (formats 4 et 3, choix, rejets), réouverture des segments (couverture conservée, proptest), tour en base (mise en cause, écart, remise à zéro), téléchargements face à un serveur qui envoie des octets faux (seul le morceau faux redemandé, trois tours puis l'échec et la relance, miroir fautif écarté au profit de la référence, arrêt pendant la vérification, arrêt après un tour, empreinte entière lue dans la même lecture, serveur sans plages), protocole.

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
| Contrôle des plages horaires (section 7.11) | 15 s | non |
| Délai entre l'annonce d'une action de fin et son exécution | 60 s | non |

### 7.11 Planification (F-16)

Décisions du 2026-11-07 : une planification s'attache à une file ; ses actions de fin sont la mise en veille et l'extinction (la commande externe viendra avec F-34) ; une reprise manuelle hors des plages attend la plage suivante.

- **Règles.** Une planification porte un nom, un drapeau `enabled` et des règles : au moins une plage, et une action de fin (`onDone` : `none`, `sleep` ou `shutdown` ; `none` si absente).
  - Une plage : `days` (de 1, lundi, à 7, dimanche ; au moins un), `start` et `end` au format `HH:MM`, heure locale de la machine du moteur.
  - Une plage dont `end` est inférieur ou égal à `start` passe minuit, et appartient au jour où elle commence : « vendredi, 23:00 à 07:00 » couvre la nuit du vendredi au samedi. `start` égal à `end` : 24 h.
  - L'heure locale suit l'heure d'été du système (lue par SQLite, modificateur `localtime`).
- **Ouverture.** Une file sans planification, ou dont la planification est désactivée, télécharge à toute heure. Sinon, elle est ouverte pendant ses plages et fermée en dehors.
  - Une file fermée ne démarre rien : ni ses tâches en file, ni ses nouvelles tentatives, ni une tâche reprise à la main, qui attend la plage suivante.
  - Quand elle ferme, ses tâches en cours s'arrêtent après leur point de contrôle et retournent en file (`active` → `paused` → `queued`, comme à l'arrêt du moteur, section 7.4). Elles reprendront à la plage suivante, à partir de leurs octets enregistrés.
  - Une tâche restée `active` après un arrêt brutal du moteur, dans une file fermée à son redémarrage, retourne de même en file (`paused`, cause `schedule`, puis `queued`).
  - Le moteur contrôle les plages toutes les 15 s : une file s'ouvre ou se ferme au plus 15 s après l'heure prévue.
- **Protocole.** `schedule.list` et `sync.snapshot` décrivent chaque planification : `scheduleId`, `name`, `enabled`, `rules`, `open` (ses files peuvent télécharger ; toujours vrai si elle est désactivée) et `nextChangeAt` (prochain changement d'ouverture, millisecondes Unix, `null` s'il n'y en a pas).
  - `schedule.set` crée une planification (sans `scheduleId`) ou la remplace ; `enabled` vaut vrai s'il est absent. Des règles invalides sont refusées par `INVALID_PARAMS`, un identifiant inconnu par `SCHEDULE_NOT_FOUND`.
  - `queue.create` et `queue.update` acceptent `scheduleId` ; `null` détache la planification. Une file la décrit dans `scheduleId`.
  - `schedule.delete` la supprime ; ses files la perdent, ce qu'annonce `queue.updated`.
  - Chaque changement, et chaque ouverture ou fermeture, est annoncé par `schedule.updated` avec une nouvelle révision.
- **Action de fin.** Quand une tâche d'une file planifiée se termine, et que la planification active de cette file a une action de fin, le moteur l'arme. Dès que la file n'a plus rien à télécharger (aucune tâche en file, en cours, en attente d'une nouvelle tentative, en vérification ou en finition ; les pauses ne comptent pas), il l'annonce par `engine.endAction`, avec l'heure prévue `at`, une minute plus tard.
  - Pendant cette minute, `engine.cancelEndAction` l'annule ; elle tombe aussi d'elle-même si la file a de nouveau du travail, ou si sa planification ne la demande plus (désactivée, action changée, planification détachée). L'annulation est annoncée par `engine.endAction` avec `pending` à `null`.
  - À l'heure dite, le moteur exécute l'action, puis l'annonce de même ; si le système la refuse, `error` le dit.
  - Une seule action de fin à la fois. `sync.snapshot` porte celle qui est annoncée dans `endAction`.
  - Exécution : sous Windows, `SetSuspendState` pour la veille, `%SystemRoot%\System32\shutdown.exe /s /t 0` pour l'extinction ; sous Linux, `systemctl suspend` ou `systemctl poweroff`. Ailleurs (Android), l'action est refusée.
  - Seul `aurod` branche ces actions. La bibliothèque du moteur, ses tests et ses bancs n'en ont aucune, et refusent. `aurod --simulate-power` écrit l'action dans son journal au lieu de l'exécuter, pour les essais.
  - Portée (section 6.9) : créer, modifier, supprimer ou attacher une planification demande la portée `admin` ; annuler une action de fin, `control`.

### 7.12 Déplacer et renommer (depuis le 2026-11-08)

« Déplacer/Renommer » change le dossier ou le nom du fichier d'une tâche qui ne tourne pas : `queued`, `paused`, `retry_wait`, `failed`, `cancelled` ou `completed`. Une tâche `active` est refusée (`INVALID_STATE`) : il faut d'abord l'arrêter. Protocole : `task.update` avec `destDir`, `fileName`, ou les deux.

- **Ce qui bouge** : le fichier final d'une tâche terminée, le `.part` des autres, que le téléchargement continue ensuite au nouvel endroit. Une tâche pas encore préparée n'a pas de fichier : la préparation prendra ce dossier et ce nom.
- **Nom choisi** : il est gardé dans `tasks.requested_name` (migration 0003) et sert aussi de nom affiché. La préparation le prend au lieu du nom du serveur, avec « nom (1).ext » s'il est déjà pris (section 7.1, étape 3), et il survit au retéléchargement. `task.add` accepte aussi `fileName`.
- **Vérifications**, avant de toucher au disque :
  - un nom valide sur ce système, tel quel : `net::filename::sanitize` doit le rendre inchangé. Sinon le refus est `INVALID_PARAMS`, avec une proposition ; le moteur ne corrige pas en silence ;
  - un dossier existant, en chemin absolu (`DEST_NOT_WRITABLE`) ;
  - aucun fichier ni `.part` à ce nom, et aucune autre tâche du dossier qui l'a réservé (`FILE_EXISTS`) : le moteur n'écrase jamais un fichier. Sous Windows, changer seulement la casse du nom est permis ;
  - le fichier d'une tâche terminée existe encore (`FILE_MISSING`). Un `.part` absent n'empêche rien : la reprise le recrée (section 7.4).
- **Même volume** : un renommage, durable (dossiers vidés sur disque sous Linux). Le fichier est à sa place au retour de `task.update`.
- **Autre volume** : le renommage échoue (`CrossesDevices`) et le fichier est copié hors de la boucle du planificateur, qui ne se bloque pas.
  - La copie va d'abord dans `<destination>.auro-move`. Elle est vidée sur disque avec la date de modification d'origine, puis renommée à sa destination.
  - `task.update` répond dès le début de la copie, avec `moving` à vrai et l'ancien emplacement.
  - La fin est annoncée par `task.updated`, avec `moving` à faux et le nouvel emplacement. Si la copie échoue, l'annonce porte l'ancien emplacement et `moveError`, et la copie partielle est effacée.
  - Pendant la copie, la tâche ne démarre pas. Elle ne se retire pas, ne s'annule pas, ne se retélécharge pas et ne se déplace pas ; ces demandes sont refusées par `INVALID_STATE`. Elle peut seulement être mise en pause ou reprise.
  - Un arrêt du moteur n'attend pas la fin d'une copie : la reprise au démarrage s'en occupe.
- **Journal** (table `file_moves`, section 5.3) : écrit avant le premier changement sur le disque.
  - Phase `moving` : la tâche porte encore l'ancien emplacement.
  - La tâche ne prend le nouveau que quand le fichier y est, dans la même transaction que la fin du journal. Pour une copie, l'original n'est effacé qu'ensuite, en phase `moved`. À tout instant, la tâche désigne donc un fichier complet.
- **Reprise au démarrage**, avant toute exécution (`moves::recover`) :
  - phase `moved` : l'original est effacé ;
  - copie temporaire présente : effacée ;
  - seule la destination existe : le renommage avait eu lieu, la tâche la suit ;
  - les deux existent après une copie, de même taille : la copie était complète et renommée ; la tâche la suit, l'original est effacé ;
  - sinon, la tâche garde son ancien emplacement, et aucun fichier n'est effacé.
- **Historique** : pour une tâche terminée, le chemin de sa dernière fin (`history.final_path`) suit le fichier.
- **Tests** : chaque point d'arrêt de la reprise (`moves::tests`) ; un arrêt simulé entre le renommage et l'enregistrement, suivi d'un redémarrage du moteur (`scheduler::tests`). Les copies vers un autre volume sont simulées dans les tests par un dossier `autre-volume`. Un essai sur deux vrais volumes reste à faire.

### 7.13 Actualiser l'adresse (depuis le 2026-11-08)

« Actualiser l'adresse » remplace l'adresse d'une tâche qui ne tourne pas, typiquement un lien expiré ou à usage limité, régénéré sur le site d'origine. Les états permis sont ceux de la section 7.12. Protocole : `task.update` avec `url`.

- **Tâche commencée** (taille connue, ni terminée ni annulée) : le moteur interroge d'abord la nouvelle adresse (préparation, section 7.1), hors de sa boucle, avec les en-têtes de la tâche.
  - Elle doit annoncer la même taille, sinon `SOURCE_MISMATCH`. Si elle ne répond pas, `SOURCE_UNREACHABLE`, avec la cause classée (section 6.8). Rien ne change alors.
  - Si elle est acceptée, ses validateurs (`ETag`, `Last-Modified`) et sa prise en charge des plages remplacent ceux de l'ancienne adresse. La suite du téléchargement les vérifie par `If-Range` (section 7.4).
  - **La validation repose uniquement sur la taille annoncée du fichier ; aucune comparaison de contenu ni d'empreinte n'est faite avant la reprise.** Décision du 2026-11-08, confirmée par l'utilisateur à titre provisoire : seule la taille relie les octets déjà reçus à la nouvelle adresse. Deux serveurs d'un même fichier ont rarement le même `ETag`, et l'exiger refuserait presque tout lien régénéré. C'est une validation faible (section 7.4, étape 5) ; une empreinte fournie (F-11, section 7.9) reste la seule garantie : elle est vérifiée à la fin, sur tout le fichier. Plus tard, peut-être : comparer aussi `ETag`, `Last-Modified`, ou une empreinte publiée, quand les deux adresses les donnent.
  - Une nouvelle adresse sans plages fait repartir le téléchargement du début.
- **Tâche pas encore préparée, terminée ou annulée** : l'adresse est remplacée sans être interrogée ; la prochaine préparation l'interrogera. Le nom affiché, tant que ni le serveur ni l'utilisateur n'en ont donné, suit la nouvelle adresse.
- **Cookies** : si l'hôte change, les en-têtes secrets de la tâche (`Cookie`, `Authorization`, `Proxy-Authorization`) ne suivent pas. Ils sont effacés, référence d'abord puis coffre (section 5.1), et la vérification se fait sans eux. Les autres en-têtes (`Referer`, `User-Agent`) restent.
- **Échecs** : les échecs comptés de la source sont oubliés. L'état de la tâche ne change pas ; l'application relance d'elle-même une tâche en échec après l'actualisation.
- **Annonce** : `task.updated` avec `displayName`.
- **Pas encore** : la capture du nouveau lien par l'extension (IDM ouvre la page d'origine et attend le prochain clic) ; aujourd'hui, l'adresse se colle.

### 7.14 Identifiants de sites (F-06, depuis le 2026-11-08)

Comme l'onglet « Logins de sites » d'IDM : un nom d'utilisateur et un mot de passe par site, envoyés quand un serveur de ce site les demande. Cela sert aux serveurs qui demandent une identification HTTP (`401` avec `WWW-Authenticate`), par exemple un dossier protégé, certains liens directs de services de débridage, un serveur privé. Un site qui identifie par un formulaire et des cookies se télécharge par l'extension, qui transmet les cookies (section 4.5).

- **Enregistrement** : table `site_logins` (migration 0004). Le mot de passe va au coffre du système (section 5.1), la base n'en garde que la référence ; il ne ressort jamais par le protocole.
- **Site** : une adresse ou un nom d'hôte, normalisé à l'enregistrement (`[protocole://]hôte[:port][chemin]`, sans `/` final, hôte en minuscules ; ni identifiants, ni paramètres, ni ancre).
  - `exemple.org` vaut pour HTTP et HTTPS, pour l'hôte et ses sous-domaines (`dl.exemple.org`), sur tous les ports.
  - Un protocole, un port ou un chemin précisés restreignent la portée. Un chemin couvre ce qui est dessous : `/prive` couvre `/prive/a`, pas `/privee`.
  - Si plusieurs sites conviennent, le plus précis l'emporte : le chemin le plus long, puis l'hôte exact, puis le protocole précisé.
  - Un site n'a qu'un jeu d'identifiants.
- **Choix pour une tâche** : au départ de chaque exécution, les identifiants du site de son adresse, mot de passe lu au coffre. Si la tâche porte son propre en-tête `Authorization` (section 6.5, en-têtes), il passe avant, et aucun identifiant de site n'est utilisé. Un mot de passe absent du coffre : la tâche part sans.
- **Envoi** : rien d'avance.
  - À la première demande du serveur, le moteur répond et recommence la requête, à l'adresse qui l'a demandée. Il refuse si cette adresse n'est pas du site : après une redirection vers un autre hôte, rien n'est envoyé à celui-ci.
  - Ensuite, toutes les requêtes de l'exécution, toutes connexions confondues, portent l'identification.
  - Digest (RFC 7616) est préféré à Basic (RFC 7617), et SHA-256 à MD5. Variantes prises en charge : `MD5`, `MD5-sess`, `SHA-256`, `SHA-256-sess`, avec `qop=auth` ou sans `qop`. Ne sont pas prises en charge : `auth-int`, `SHA-512-256`, NTLM et Negotiate.
  - Un nonce Digest périmé (`stale=true`) est renouvelé sans compter comme un refus.
- **Basic sur une connexion non chiffrée** : refusé, car le mot de passe y circulerait en clair. Le moteur n'envoie rien, et la tâche échoue avec `AUTH_FAILED`, avec un message qui le dit. Digest reste permis en HTTP : le mot de passe n'y circule pas. Décision du 2026-11-08, confirmée par l'utilisateur : le moteur n'envoie **jamais** d'en-tête `Authorization: Basic` sur `http://`, même si d'autres outils (IDM) le font ; un site qui l'exige doit passer par HTTPS, et le message de l'échec le dit.
- **Échecs** (section 6.8) :
  - `AUTH_REQUIRED` : le serveur demande une identification, et aucun identifiant ne convient à cette adresse, ou le schéma demandé n'est pas pris en charge ;
  - `AUTH_FAILED` : les identifiants envoyés sont refusés, ou Basic est demandé en clair ;
  - les deux sont définitifs : la tâche se relance une fois les identifiants corrigés.
  - Un `401` sans `WWW-Authenticate` reste `HTTP_STATUS`.
- **Actualiser l'adresse** (section 7.13) : la vérification de la nouvelle adresse utilise les identifiants de son site.
- **Protocole** : `login.list` (`items[]` : `loginId`, `site`, `username`) ; `login.set` (`loginId` pour modifier, `site`, `username`, `password`), qui rend les identifiants enregistrés ; `login.delete` (`loginId`). Les trois demandent la portée `admin` (section 6.9), même la liste, qui montre les noms d'utilisateur.
  - Pour de nouveaux identifiants, `password` est obligatoire ; pour une modification, absent, il garde celui enregistré.
  - Refus `INVALID_PARAMS` : site invalide, site déjà enregistré, nom vide, de plus de 256 octets, avec `:` (interdit par Basic) ou un caractère de contrôle, ou mot de passe vide.
  - Refus `LOGIN_NOT_FOUND` : identifiant inconnu.
  - Pas de notification : un client relit la liste quand il l'affiche.
- **Coffre** : le nouveau mot de passe est écrit avant sa référence, l'ancien effacé après ; un retrait efface la référence, puis le mot de passe. Le ramassage du démarrage garde les mots de passe des identifiants (`store::headers::all_secret_refs`).
- **CLI** : `auro login list`, `auro login set <site> -u <nom>` (mot de passe saisi sans écho, ou `--password-stdin`, jamais en argument), `auro login remove <site>`.
- **Application** : onglet « Identifiants de sites » des options, avec une liste, « Nouveau… », « Modifier… » et « Supprimer ». Le champ du mot de passe reste vide à la modification, et vide garde celui enregistré.
- **Bearer** (F-06) : un jeton se donne par l'en-tête `Authorization` de la tâche (`auro add -H "Authorization: Bearer …"`), qui va au coffre (section 5.1).

### 7.15 Réseau utilisable et arrière-plan (depuis le 2026-11-09)

Le moteur ne sait pas seul si le réseau convient : l'application le lui dit (`engine.setNetwork`). Sans réseau utilisable, rien ne démarre, sauf le calcul d'une empreinte, qui n'en a pas besoin ; les tâches en cours retournent en file (`active`, puis `paused` cause `network`, puis `queued`), comme à la fermeture d'une plage horaire. Au retour du réseau, tout repart, et les tâches qui attendaient un nouvel essai après une erreur de réseau (`NET_UNREACHABLE`, `NET_TIMEOUT`, survenue avant que l'application ne dise la coupure) repartent aussitôt, sans attendre leur délai ; celles qu'un serveur a fait attendre gardent le leur. L'état est annoncé (`engine.network`) et figure dans l'instantané (`networkUsable`). Il n'est pas enregistré : vrai au démarrage d'`aurod`, qui ne le change jamais lui-même.

Application Android (phase 4, étape 4) :

- Le moteur y démarre sans réseau utilisable (`Planning::network_usable`). L'application le dit utilisable quand le réseau convient (connecté ; sans frais au volume avec « Wi-Fi seulement ») **et** qu'Android protège le processus : un transfert lancé par l'utilisateur (Android 14 et plus, `JobScheduler`, `setUserInitiated`, contrainte de réseau : quelconque, ou sans frais avec « Wi-Fi seulement »), un service au premier plan de type « synchronisation de données » (Android 8 à 13), ou l'application à l'écran. Un processus qu'Android peut tuer à tout moment ne télécharge pas sans protection ; rien ne part au démarrage de l'appareil sans l'utilisateur.
- Pourquoi le transfert lancé par l'utilisateur à partir d'Android 14 : à partir d'Android 15, un service « synchronisation de données » est limité à 6 heures par période de 24 heures (remises à zéro quand l'utilisateur revient dans l'application), et ne peut pas être démarré au démarrage de l'appareil (documentation d'Android, consultée le 2026-11-09). Le transfert n'a pas de limite fixe ; Android l'arrête si sa contrainte de réseau tombe, ou pour la santé de l'appareil, puis le relance de lui-même.
- Le transfert ou le service se demande quand une tâche est en file ou en cours et que l'application est à l'écran (Android l'exige) ; il se termine quand il n'y a plus rien à faire. Sa notification montre le nombre de téléchargements, l'avancement global et le débit, ou la cause de l'attente (« En attente du réseau », « En attente du Wi-Fi », aussi en bandeau dans la liste), avec « Tout mettre en pause ». Une fin ou un échec, l'application n'étant pas à l'écran, fait une notification.
- Redémarrage de l'appareil, avec l'option « Reprendre après le redémarrage » (désactivée par défaut) : sous Android 8 à 13, le service repart ; à partir d'Android 14, Android ne laisse démarrer ni service ni transfert à ce moment, et une notification propose de reprendre d'un geste.
- Liens partagés ou ouverts : reçus par une activité à part, sans fenêtre, toujours neuve (`ShareActivity`). Confiés à la fenêtre principale, ils se perdaient quand Android la recréait depuis son état sauvegardé (processus arrêté, fenêtre restée dans les applications récentes) : constaté sur la tablette le 2026-11-09.
- Essais du 2026-11-09 sur la tablette (Android 15) : ISO de 756 Mio menée à terme l'application en arrière-plan, notification à jour, transfert fermé à la fin ; Wi-Fi coupé en cours : transfert arrêté par Android, rien de reçu, puis relancé de lui-même au retour du Wi-Fi, l'application toujours en arrière-plan, et le téléchargement fini avec l'empreinte publiée ; « Wi-Fi seulement » avec le réseau déclaré facturé : rien de reçu, bandeau « En attente du Wi-Fi », reprise ensuite ; lien ouvert après l'arrêt du processus : ajouté. Le même jour, DVD Debian de 3,7 Gio : lien ouvert au tout premier démarrage de l'application (données effacées), application en arrière-plan, 25 à 29 Mio/s ; tablette redémarrée (`adb reboot`, accord de l'utilisateur) à 2,0 Gio ; après le déverrouillage par l'utilisateur, notification « Téléchargements à reprendre », touchée : reprise à 2,2 Gio sous un transfert lancé par l'utilisateur, fin en arrière-plan, empreinte publiée vérifiée par le moteur puis recalculée sur l'appareil (`sha256sum`). « Tout mettre en pause », touché dans la notification dépliée : transfert fermé, notification retirée, fichier immobile, tâche en pause. Pas encore essayés : Android 8 à 13 (service).
- Essais du 2026-11-09 sur le téléphone Galaxy S25 Ultra, passé entre-temps à Android 17 (API 37 ; l'application vise l'API 35) : DVD Debian de 3,7 Gio ouvert au tout premier démarrage, application à l'écran, 41 Mio/s, empreinte juste ; le même, application en arrière-plan sous un transfert lancé par l'utilisateur, 40 Mio/s, notification à jour, transfert fermé à la fin, empreinte juste ; le même encore, processus de l'application tué (`kill -9`) à 1,0 Gio : Android relance aussitôt le processus pour le transfert, qui reprend sans repartir de zéro et finit avec l'empreinte juste. Chaque empreinte est vérifiée par le moteur, puis recalculée sur l'appareil. L'antivirus AVG du téléphone signale l'application (paquet de mise au point signé par la clé d'essai, installé par adb) comme « application suspecte » à son installation : à suivre pour la diffusion (paquet signé, distribution par une boutique).
- Premier démarrage par un lien (essai du 2026-11-09) : le moteur d'Android n'a pas de dossier « Téléchargements » du système à prendre par défaut, et l'application ne règle `downloads.dir` qu'après une première réponse du moteur ; un ajout arrivé avant était refusé (`DEST_NOT_WRITABLE`), et le lien perdu. Les ajouts de l'application attendent désormais la fin de ce réglage.

### 7.16 Torrent (phase 5, conception du 2026-11-09)

Conception arrêtée après le prototype (`experiments/torrent/README.md`) et les décisions de l'utilisateur du 2026-11-09. Mise en œuvre par étapes (3a à 3e, CLAUDE.md) ; ce qui n'est pas encore fait reste refusé par `CAPABILITY_MISSING`.

**Bibliothèque.** librqbit 9.1.0, sans ses fonctions par défaut, avec `rust-tls` (rustls et aws-lc, comme le reste du moteur), derrière le module `torrent` : rien d'autre dans le moteur ne la connaît, pour pouvoir en changer (section 9.2). Une seule session par moteur, créée au premier torrent à démarrer : un moteur sans torrent n'ouvre ni port ni DHT.

**Ajout.** `task.add` reçoit dans `sources[0]` soit `url`, un lien `magnet:`, soit `torrent`, le contenu d'un fichier `.torrent` en base64. La tâche est de type `torrent`. Les métadonnées d'un lien magnet s'obtiennent au démarrage de la tâche, par le DHT et les pairs, comme la préparation d'un téléchargement HTTP (section 7.1). La table `torrents` garde l'empreinte d'information, le lien et les métadonnées une fois connues ; `task_files` reçoit les fichiers, tous choisis par défaut.

**Fichiers.**
- Torrent à un fichier : `destDir/<nom>`. À plusieurs fichiers : `destDir/<nom du torrent>/<chemins>`.
- Chaque composant de nom passe par `net::filename::sanitize` (section 8.1 : un chemin du torrent ne sort jamais du dossier).
- Refusés à l'ajout (`INVALID_PARAMS`, « fichier .torrent invalide ») : un composant `..`, un séparateur dans un nom, un fichier sans nom, deux noms identiques (librqbit) ; et, depuis la porte de la phase 5 (2026-11-10, décision de l'utilisateur), deux fichiers au même chemin une fois les noms nettoyés (`CON` et `_CON` sous Windows, `a\0b` et `a_b`) ou un fichier et un dossier de même nom (`fin` et `fin/x`), sans distinguer la casse sous Windows et Android. Sinon deux fichiers s'écriraient dans le même `.part`. Tests de propriétés (`torrent::fuzz`) : octets quelconques, `.torrent` abîmés ou aux noms hostiles, liens magnet quelconques ; jamais de panique, et un torrent accepté reste dans son dossier, chaque nom étant un seul composant nettoyé.
- Le premier élément (fichier ou dossier) évite un nom déjà pris, comme en HTTP (« nom (1) »).
- Chaque fichier s'écrit sous `<nom>.part` jusqu'à la fin du torrent.
- Les morceaux à cheval sur un fichier non choisi écrivent une partie de ce fichier : il reste en `.part` et part avec la tâche.

**Écriture et reprise : la vérité vient des données.**
- Le prototype l'a montré : librqbit et libtorrent font tous deux confiance à leur état de reprise, et finissent avec un fichier faux après des écritures perdues.
- Le moteur n'utilise donc ni la reprise rapide de librqbit ni sa persistance. Chaque démarrage d'une tâche torrent (ajout, reprise, redémarrage du moteur) revérifie l'empreinte de chaque morceau présent sur disque.
- Pendant cette vérification, la tâche est `active`, avec la phase `checking` dans son avancement. Elle a pris moins de 2 s pour 756 Mio au prototype.
- En conséquence, `done_bytes` et `task_files.done_bytes` ne sont qu'une indication pour un torrent. La règle 1 de la section 5.6, qui sert à ne compter que des octets vidés sur disque, n'y est pas nécessaire : un compteur trop optimiste ne fait rien perdre, puisque la reprise ne s'en sert pas.
- Le stockage du torrent est celui du moteur (trait `TorrentStorage` de librqbit) : écritures et lectures positionnées dans les `.part`.

**Fin.**
- Quand tous les morceaux des fichiers choisis sont là, chaque fichier est vidé sur disque, puis prend son nom final.
- La tâche passe ensuite à `seeding`, ou à `completed` si aucun partage n'est demandé.
- Un arrêt entre ces étapes laisse la tâche `active`. Au redémarrage, le stockage retrouve chaque fichier sous l'un ou l'autre nom, et la vérification complète refait le point.

**Partage** (décision de l'utilisateur du 2026-11-09 : jusqu'au ratio 1).
- Réglage `torrent.ratioLimit`, 1,0 par défaut : nombre réel, `0` pour ne pas partager, `null` pour partager sans limite. `torrents.ratio_limit` le remplace pour une tâche.
- `torrents.seed_time_limit_s` borne la durée, si l'utilisateur la fixe.
- La tâche reste `seeding` jusqu'à avoir envoyé `ratioLimit` × la taille des fichiers choisis, puis passe à `completed` et quitte la session.
- `torrents.uploaded_bytes` cumule les octets envoyés, d'une session à l'autre.
- Sous Android, le partage n'a lieu que sur un réseau sans frais au volume (précisé à l'étape 3b).

**Réseau** (décision de l'utilisateur du 2026-11-09 : port d'écoute, sans UPnP).
- Connexions entrantes en TCP sur un port fixe : `torrent.listenPort`, tiré au hasard entre 49152 et 65535 au premier démarrage de la session, puis gardé. Sous Windows, le pare-feu demande une fois s'il faut autoriser `aurod`.
- Pas d'UPnP par défaut (`torrent.upnp`, faux) : le moteur ne modifie pas la box.
- DHT actif, avec sa table gardée dans le dossier de données du moteur. Échange de pairs (PEX) actif. Pas de découverte sur le réseau local. uTP inactif (expérimental dans librqbit).
- Liaison à une interface (VPN, section 8.1) : étape 3d, y compris sous Windows, que librqbit ne gère pas (mise en œuvre ci-dessous).
- `engine.setNetwork` (section 7.15) arrête les torrents comme les autres tâches.

**Limites de débit.**
- La limite globale de réception s'applique aux torrents par le limiteur de librqbit, à part de celle des téléchargements HTTP. Les deux ensemble peuvent donc dépasser la limite globale, jusqu'au double.
- `uploadBps` (`limits.set`) limite l'envoi.
- `speedLimitBps` d'une tâche torrent limite sa réception.
- La limite d'une file ne s'applique pas aux torrents en v1.

**Pas en v1** (décision de l'utilisateur du 2026-11-09) : chiffrement du protocole (MSE/PE), sources web (BEP 19), torrents v2 (BEP 52).

**Mise en œuvre de l'étape 3a (2026-11-09).**
- **Fait** : ajout par lien magnet ou fichier `.torrent` (section 6.5) et téléchargement jusqu'à la fin.
  - Module `torrent` : session, stockage dans les `.part`, revérification à chaque démarrage, vidage sur disque puis renommage à la fin.
  - `store::torrents` ; colonne `kind` lue dans `TaskRow`.
  - Pas de nouvelle migration : la migration 1 avait déjà les tables `torrents` et `task_files`.
  - En attendant l'étape 3b, un torrent terminé passe directement à `completed`, sans partage.
- **Vérifications** :
  - Le moteur refuse un `.torrent` dont le nombre d'empreintes de morceaux ne correspond pas à la taille, ce que librqbit ne vérifie pas : un tel fichier n'échouait qu'à la vérification.
  - Le moteur attend que librqbit ait fermé tous les fichiers d'un torrent avant de les renommer ou de les effacer. Sous Windows, un fichier encore ouvert ne disparaît pas tout à fait.
- **Tests** : essaim local de deux pairs, sans Internet, couvrant :
  - un fichier seul, plusieurs fichiers avec dossiers et fichier vide ;
  - un lien magnet ;
  - un nom déjà pris (« nom (1) ») ;
  - un `.part` effacé pendant une pause, retrouvé et retéléchargé ;
  - la reprise après un redémarrage du moteur ;
  - l'annulation avec effacement des fichiers ;
  - le même torrent ajouté deux fois ;
  - des noms hostiles nettoyés (`CON`, `a<b`) et un chemin en `..` refusé.
- **Constaté** : `librqbit::create_torrent` (9.1.0) oublie l'empreinte du dernier morceau quand le dernier fichier est vide. Le moteur ne s'en sert pas ; les tests écrivent leurs `.torrent` eux-mêmes.

**Mise en œuvre de l'étape 3b (2026-11-09).**
- **Session** : elle s'ouvre avec le premier torrent en cours et se ferme avec le dernier. Sans torrent en cours, il n'y a ni port ouvert ni DHT. `engine.setNetwork` arrête les torrents, donc la session.
- **Partage** :
  - Fin du téléchargement : le torrent quitte la session, les fichiers choisis sont vidés sur disque puis renommés.
  - Si un partage est demandé, la tâche passe à `seeding` et le torrent revient dans la session sur les fichiers finaux. librqbit les revérifie en entier avant de partager, ce qui sert de vérification finale sur disque.
  - Le partage finit au ratio, compté sur la taille des fichiers choisis et les octets envoyés depuis l'ajout (`torrents.uploaded_bytes`). La tâche passe alors à `completed`, avec sa ligne d'historique.
  - Le ratio vient de `torrents.ratio_limit`, sinon du réglage `torrent.ratioLimit`. Il est lu au début de chaque exécution.
- **Interruptions du partage** :
  - Une pause pendant le partage l'arrête, et la tâche est terminée (`seeding` → `completed`). Une annulation pendant le partage l'annule ; avec `deleteFiles`, les fichiers terminés partent aussi.
  - Arrêt du moteur, réseau inutilisable, ou réseau facturé (`engine.setNetwork` avec `metered`) : la tâche reste `seeding`. Le planificateur relance son partage quand le réseau convient et n'est pas facturé.
  - Un partage ne prend pas de place dans sa file et ne suit pas ses plages horaires.
  - Si les fichiers terminés ont changé sur le disque, ou si la session ne s'ouvre pas, le partage s'arrête et la tâche est terminée.
- **Android** : l'application dit `metered` (réseau sans `NOT_METERED`). Le partage ne demande pas la protection d'Android : il n'a lieu que si l'application est à l'écran ou qu'un téléchargement est protégé, et jamais sur un réseau facturé.
- **Limites** : `uploadBps` global ; la limite globale de réception vaut aussi pour la session, à part du HTTP. Les deux s'appliquent aussitôt à la session ouverte. La limite propre à une tâche torrent vaut à sa prochaine reprise.
- **Choix des fichiers** : `task.setFiles` et `include: files` (section 6.5).
  - Une exécution en cours relit le choix à chaque seconde et le transmet à librqbit.
  - Seuls les fichiers choisis sont vidés et renommés à la fin.
  - Un fichier non choisi garde les octets des morceaux qu'il partage avec un fichier choisi, sous son `.part`, qui part avec la tâche.
- **Réglages** : `torrent.ratioLimit` et `torrent.upnp` (section 6.5).
- **Tests** : partage jusqu'au ratio avec un pair local qui vient télécharger, pause pendant le partage, reprise du partage après un redémarrage du moteur, partage retenu sur un réseau facturé puis repris, choix des fichiers en cours de téléchargement ; protocole (limites, ratio, UPnP, réseau facturé, `task.setFiles`).
- **Essai réel** (2026-11-09, WSL) : `aurod` d'essai et la CLI. Netinst Debian 13.7 (756 Mio) par un lien magnet sans tracker (DHT seul), ratio 0, envoi limité à 100 Kio/s : terminé en 27 s, empreinte publiée juste. La table du DHT ne s'est pas enregistrée en si peu de temps (librqbit l'écrit toutes les 60 s).
- **Reste après l'étape 3b** : le banc de reprise des torrents (3c) et la liaison à une interface réseau, Windows compris (3d). La durée de partage (`torrents.seed_time_limit_s`) n'est pas encore suivie.

**Mise en œuvre de l'étape 3c (2026-11-09) : banc de reprise des torrents.**
- **Banc** (`crates/daemon/tests/crash_torrent.rs`) : `aurod` télécharge plusieurs torrents à la fois depuis un pair local (`testing::swarm`, librqbit, envoi bridé pour que chaque cycle tombe en plein téléchargement). Le banc en ajoute par le protocole (`.torrent` en base64) pour en garder toujours trois en cours : un seul fichier ou un dossier, avec des sous-dossiers et des fichiers vides, sans partage (ratio 0).
  - Chaque cycle tue `aurod` à un instant aléatoire, sauf un sur cinq qui finit par `engine.shutdown`.
  - Un arrêt sur deux est suivi d'une écriture perdue simulée : une plage de 64 Kio au plus mise à zéro dans un `.part` au hasard, comme un cache du système perdu avec la machine.
  - Après chaque arrêt : aucune tâche en échec ni annulée ; chaque torrent terminé a tous ses fichiers sous leur nom final, identiques octet par octet au contenu partagé, et plus aucun `.part`. Le banc compte les arrêts tombés entre le renommage et l'enregistrement de la fin, que la reprise doit achever.
  - À la fin, `aurod` reprend tout sans arrêt jusqu'au bout, et chaque torrent est vérifié.
  - Réglages : `AURO_CRASH_CYCLES` (6 par défaut, version courte de `cargo test`), `AURO_CRASH_TASKS`, `AURO_CRASH_SIZE`, `AURO_CRASH_SECONDS`, `AURO_CRASH_SEED` pour rejouer un échec.
- **`aurod --torrent-peers`** (essais seulement) : les torrents ne passent que par ces pairs, sans DHT ni connexion entrante.
- **Campagnes** : voir la section 9.1.
- **Constaté** : sans dossier de destination explicite, librqbit (9.1.0) range sans son dossier un torrent « à plusieurs fichiers » qui n'en contient qu'un. Le moteur donne toujours le dossier et garde celui du torrent (test `a_folder_torrent_with_one_file_keeps_its_folder`) ; le pair de test, qui ne le donnait pas, ne trouvait pas ses données, d'où des torrents bloqués à 0 octet dans la première campagne.
- **Reste après l'étape 3c** : la liaison à une interface réseau, Windows compris (3d). La durée de partage (`torrents.seed_time_limit_s`) n'est pas encore suivie.

**Mise en œuvre de l'étape 3d (2026-11-09) : liaison à une interface réseau.** Décisions de l'utilisateur du même jour : copies corrigées de librqbit pour Windows, torrents en attente quand l'interface manque, liaison des seuls torrents.
- **Réglage** `torrent.interface` : le nom de l'interface (« wg0 », « tun0 » ; sous Windows, le nom affiché de la carte, celui du VPN), ou `null` pour aucune (section 6.5). Les téléchargements HTTP suivent toujours les routes du système.
- **Liaison** : la session librqbit reçoit l'interface (`bind_device_name`). DHT, connexions aux pairs, trackers UDP et HTTP, et écoute ne passent que par elle.
  - Sous Linux et Android : `SO_BINDTODEVICE`, par librqbit. Sous Android, ou sur un noyau Linux antérieur à 5.7, le système peut le refuser à une application : la session ne s'ouvre pas (`TORRENT_SESSION`). Pas encore essayé sur un appareil.
  - Sous Windows, que librqbit 9.1.0 refuse (`BindDeviceNotSupported`) : copies corrigées de `librqbit-dualstack-sockets` 0.7.0 et de `librqbit` 9.1.0 dans `third_party/` (`[patch.crates-io]`, détail dans `third_party/README.md`).
    - Chaque socket reçoit `IP_UNICAST_IF` (`IPV6_UNICAST_IF` en IPv6), puis se lie à l'adresse de l'interface : IPv4 d'abord, IPv6 sinon. Par le modèle d'hôte strict de Windows, elle n'envoie et ne reçoit que par l'interface.
    - Une connexion vers un pair d'une famille d'adresses que l'interface n'a pas est refusée, jamais tentée ailleurs.
    - Les requêtes HTTP des trackers partent de l'adresse de l'interface.
  - Les noms des trackers et des nœuds DHT de départ se résolvent par le DNS du système, que règle le VPN.
- **Surveillance** (`scheduler`) : toutes les 2 s (`INTERFACE_CHECK`), le moteur cherche l'interface par son nom (`torrent::system_interface`, crate `network-interface`). Elle est là si elle a au moins une adresse hors adresses locales au lien (169.254/16, fe80::/10, celles d'une carte VPN déconnectée sous Windows).
  - Absente : aucun torrent ne démarre, partage compris. Une tâche restée `active` par un arrêt brutal retourne en file.
  - Disparue, revenue ou changée (index ou adresses : VPN reconnecté) : les torrents en cours s'arrêtent et retournent en file ; un partage reste `seeding`. Ils repartent sur une nouvelle session, liée à l'interface du moment, quand elle est là et que les torrents de la session précédente l'ont tous quittée (génération de l'interface).
  - Changer le réglage fait de même aussitôt.
  - Entre deux vérifications, les sockets restent liées à l'interface disparue : rien ne passe par une autre.
  - Une exécution qui trouve l'interface absente à l'ouverture de la session attend, sans échouer, que le planificateur l'arrête.
- **Protocole** : `sync.snapshot` porte `torrentInterfaceUp`, et `engine.torrentInterface` en annonce les changements (section 6.5). Les interfaces s'en servent pour montrer pourquoi un torrent attend (étape 5).
- **Tests** : interfaces simulées, liées en vrai à l'interface de bouclage, où écoute le pair local. Ils couvrent :
  - un torrent qui attend son interface, puis part quand elle apparaît ;
  - une interface perdue en plein téléchargement, puis revenue avec un autre index : le torrent s'arrête, attend et finit sans défaut ;
  - une interface changée sans disparaître : le torrent repart ;
  - le réglage changé en cours de route, vers une interface absente puis vers aucune ;
  - la vraie interface de bouclage, trouvée par son nom : le pair local est joint ;
  - une session liée à une carte réseau : le pair local, qui n'est que sur l'interface de bouclage, n'est jamais joint ;
  - le protocole (réglage, valeurs refusées, instantané, notification).
- **Essai réel** (2026-11-09, Windows 11) : VPN de l'utilisateur (« Ma Protection » du Pack Vie Digitale, carte Wintun `FSVpnSDK-HydraTunnel`, une adresse IPv4 et une adresse locale au lien), `aurod` d'essai lié à elle, netinst Debian 13.7 (756 Mio) par lien magnet (DHT seul), ratio 0, envoi limité à 100 Kio/s, réception à 2 Mio/s. Relevé toutes les 5 s : jusqu'à 263 connexions TCP et les deux sockets UDP, toutes sur l'adresse du VPN, aucune ailleurs. À 4,4 %, l'utilisateur a coupé le VPN : le torrent est revenu en file en moins de 5 s, sans plus aucune connexion ; au retour du VPN, il est reparti seul sur une nouvelle session, liée à lui. Terminé en 7 min 20 s, empreinte publiée juste.
- **Reste de l'étape 3** : la durée de partage (`torrents.seed_time_limit_s`) n'est pas encore suivie ; la liaison sous Android n'a pas été essayée sur un appareil, et l'application Android ne la propose pas (décision de l'utilisateur du 2026-11-10, étape 5e : le VPN d'Android vaut pour toute l'application ; section 4.4).

### 7.17 Vidéo (phase 5, conception du 2026-11-09)

Module `media` du moteur : extraction par yt-dlp, téléchargement, fusion par ffmpeg (F-22 à F-25). Windows et Linux seulement en v1 : le moteur Android n'annonce pas la capacité `media`, et ses méthodes y rendent `CAPABILITY_MISSING`.

**Décisions de l'utilisateur (2026-11-09).**
- Architecture mixte. yt-dlp ne fait qu'extraire : adresses des flux, en-têtes, formats. Le moteur télécharge lui-même les flux directs, avec la règle 1, les limites, la reprise vérifiée et les segments. Il ne confie à yt-dlp que les flux fragmentés (HLS, DASH à fragments). ffmpeg fusionne ensuite les flux.
- yt-dlp et ffmpeg sont téléchargés au besoin, avec l'accord de l'utilisateur, depuis leurs publications officielles, empreinte publiée vérifiée. Rien de plus dans l'installateur.
- yt-dlp se met à jour automatiquement : au plus une fois par jour, empreinte et signature du projet vérifiées, remplacement seulement entre deux exécutions.
- Essais réels sur des vidéos sous licence libre (PeerTube, Wikimedia Commons, Internet Archive), jamais YouTube.

**Outils** (`media::tools`). Dossier `tools/` à côté de la base.
- **yt-dlp** : fichier de la plateforme publié par le projet (`github.com/yt-dlp/yt-dlp/releases`) :
  - Windows : `yt-dlp.exe`, ou `yt-dlp_arm64.exe` sur ARM ;
  - Linux : `yt-dlp_linux`, ou `yt-dlp_linux_aarch64`.
  - Le moteur télécharge aussi `SHA2-256SUMS` et sa signature `SHA2-256SUMS.sig`. Il vérifie la signature avec la clé publique du projet, intégrée au moteur (`public.key` du dépôt de yt-dlp, empreinte notée dans le code), puis l'empreinte du fichier. Le fichier est écrit sous un nom provisoire, vidé sur disque, puis renommé.
- **ffmpeg et ffprobe** :
  - Windows : version stable des builds BtbN (`ffmpeg-n8.1-latest-win64-gpl-8.1.zip`, `github.com/BtbN/FFmpeg-Builds`). Empreinte vérifiée dans `checksums.sha256` de la même publication ; les deux exécutables sont extraits de l'archive.
  - Linux : `ffmpeg` et `ffprobe` du système s'ils sont dans le `PATH`, sinon la même publication BtbN (`linux64` ou `linuxarm64`, `.tar.xz`).
  - BtbN ne signe pas ses builds : l'empreinte ne protège que contre un fichier abîmé, pas contre une publication compromise. Limite acceptée, à revoir si une source signée apparaît.
- **Installation** : jamais d'elle-même. `media.tools` rend l'état des outils : présents ou non, version, origine (téléchargés ou système). `media.installTools` les installe, sur l'accord que l'interface a demandé à l'utilisateur. Sans outils, `media.probe` et l'ajout d'une vidéo rendent `TOOLS_MISSING`.
- **Mise à jour de yt-dlp** (F-25) : réglage `media.autoUpdate` (vrai par défaut), désactivable comme toute vérification de mise à jour (section 8.2).
  - Au plus une fois par 24 heures, à l'occasion d'un `media.probe` ou d'un ajout, le moteur lit la dernière publication. Si elle diffère de la version installée, il la télécharge et la vérifie comme à l'installation.
  - Il la met en place quand aucun yt-dlp ne tourne, et l'annonce par `engine.warning` (`YTDLP_UPDATED`). Un échec garde l'ancienne version (`engine.warning`, `YTDLP_UPDATE_FAILED`).
  - ffmpeg ne se met à jour qu'à la demande (`media.installTools`).
- **Lancement des processus** :
  - arguments passés un à un, jamais par un interpréteur de commandes ;
  - l'adresse de la page placée après `--`, pour qu'elle ne puisse pas passer pour une option ;
  - adresses `http` et `https` seulement ;
  - sans fenêtre de console sous Windows ;
  - tués avec tout leur arbre quand leur exécution s'arrête ou dépasse son délai : yt-dlp publié est un « onefile » PyInstaller, qui lance lui-même l'interpréteur Python. Sous Windows, un Job Object (`KILL_ON_JOB_CLOSE`) les tue aussi si `aurod` meurt brutalement ; sous Linux, un groupe de processus, et `PR_SET_PDEATHSIG` (`SIGTERM`) : un `aurod` tué brutalement ne tue pas le groupe, mais le processus lancé reçoit alors `SIGTERM`, que le chargeur PyInstaller transmet à l'interpréteur (étape 4d ; sans lui, le banc de reprise des vidéos trouvait des fichiers de flux écrits après la fin de leur vidéo).
  - Les messages de yt-dlp ne citent pas l'adresse dans les journaux du moteur (section 8.2).

**Extraction** (`media.probe`, portée `add`).
- `yt-dlp -J --no-playlist -- <pageUrl>`, délai de 90 s : titre, durée, extracteur, vignette.
- `formats[]` avec, pour chaque format : `formatId`, `ext`, codecs vidéo et audio, `width`, `height`, `fps`, débit, taille (exacte ou estimée), `protocol`, `drm`.
- Langues des sous-titres.
- Une page de liste de lecture ne rend que sa vidéo (`--no-playlist`) ; les listes viendront plus tard, par l'analyse de liens (F-29).
- Échec : `EXTRACTOR_FAILED`, avec le dernier message de yt-dlp, sans l'adresse.

**Ajout** (`task.add` avec `media`).
- `media` porte `pageUrl` et éventuellement :
  - `format` (sélecteur de yt-dlp ; par défaut `bv*+ba/b`, la meilleure vidéo et le meilleur audio) ;
  - `container` (`mp4`, `mkv`, `webm` ; par défaut, automatique) ;
  - `subtitles` (langues).
- La tâche est de genre `media`, avec sa ligne `media_jobs` (migration 1). Elle prend le titre de la vidéo pour nom d'affichage dès la résolution.
- Un format protégé par DRM est refusé (section 8.3) : le moteur ne le télécharge pas, même si yt-dlp le proposait.

**Exécution** (`media::run`). États : `queued` → `active` (résolution et flux) → `post_processing` (fusion) → `completed`.
- **Résolution** : `yt-dlp -J -f <format> -- <pageUrl>` donne les flux retenus (`requested_formats`, ou le format seul). Chaque flux est enregistré comme fichier de la tâche (`task_files`), avec son genre (vidéo, audio, sous-titres), son protocole et son nom provisoire. Les adresses et en-têtes des flux ne sont gardés qu'en mémoire : ils expirent, et un `Cookie` fourni par yt-dlp est un secret (section 5.1). Chaque exécution refait la résolution.
- **Flux direct** (`http`, `https`, sans fragments) : téléchargé par le moteur, comme une tâche HTTP, dans `<nom>.f<id>.<ext>.part`.
  - Les règles habituelles s'appliquent : segments et règle 1 (base `segments` du flux), limites, reprise par `If-Range`, nom de fichier nettoyé.
  - Les en-têtes de yt-dlp s'appliquent (`User-Agent`, `Referer`…).
  - Une adresse expirée (`403`, `410`) fait refaire la résolution, au plus trois fois, puis le téléchargement reprend sur la nouvelle adresse.
- **Flux fragmenté** (`m3u8`, `m3u8_native`, `http_dash_segments`…) : confié à `yt-dlp -f <id> --newline --progress-template … -o <fichier du flux>`. Le moteur lit l'avancement sur la sortie de yt-dlp et le publie comme celui d'un flux.
  - Pause : yt-dlp est arrêté ; à la reprise, il est relancé et reprend ses fichiers partiels.
  - À la fin, le moteur vide le fichier sur disque avant d'enregistrer le flux comme terminé.
  - Ici la règle 1 ne vaut qu'à la fin du flux. Une coupure de la machine pendant ce flux peut laisser des fragments perdus que yt-dlp ne revérifie pas. Limite acceptée avec l'architecture mixte ; ffprobe contrôle le fichier fusionné.
- **Fusion** (`post_processing`, F-23) : `ffmpeg -i <flux>… -map … -c copy` vers un fichier provisoire, sans réencodage.
  - Conteneur : celui demandé, sinon `mp4` si tous les flux sont `mp4` ou `m4a`, `webm` si tous sont `webm`, `mkv` sinon.
  - `ffprobe` vérifie que le fichier produit se lit et qu'il a la durée annoncée, à 2 s près. Le fichier est vidé sur disque, renommé à son nom final (`nom (1).ext` si le nom est pris), puis les fichiers des flux sont effacés.
  - Un flux unique est seulement contrôlé par `ffprobe` et renommé, sans ffmpeg.
  - Une coupure pendant la fusion la recommence : le fichier provisoire est effacé au démarrage.
- **Sous-titres** (F-24, priorité C) : téléchargés par le moteur, gardés à côté de la vidéo (`<nom>.<langue>.<ext>`). Sous-étape à part, après le reste (étape 4e, détail plus bas).
- **Erreurs** :
  - `EXTRACTOR_FAILED` : la résolution échoue ;
  - `TOOLS_MISSING` (nouveau, non réessayé) : yt-dlp ou ffmpeg absent ;
  - `MERGE_FAILED` (nouveau, non réessayé) : ffmpeg échoue, ou ffprobe refuse le fichier produit ;
  - les erreurs réseau et disque des flux sont celles d'une tâche HTTP.

**Protocole** : `media.probe`, `media.tools`, `media.installTools` (portée `admin`), `task.add` avec `media`, réglage `media.autoUpdate`, `engine.warning` (`YTDLP_UPDATED`, `YTDLP_UPDATE_FAILED`, `SUBTITLES_MISSING`). Capacité `media` annoncée sous Windows et Linux. Détail en section 6.5, à chaque sous-étape.

**Tests** :
- Faux yt-dlp et faux ffmpeg : petits programmes de test qui rendent une extraction donnée et écrivent des fichiers, sur un serveur HTTP local.
- Faux serveur de publications (outils, empreintes, signature faite par une clé de test).
- Tests ignorés par défaut avec les vrais outils (`AURO_YTDLP`, `AURO_FFMPEG`), sur une page locale avec une vidéo directe et une liste HLS.
- Banc de reprise des vidéos sous arrêt brutal, comme ceux des tâches HTTP et des torrents.
- Essais réels sous Windows et Linux.

**Sous-étapes** :
- 4a. Outils : installation, vérification (signature, empreintes), mise à jour, `media.tools`, `media.installTools`.
- 4b. Extraction : `media.probe`, mise à jour de yt-dlp à cette occasion, `engine.warning`.
- 4c. Ajout et flux : `task.add` avec `media`, `media_jobs`, résolution ; flux directs par le moteur, fragmentés par yt-dlp, pause et reprise, adresse expirée. (L'ajout est passé de 4b à 4c le 2026-11-09 : une tâche vidéo ajoutée sans exécution serait lancée par le planificateur sans rien savoir en faire.)
- 4d. Banc de reprise des vidéos (arrêts pendant les flux et la fusion) ; limite de débit des flux fragmentés. (La fusion et son contrôle sont faits dès 4c, le 2026-11-09 : sans eux, une vidéo ne finissait pas.)
- 4e. Essais réels ; sous-titres (F-24).

**Mise en œuvre de l'étape 4a (2026-11-09) : outils.**
- **Signature** (`media::openpgp`) : lecture d'un paquet de signature OpenPGP v4 et d'une clé RSA v4. Seuls les cas de yt-dlp sont acceptés (signature binaire détachée, RSA, SHA-256 ou SHA-512) ; tout autre format est refusé.
  - La vérification RSA (PKCS #1 v1.5) est celle d'aws-lc, déjà dans le moteur par rustls et librqbit.
  - L'émetteur doit être la clé attendue, par son empreinte ou son identifiant.
  - La clé de yt-dlp est `crates/engine/src/media/yt-dlp-public.key`, copie exacte de `public.key` de son dépôt. Son empreinte, AC0C BBE6 848D 6A87 3464 AF4E 57CF 6593 3B5A 7581 (RSA 4096, relevée par `gpg` le 2026-11-09), est contrôlée au chargement.
- **Outils** (`media::tools`) :
  - L'installation suit la section ci-dessus. Les fichiers téléchargés sont écrits sous un nom provisoire, contrôlés pendant l'écriture, vidés sur disque, puis renommés. Le moteur ne tire de l'archive de ffmpeg que `bin/ffmpeg` et `bin/ffprobe`, et l'efface ensuite.
  - L'état est gardé dans `tools/tools.json` (version et empreinte de chaque outil, dernière recherche de mise à jour), écrit de la même façon. Au démarrage, un outil enregistré dont le fichier manque est oublié.
  - La mise à jour de yt-dlp (`update_ytdlp_if_due`) remplace le fichier sous un verrou en écriture : elle attend qu'aucune exécution de yt-dlp ne tienne le verrou en lecture.
  - Sous Linux, le ffmpeg du système est préféré s'il est dans le `PATH`, avec son ffprobe à côté.
- **Moteur et protocole** : `Engine::media_tools`, `Engine::install_media_tools` (en arrière-plan, une installation à la fois), `media.tools`, `media.installTools`, notification `media.tools`, réglage `media.autoUpdate` (section 6.5). Hors de Windows et Linux, pas d'outils : `CAPABILITY_MISSING`.
- **Dépendances** :
  - `aws-lc-rs` en dépendance directe (déjà présente) ;
  - `zip` (MIT, sous Windows) ;
  - `tar` (MIT ou Apache-2.0) et `xz2` (MIT ou Apache-2.0, avec liblzma ; sous Linux).
  - `cargo deny check licenses` passe.
- **Tests** :
  - **Vraie signature** : `SHA2-256SUMS` de la publication 2026.10.26 de yt-dlp et sa signature, gardés tels quels dans `testdata/`. `.gitattributes` empêche Git d'en convertir les fins de ligne, ce qui casserait la signature. Le test vérifie aussi qu'un octet changé, une fin de ligne Windows ou une autre clé font refuser le fichier.
  - **Faux serveur de publications** (`testing::tools`, clé de test, archives fabriquées par le test). Il couvre :
    - l'installation complète ;
    - une liste d'empreintes signée par une autre clé ;
    - un outil ou une archive différents de leur empreinte publiée ;
    - une archive sans ffprobe ;
    - la mise à jour au plus une fois par jour, retenue à travers un redémarrage ;
    - une mise à jour à la signature fausse, qui garde l'ancienne version ;
    - une mise à jour qui attend la fin d'un yt-dlp en cours ;
    - une seule installation à la fois.
  - **Protocole** : portée `admin`, aucune requête avant la demande, notification de fin, réglage.
- **Essai réel** (2026-11-09, test ignoré `real_tools_install_from_the_official_releases`), depuis les publications officielles :
  - Windows 11 : yt-dlp 2026.10.26 (23,6 Mo), ffmpeg et ffprobe 8.1 de BtbN (165 Mo chacun), en 50 s ;
  - Linux (WSL, sans ffmpeg dans le système) : yt-dlp 41,2 Mo, ffmpeg et ffprobe 173 Mo chacun, en 55 s.
- **Reste pour 4b** : déclencher la mise à jour de yt-dlp à l'occasion d'un `media.probe`, et l'annoncer (`engine.warning`) ; fait en 4b.

**Mise en œuvre de l'étape 4b (2026-11-09) : extraction.**
- **`media::extract`** : `yt-dlp --ignore-config --no-cache-dir --no-color -J --no-playlist --no-warnings -- <adresse>`, délai de 90 s.
  - La réponse est réduite à ce que montre `media.probe`. Une liste de lecture est refusée, une page sans format aussi.
  - Un format marqué `has_drm` (vrai, ou « maybe ») est signalé `drm`. Les sous-titres automatiques ne sont pas proposés.
  - Message d'erreur : le dernier `ERROR:` de yt-dlp, sinon sa dernière ligne. Les adresses y sont remplacées par `<adresse>`, et il est raccourci à 300 caractères.
- **`media::process`** : arbre de processus tué, comme décrit plus haut. Le test d'un faux yt-dlp qui dépasse son délai le montre : sous Windows, son processus enfant gardait la sortie ouverte 20 s ; tué avec l'arbre, l'essai finit en 0,6 s.
- **Moteur et protocole** : `Engine::probe_media` vérifie l'adresse, lance la mise à jour de yt-dlp en arrière-plan si `media.autoUpdate` le permet, puis extrait en gardant yt-dlp pendant l'extraction (une mise à jour attend). La mise à jour s'annonce par `engine.warning` (`YTDLP_UPDATED`, ou `YTDLP_UPDATE_FAILED` si l'ancienne version reste), puis par `media.tools`.
- **Essais** : `Sources::local_ytdlp` remplace yt-dlp par un faux (`testing::tools::fake_ytdlp` : script `.cmd` sous Windows, `sh` sous Linux, qui note ses arguments). Les tests couvrent :
  - la lecture d'une réponse représentative (flux séparés, HLS, DRM, sous-titres), d'une vidéo à format unique et d'une liste de lecture ;
  - des erreurs sans adresse ;
  - l'adresse placée après `--` ;
  - un yt-dlp lent arrêté à son délai ;
  - une sortie illisible et un programme absent ;
  - par le protocole : portée `add`, `TOOLS_MISSING`, `EXTRACTOR_FAILED` sans l'adresse, adresses refusées ;
  - la mise à jour à l'occasion d'un `media.probe`, empêchée par `media.autoUpdate` à faux, puis annoncée.
- **Essai réel** (2026-11-09, test ignoré `real_probe_of_free_videos`, yt-dlp 2026.10.26 officiel), sur Windows 11 :

  | Page | Titre | Formats | Hauteur max | Sous-titres |
  |---|---|---|---|---|
  | PeerTube de la Blender Foundation (CC BY-SA, 75 s) | Inverse Kinematics Bone Constraint | 1, direct | 400 | aucun |
  | Wikimedia Commons | Big Buck Bunny 4K | 5, directs | 2250 | 31 langues |
  | Internet Archive | Big Buck Bunny | 3, directs | 720 | aucun |

  Chaque extraction prend 4 à 5 s, et aucun format n'est protégé par DRM. Aucune de ces pages n'a de flux HLS : il en faudra une pour l'étape 4c.

**Mise en œuvre de l'étape 4c (2026-11-09) : ajout, flux et fusion.** Elle s'écarte sur un point de la conception ci-dessus : les flux ne sont pas des fichiers de la tâche (`task_files`).
- **Modèle** (migration 6, section 5.4) : les flux sont dans `media_streams`. Chaque flux direct se télécharge comme une **tâche HTTP interne**, rattachée à la vidéo par `tasks.parent_id`.
  - Elle passe par `transfer::run`, inchangé : règle 1, segments, limites (seaux de la vidéo), reprise par `If-Range`, banc de reprise.
  - Le planificateur ne la lance jamais (`runnable_tasks`). Les listes, les comptes de file, les nouvelles tentatives et l'historique l'ignorent ; elle disparaît avec sa vidéo (`ON DELETE CASCADE`).
  - Ses en-têtes viennent de yt-dlp (`http_headers`). Ceux que le moteur fixe lui-même sont écartés, et les secrets (`Cookie`…) vont au coffre (section 5.1).
- **Exécution** (`media::run`) :
  - La résolution se refait à chaque exécution, puisque les adresses expirent. La première retient le titre (nom affiché), le conteneur et le nom final, pris sur le disque (`.part`) comme pour une tâche HTTP. Les suivantes demandent les mêmes formats (`id1+id2`) ; si la page ne les propose plus, échec `EXTRACTOR_FAILED`.
  - Les flux se téléchargent un par un.
  - Un flux direct reçoit à chaque exécution son adresse fraîche (`replace_url`), ses validateurs restant pour `If-Range`. Un `HTTP_STATUS` (adresse expirée) fait refaire la résolution, au plus trois fois par exécution. Un `FILE_CHANGED` fait repartir le flux de zéro, une fois. Les autres issues (pause, place, nouvelle tentative, échec) deviennent celles de la vidéo.
  - Un flux fragmenté passe par `yt-dlp -f <id> --continue -o <fichier>` : modèle d'avancement `--progress-template`, `%` doublés dans le chemin, `--ffmpeg-location`, arbre tué à l'arrêt. À la fin, le fichier est vidé sur disque avant d'être enregistré.
  - L'avancement de la vidéo est la somme de ses flux ; les flux finis sont enregistrés comme indication (`done_bytes`).
- **Fusion** (`post_processing`, nouvelle transition `active` → `post_processing`, section 5.5) :
  - `ffmpeg -c copy -f <format>` vers le `.part` du nom final, puis `ffprobe` (durée annoncée, à 2 s près). Le fichier est vidé sur disque, renommé, et les flux sont effacés.
  - Un flux seul déjà dans le bon conteneur est seulement contrôlé et renommé.
  - Un arrêt pendant la fusion la recommence au démarrage : `post_processing` fait partie des tâches reprises.
  - Un arrêt après le renommage du fichier final et avant l'enregistrement de la fin (flux en cours d'effacement, ou tous effacés) : au démarrage, un flux manquant à côté du fichier final veut dire que la fusion est faite ; les flux restants sont effacés et la fin est enregistrée (`already_merged`, porte de la phase 5, 2026-11-10). Jusque-là, la fusion était relancée sur des flux effacés, et la vidéo, pourtant complète, finissait en échec `MERGE_FAILED` (trouvé par le banc des vidéos sous Windows, 3 passages sur 8).
  - Un outil (ffmpeg, ffprobe) qui échoue sans message est nommé, avec son code de sortie (le message de repli nommait yt-dlp).
  - Sous Windows, vider un fichier ouvert en lecture seule est refusé : il est ouvert en écriture, sans troncature.
- **Annulation et retrait** : les fichiers des flux, leurs `.part`, ceux des tâches internes et le `.part` du nom final sont effacés.
- **Protocole** : `task.add` avec `media`, et la capacité `media` (section 6.5).
- **Tests** :
  - Faux outils (`fake_ytdlp_answers`, `fake_ffmpeg` : ffmpeg met ses entrées bout à bout, ffprobe rend une durée), sur un site local :
    - deux flux directs donnent une vidéo identique octet par octet, sans flux ni `.part` restant ; les en-têtes de yt-dlp sont envoyés, sauf ceux du moteur ; une seule tâche visible, une seule ligne d'historique ;
    - pause et reprise en plein flux, sans perte ;
    - adresse expirée (`403`), résolue de nouveau ;
    - outils absents ; format protégé par DRM refusé avant toute requête ;
    - annulation qui efface tout.
  - Protocole : vérifications de `task.add` avec `media`, `kind: media`, capacité.
- **Essai avec les vrais outils, sans Internet** (test ignoré `real_local_videos_are_downloaded_and_merged`, outils de `AURO_TOOLS_DIR`). ffmpeg fabrique une vidéo de 6 s (mire et son), servie en local de trois façons, chacune téléchargée et contrôlée (image et son présents) :
  - un fichier mp4 seul : tâche interne, gardé tel quel ;
  - une liste HLS à image et son séparés : deux flux par yt-dlp, puis fusion ;
  - un manifeste DASH à fichiers directs (`BaseURL`) : deux tâches internes, puis fusion.
  
  Passé sous Windows 11 et Linux (WSL), avec yt-dlp 2026.10.26 et ffmpeg 8.1. Bancs de reprise longs passés sous Linux le même jour (crash_daemon, 150 cycles, 67 fichiers ; crash_resume, 300 cycles, 71 fichiers), les tâches internes et les requêtes du planificateur ayant changé. Le DASH « fichier unique » que fabrique ffmpeg (`-single_file`) fait échouer l'extracteur de yt-dlp (`KeyError: sourceURL`) : l'essai écrit son manifeste à la main.
- **Reste pour 4d** : banc de reprise des vidéos (arrêts brutaux pendant les flux et la fusion) ; limite de débit des flux fragmentés (yt-dlp ne suit pas encore les seaux du moteur).

**Mise en œuvre de l'étape 4d (2026-11-09) : banc de reprise et limite des flux fragmentés.**
- **Banc** (`crates/daemon/tests/crash_media.rs`, section 9.1) : `aurod` tué pendant les flux et la fusion. Les outils lui sont imposés par `--media-ytdlp`, `--media-ffmpeg` et `--media-ffprobe` (essais seulement).
- **Défauts trouvés et corrigés grâce au banc** :
  - **Vidéo bloquée en `post_processing`** : une vidéo arrêtée pendant sa fusion ne repartait jamais. Le planificateur prenait `post_processing` pour un état neuf, tentait `post_processing` → `active`, transition interdite, et la mettait de côté. Il la reprend maintenant telle quelle.
  - **Fusion non annoncée** : le passage à `post_processing` n'était pas annoncé. Il l'est désormais, comme `verifying` : nouveau champ `Progress::merging`, et note `RunNote::PostProcessing`.
  - **Fichiers de flux restants sous Linux** : un `aurod` tué laissait tourner ses outils, qui pouvaient écrire un fichier de flux après la fin de leur vidéo (2 fichiers restants en 150 cycles). Corrigé par `PR_SET_PDEATHSIG`, comme décrit plus haut.
- **Limite de débit des flux fragmentés** : yt-dlp reçoit `--limit-rate`, la plus petite des limites de la tâche, de sa file et globale, lue au lancement. Ce n'est pas un seau partagé avec les autres téléchargements, qui peuvent donc ensemble dépasser la limite globale ; un changement de limite vaut au flux suivant.
- **Tests** : flux fragmenté par yt-dlp (`-o`, `--limit-rate` à la plus petite limite) suivi de la fusion ; passage annoncé en `post_processing`.
- **Reste pour 4e** : essais réels sur PeerTube, Wikimedia Commons et Internet Archive ; sous-titres (F-24).

**Mise en œuvre de l'étape 4e (2026-11-09) : sous-titres et essais réels.**
- **Sous-titres** (F-24 ; décisions de l'utilisateur du 2026-11-09 : à côté de la vidéo, au format du site ; une langue qui manque ne bloque pas la vidéo, elle est signalée) :
  - **Choix du format** (`media::extract`) : pour chaque langue demandée, parmi les sous-titres de la page (jamais les automatiques), le premier de `srt`, `vtt`, `ass`, `ssa`, `ttml`, `dfxp` qui se télécharge d'un seul tenant (`http`, `https`, sans fragments) ou que yt-dlp donne en ligne (`data`, 8 Mio au plus). Pas de conversion. Les formats propres à un site (`json3`, `srv3`…) et les sous-titres seulement en HLS ou DASH sont écartés : le moteur ne les télécharge pas par yt-dlp. Un code de langue hors lettres, chiffres, `-` et `_` est écarté (il entre dans un nom de fichier). `media.probe` ne propose que les langues ainsi téléchargeables.
  - **Modèle** (migration 7, section 5.4) : un sous-titre est une ligne de `media_streams` après les flux, avec sa langue, créée à la première résolution si la page le propose. Les en-têtes sont ceux du sous-titre, sinon ceux de la page.
  - **Téléchargement** : une fois les flux complets, avant la fusion. Un sous-titre à télécharger devient une tâche HTTP interne, comme un flux direct, sous `<nom>.<langue>.<ext>` (le nom final de la vidéo sans son extension) ; un sous-titre donné en ligne est écrit sous un nom libre, vidé sur disque puis renommé. Ils ne comptent ni dans la taille ni dans l'avancement de la vidéo.
  - **Sous-titre manquant** : absent de la page, ou refusé par le site (`HTTP_STATUS` après une nouvelle résolution, au plus trois en tout comme pour les flux ; `AUTH_REQUIRED`, `AUTH_FAILED`, `FILE_CHANGED`) : la vidéo se termine sans lui. À la fin, le planificateur annonce `engine.warning` `SUBTITLES_MISSING`, avec `taskId` et les langues manquantes. Une coupure passagère, un disque plein ou un arrêt traitent la vidéo comme pour un flux.
  - **Fusion** : les sous-titres n'y entrent pas.
  - **Effacement** : les sous-titres d'une vidéo terminée sont des fichiers finaux : un retéléchargement sans effacer les garde, un retrait ou un retéléchargement avec les fichiers les efface. Ceux d'une vidéo inachevée partent avec ses flux.
  - **Limite** : « Déplacer/Renommer » (section 7.12) ne déplace que la vidéo, pas ses sous-titres.
- **Défauts trouvés par les essais réels et corrigés** :
  - **Sous-titres invisibles** sur PeerTube : l'extracteur de yt-dlp ne les cherche que s'ils sont demandés. Le moteur lit donc les pages avec `--write-subs` (`-J` reste une simulation, rien n'est écrit).
  - **`If-Range` refusé** par `upload.wikimedia.org` (ETag sans guillemets) : toute vidéo de Commons, et tout fichier de ce serveur, échouait en `FILE_CHANGED`. Corrigé dans `net` (section 7.4), pour toutes les tâches HTTP.
  - **Plages annoncées puis ignorées** par l'API de Commons (sous-titres) : `Accept-Ranges: bytes` au `HEAD`, sans validateur, puis `200` à toute plage ; la vidéo échouait en `RANGE_UNSUPPORTED`. Un `HEAD` sans validateur ne suffit plus à prouver les plages (section 7.1, décision de l'utilisateur), pour toutes les tâches HTTP.
- **Défaut de 4c corrigé** : les tâches internes d'une vidéo cherchaient une empreinte dans un fichier voisin (`downloads.neighborChecksum`, section 7.9), soit des requêtes inutiles vers les serveurs des vidéos. Elles ne la cherchent plus.
- **Essais réels** (test ignoré `real_free_videos_are_downloaded_to_the_end`, vrais yt-dlp et ffmpeg) : flux direct seul (PeerTube de Blender ; Internet Archive, format `1`), flux direct et sous-titres WebVTT avec une langue absente (Wikimedia Commons, format `0`), HLS par yt-dlp et son seul direct fusionnés en mp4 avec un sous-titre (Framatube, PeerTube de Framasoft : « Bruce Schneier introduces FSF EncryptFest », CC BY, formats `3+Audio_only-0`).
- **Tests** : choix du format des sous-titres ; vidéo avec sous-titre téléchargé, sous-titre en ligne, refusé, seulement en HLS et absent, avertissement, retéléchargement et retrait ; `taskId` de `engine.warning` ; `If-Range` avec une `ETag` sans guillemets (serveur strict) ; plages annoncées sans validateur puis ignorées.
- **Résultats** (2026-11-09) : essai réel passé sous Windows et sous Linux (WSL), les quatre vidéos complètes, durées vérifiées par ffprobe, sous-titres `en` et `fr` de Commons et `en` de Framatube à côté, `xx` signalé, dossier sans flux ni `.part`. Bancs sans défaut : vidéos, 150 cycles (Windows 124 vidéos, Linux 132) ; `aurod`, 150 cycles (Linux, 56 tâches) ; reprise, 300 cycles (Linux, 73 fichiers).

### 7.18 Analyse de liens (phase 6, étape 2, conception du 2026-11-11)

F-29 (texte ou page, filtres), F-31 (import d'une liste), F-30 (presse-papiers). Décisions de l'utilisateur du 2026-11-11 :
- **Page** : lue par le moteur (`linkscan.run`, 4 Mio au plus, sans en-têtes de tâche ni identifiants), HTML seul : pas de JavaScript (l'extension pourra plus tard envoyer les liens de la page ouverte), pas d'exploration des pages liées. Liens relevés : `href` de `a` et `area` ; `src` de `img`, `video`, `audio`, `source`, `track`, `embed`, `iframe`, `frame` ; `srcset` de `img` et `source` ; `data-src` de `img` ; `poster` de `video` ; `data` de `object` ; résolus par rapport à l'adresse finale de la page, ou à sa première balise `<base href>`. Puis les adresses écrites en clair dans le texte hors balises (scripts compris, `\/` lu `/`), commentaires exclus. Les adresses des autres attributs (feuilles de style, `<meta>`) ne comptent pas.
- **Texte et liste importée** (F-31) : toute adresse HTTP(S) ou lien magnet écrit en clair, quelle que soit la colonne ; une adresse s'arrête à une espace, un guillemet, un chevron, une virgule ou un point-virgule (séparateurs d'un CSV), sans sa ponctuation finale. Le nom vient de l'adresse.
- **Chaque lien** : adresse absolue sans ancre, sans doublon, dans l'ordre du document, 10 000 au plus ; HTTP(S) et magnet seulement (un lien magnet sans `xt` est ignoré) ; nom tiré du dernier segment, nettoyé (F-09), sinon l'hôte ; pour un lien magnet, `dn`. Sorte : `file` si une extension (1 à 10 lettres ou chiffres) hors pages web (`htm`, `html`, `php`, `asp`, `jsp`, `cgi`…), `page` sinon (pages et adresses sans extension, montrées seulement sur demande, décision), `magnet`.
- **Filtres** (extension, taille minimale et maximale, expression régulière sur l'adresse, « tous les liens ») : appliqués par chaque client, la boîte du bureau à chaque frappe sans relire la page ; le moteur n'en applique aucun. Un filtre de taille écarte un lien de taille inconnue.
- **Tailles** (décision : automatiques, limitées) : `linkscan.probe`, une requête `HEAD` et au besoin une requête d'un octet par lien, 4 à la fois, 500 au plus, liens magnet exclus : d'abord les liens montrés, puis les « pages » masquées. La même requête donne le type annoncé : un lien classé `page` que le serveur décrit comme un fichier (type autre que HTML, ou nom donné) devient un fichier et apparaît (décision du 2026-11-11, après l'essai de l'interface : `SHA256SUMS`, `README`, `/download?id=…` n'ont pas d'extension) ; jamais l'inverse, un fichier qui rend une page d'erreur reste un fichier.
- **Ajout** : `task.addBatch`, chaque lien ajouté ou refusé pour lui-même (décision).
- **Interfaces** (décision) : CLI `auro links <page|fichier|->` (filtres `-e`, `--regex`, `--min-size`, `--max-size`, `--all`, `--no-sizes` ; `--add -o <dossier> [--paused]`) ; bureau (étape 2b) ; presse-papiers du bureau (étape 2c, F-30, désactivé par défaut ; une adresse copiée fait apparaître une notification du système, un clic ouvre la boîte « Ajouter » préremplie, décision). L'extension et Android plus tard.
- Mise en œuvre : 2a. moteur (module `linkscan`, `linkscan.run`, `linkscan.probe`, `task.addBatch`) et CLI (faite le 2026-11-11) ; 2b. bureau (faite le 2026-11-11, section 4.6 ; essai de l'interface validé par l'utilisateur sur la variante d'essai : page Debian réelle, liste CSV importée, texte collé, filtres, ajout groupé ; il a fait ajouter la lecture du type des liens sans extension) ; 2c. presse-papiers (faite le 2026-11-11, ci-dessous ; essai de l'interface validé par l'utilisateur sur la variante d'essai : notification, clic, copies sans effet, lien magnet, option décochée, avertissement ; étape 2 terminée).
- **Presse-papiers** (F-30, étape 2c, décisions de l'utilisateur du 2026-11-11) : préférence de l'interface « Proposer d'ajouter les liens copiés » (Options, Général ; `watchClipboard`, désactivée par défaut ; le moteur n'en sait rien). Cochée, l'application lit le texte du presse-papiers une fois par seconde (`arboard`). Seule une nouvelle copie compte : ce qui était copié avant de cocher ne déclenche rien, ni une copie faite pendant que la fenêtre d'AuroDownloader est au premier plan. Déclenche une notification : une adresse HTTP(S) seule dont le dernier segment a une extension hors pages web (comme `linkscan`), ou un lien magnet avec `xt` ; une page, un texte, plusieurs adresses ne déclenchent rien. La notification (« Lien copié », nom du fichier) est celle du système : `tauri-winrt-notification` sous Windows (identifiant de l'application installée ; celui de PowerShell pour un exécutable lancé depuis `target`), `notify-rust` sous Linux (action par défaut), le greffon de notification de Tauri ne transmettant pas le clic sur ordinateur. Un clic ouvre la boîte « Ajouter » avec le lien, à confirmer ; rien n'est ajouté d'office. Vie privée (section 8.2) : le texte lu n'est ni gardé ni écrit ; seule une empreinte du dernier texte reste en mémoire, pour reconnaître une nouvelle copie. Notifications coupées dans Windows (interrupteur général `PushNotifications\ToastEnabled`, ou celles de l'application, `Notifications\Settings\<identifiant>\Enabled`, lus sans écrire) : un avertissement apparaît sous la case cochée, relu à chaque ouverture des Options (méthode de l'hôte `notificationsBlocked`) ; sans lui, l'option ne ferait rien sans le dire (trouvé pendant l'essai). Rien de tel sous Linux.

### 7.19 Interface web distante (phase 6, étape 3, conception du 2026-11-11)

F-39, transports de la section 6.2, menaces de la section 8.1. Les décisions de l'utilisateur du 2026-11-11 sont marquées « décision » ; le reste est un choix par défaut, à revoir à la porte de la phase (revue de sécurité).
- **Serveur** (décision) : `aurod` lui-même, module `remote` du moteur, désactivé par défaut (réglage `remote.enabled`). Il sert la page de l'interface (`ui`, intégrée au binaire), le WebSocket `/ws` (un message JSON-RPC par trame texte, mêmes règles que le canal local) et `POST /rpc` (décision : dès la v1 ; une requête, une réponse, sans notifications).
- **Écoute** (décision) : adresse et port choisis par l'utilisateur (`remote.listen`, défaut `127.0.0.1:7380`). Sur la boucle locale (`127.0.0.1`, `::1`), sans TLS ; toute autre adresse (`0.0.0.0` et `::` compris : toutes celles de la machine) en TLS, jamais sans.
- **Certificat** (décision) : fabriqué par le moteur à la première écoute en TLS (auto-signé, noms `localhost`, nom de la machine et adresse d'écoute), son empreinte SHA-256 montrée par la CLI et le bureau pour la comparer dans le navigateur ; ou fourni par l'utilisateur (certificat et clé PEM, réglages `remote.tlsCert` et `remote.tlsKey`). Décisions de l'utilisateur du 2026-11-11 (étape 3b) : la clé fabriquée est gardée dans un fichier à côté de la base (`remote-key.pem`, lisible par l'utilisateur seul : `0600`, ou sous Windows un descripteur qui n'autorise que lui, sans droit hérité), pas dans le coffre du système, absent d'un serveur sans session graphique ; le certificat fabriqué dure 824 jours (les iPhone refusent plus de 825) et se refait seul 30 jours avant son expiration ou quand un nom voulu manque, l'empreinte changeant alors.
- **Jetons** : `token.create` (nom, portée) rend le jeton une seule fois (32 octets aléatoires) ; la base n'en garde que l'empreinte SHA-256, le nom, la portée et les dates de création et de dernier usage ; `token.list`, `token.revoke` (ferme ses connexions et ses sessions). Portée `read`, `add` ou `control` (décision : jamais `admin`, qui reste au canal local). Les méthodes `token.*` et les réglages `remote.*` ne s'atteignent que par le canal local (décision : aucun jeton ne crée de jeton).
- **Connexion du navigateur** (décision) : le jeton, saisi une fois dans la page, est envoyé à `POST /login`, qui pose un cookie de session `HttpOnly`, `SameSite=Strict`, `Secure` en TLS ; le navigateur ne garde jamais le jeton. Session aléatoire, empreinte seule en base, valable 30 jours, effacée par `POST /logout` ou avec son jeton. Le WebSocket accepte le cookie ou `Authorization: Bearer` ; `POST /rpc` seulement `Authorization` (décision : jamais le cookie, contre le CSRF). Le QR code du bureau porte l'adresse et le jeton dans l'ancre (`#token=…`, jamais envoyée au serveur) ; la page l'échange aussitôt contre une session et l'efface de la barre d'adresse.
- **`Host` et `Origin`** (section 6.2) : `Host` doit nommer ce serveur, avec son port : toute adresse IP écrite en chiffres (décision de l'utilisateur du 2026-11-11, étape 3b : le DNS rebinding passe par un nom, jamais par une adresse), `localhost`, le nom de la machine (et `<nom>.local`) ou un nom ajouté par l'utilisateur (`remote.hosts`) : contre le DNS rebinding. Une requête avec `Origin` doit venir de la même origine ; sans `Origin` (scripts), seulement avec `Authorization`. Aucun CORS. Jetons comparés par empreinte ; au-delà de 10 échecs par minute d'une même adresse, refus (`429`) pendant une minute.
- **SSRF** (décision : pour toute connexion par jeton, même sur la boucle locale) : les adresses de boucle locale, privées, de lien local, non spécifiées et de multidiffusion sont refusées, vérifiées à chaque connexion après la résolution DNS, redirections comprises ; la tâche garde cette restriction pour toute sa vie, reprises comprises. Valent aussi pour les miroirs, `linkscan.*`, `metalink.*`, `media.probe` et l'actualisation de l'adresse. Limites connues : yt-dlp fait ses propres requêtes (seule l'adresse de la page est vérifiée) ; les traqueurs et les pairs des torrents ne sont pas filtrés. Réglage `remote.allowPrivate` (faux) pour les autoriser.
- **Dossiers** (décision) : une tâche ajoutée ou déplacée par jeton n'écrit que sous le dossier des téléchargements du moteur (`downloads.dir`) : chemin résolu, sans `..` ni lien qui en sorte.
- **Fichiers terminés** (décision) : non servis en v1 ; la page distante pilote les téléchargements, les fichiers restent sur la machine du moteur.
- **Page distante** : la page du bureau avec un hôte WebSocket, sans choix de dossier ni ouverture de fichier, zone de notification, démarrage avec la session, presse-papiers ni association ; sans les Options du moteur (`admin`). En-têtes : `Content-Security-Policy` stricte (`default-src 'none'`, scripts, styles et connexions de ce serveur seulement), `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, `X-Content-Type-Options: nosniff`. Décisions de l'utilisateur du 2026-11-11 (étape 3c) : mise en page adaptée aux petits écrans (cartes avec leur bouton Arrêter/Reprendre, boîtes en plein écran) ; sous-dossier facultatif sous le dossier des téléchargements (`subDir`) à la place du choix de dossier ; .torrent et liens magnet, vidéo, analyse de liens et Metalink, historique gardés (fichiers choisis sur l'appareil par le navigateur, export CSV téléchargé) ; préférences (langue) gardées par le navigateur.
- **Gestion** (décision : CLI et bureau) : `auro remote` (activer, couper, état, empreinte) et `auro token` (créer, lister, révoquer) ; rubrique « Accès à distance » des Options du bureau, avec le QR code pour le téléphone.
- Mise en œuvre : 3a. moteur sur la boucle locale (jetons, sessions, WebSocket et `POST /rpc`, `Host` et `Origin`, SSRF, dossiers) et CLI ; 3b. TLS (certificat fabriqué ou fourni), écoute sur le réseau ; 3c. page servie par le moteur et son hôte WebSocket ; 3d. bureau (rubrique, QR code) ; 3e. essais réels (téléphone sur le Wi-Fi), banc `crash_daemon`, fuzzing des lecteurs de requêtes.
- **3a, protections et jetons** (2026-11-11) : `net::reach` (adresses publiques, résolveur filtrant, adresses écrites telles quelles vérifiées à chaque requête et redirection ; le client restreint n'utilise aucun mandataire, qui résoudrait les noms à sa place), `net::Client::restricted` ; colonne `tasks.public_only` (migration 8), prise à chaque exécution, reprises comprises, et pour les vidéos (flux internes ; adresse de la page vérifiée avant yt-dlp) ; un miroir ou une adresse donnés par un jeton restreignent une tâche locale pour toujours ; `Engine::restricted` (poignée d'une connexion à jeton) ; `store::remote` et `remote` (jetons et sessions, empreintes SHA-256) ; méthodes `token.*` ; `rpc::serve_messages` (tout transport, révocation suivie) ; codes `ADDRESS_FORBIDDEN` et `DEST_FORBIDDEN` ; CLI `auro token`. Tests : classement des adresses, client restreint face à un serveur local (aucune requête reçue), jetons et sessions en base, connexion à jeton de bout en bout (refus, dossier, tâche locale restreinte qui échoue en `ADDRESS_FORBIDDEN`, révocation qui ferme la connexion), `remote.allowPrivate`, CLI.
- **3a, écoute** (2026-11-11) : `remote::server` (hyper, HTTP/1.1 ; en-têtes reçus en 10 s ; corps reçu en 10 s pour `/login` (4 Kio au plus) et en 2 min pour `/rpc`, `408` sinon ; 64 connexions à la fois ; `GET /ws` : poignée de main faite ici, `tokio-tungstenite` sur la connexion de hyper, trames texte de 24 Mio au plus, binaire refusé, fermé à l'arrêt de l'écoute ou à la révocation ; `POST /rpc` : `Content-Type: application/json` exigé, notifications écartées, `204` sans réponse ; `POST /login` : `Origin` de même origine et JSON exigés, cookie `auro_session` de 30 jours, distinct du jeton ; `POST /logout`) ; `Host` accepté : l'adresse d'écoute avec son port, et pour la boucle locale `localhost`, `127.0.0.1` et `[::1]` (`421` sinon) ; `Origin` de même origine ou absent (`403` sinon) ; échecs comptés par adresse (`429`) ; en-têtes `nosniff`, `DENY`, `no-referrer`, `no-store`, CSP `default-src 'none'`. `remote::run` (lancé par `aurod`) tient l'écoute selon `remote.enabled` et `remote.listen`, la déplace ou la ferme (connexions comprises) à chaque changement, et refuse une adresse hors de la boucle locale tant que TLS manque (`remote.status`, `error`). CLI `auro remote` (`status`, `enable --listen`, `disable`, `allow-private on|off`). Tests : en-têtes falsifiés (`Host`, port, `Origin`, `null`), jeton absent, faux ou en cookie pour `/rpc`, type de contenu, portée, blocage après 10 échecs, session du navigateur (cookie `HttpOnly` et `SameSite=Strict`, refus sans `Origin` ou d'une autre origine, `/logout`), message binaire, révocation qui ferme les WebSocket, superviseur (ouverture, déplacement, refus hors de la boucle locale, fermeture), CLI.
- **3b, TLS** (2026-11-11) : `remote::tls` : certificat fabriqué (rcgen, ECDSA P-256, usage « serveur », pas une autorité ; noms : `localhost`, nom DNS de la machine et `<nom>.local`, noms et adresses de `remote.hosts`, `127.0.0.1`, `::1`, l'adresse d'écoute, ou pour `0.0.0.0` et `::` celles des interfaces hors adresses locales au lien) ; fichiers `remote-cert.pem` et `remote-key.pem` à côté de la base, écrits sous un nom provisoire, vidés sur disque puis renommés, la clé d'abord (une paire qui ne va pas ensemble après un arrêt est refaite) ; repris tels quels tant qu'ils vont ensemble, portent tous les noms voulus et ont plus de 30 jours devant eux, refaits sinon (fichier abîmé compris). Certificat fourni : chaîne et clé PEM lues à chaque ouverture ; refusé (écoute fermée, `error`) s'il manque, s'il a expiré ou si la clé ne va pas avec lui ; jamais refait par le moteur ; le message d'une clé illisible ne cite jamais son contenu. Serveur : rustls (aws-lc, TLS 1.2 et 1.3, ALPN `http/1.1`) par `tokio-rustls`, négociation comprise dans le délai de 10 s des en-têtes ; cookie de session `Secure` ; `Origin` en `https`. `remote::run` : le port d'abord, puis le certificat (rien n'est fabriqué pour une adresse qui ne s'ouvre pas) ; l'écoute ne repart que si `remote.enabled`, `remote.listen`, `remote.hosts`, `remote.tlsCert` ou `remote.tlsKey` ont vraiment changé, ou à l'échéance du certificat fabriqué (contrôlée au moins chaque jour), connexions fermées alors. `remote.status` : `tls`, `certificate`, `urls`. CLI : `auro remote` (adresses, empreinte, fin de validité, noms), `auro remote hosts <nom>… | --clear`, `auro remote cert --file <cert.pem> --key <clé.pem> | --generated` (chemins rendus absolus). Tests : fabrication, reprise, refait quand un nom manque, à 30 jours, avec une clé d'une autre paire ou un fichier abîmé ; droits de la clé (`0600` ; sous Windows une seule entrée, l'utilisateur, sans héritage) ; certificat fourni (empreinte, expiré, clé d'une autre paire, absent) ; valeurs de `Host` (adresses, ports, noms, formes piégées) ; de bout en bout en TLS sur la boucle locale (le superviseur peut l'imposer aux tests) : `POST /rpc` par un client qui vérifie l'empreinte, refus d'un client qui en attend une autre, client en clair sans réponse, `Origin` en `http` refusée, cookie `Secure`, WebSocket chiffré, nom ajouté (certificat refait, `Host` accepté) puis retiré (certificat repris), certificat fourni puis absent puis fabriqué ; renouvellement à l'échéance pendant que l'écoute tourne ; CLI.
- **3c, page** (2026-11-12) : `remote::page` : les fichiers de `ui/src` intégrés au binaire (`include_bytes!`), servis sans authentification par `GET /` et leur nom, rien d'autre (`404`) ; politique de sécurité de la page : `default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self' ws(s)://<Host>; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`. `GET /session` : `{name, scope}` de la session du cookie, ou `401` ; sans `Origin` (un navigateur n'en envoie pas pour un `GET` de même origine ; le cookie `SameSite=Strict` ne part pas d'un autre site, et la réponse ne se lit pas d'ailleurs). `ui/src/web.js` (`webHost`) : WebSocket `/ws`, `engine.hello` puis `events.subscribe` (avancement chaque seconde), reconnexion espacée jusqu'à 15 s, page rechargée si la session a expiré, avancement omis page cachée puis état relu ; fichiers choisis sur l'appareil (`.torrent`, Metalink, listes) lus par le navigateur ; export CSV téléchargé ; préférences dans le stockage local du navigateur. Connexion : le jeton de l'ancre (`#token=`, QR code du bureau, étape 3d) est échangé aussitôt contre une session et effacé de l'adresse et de l'historique ; sinon une boîte demande le jeton (le texte collé peut être la ligne ou l'adresse qui le contient ; un texte sans jeton complet n'est pas envoyé, et le dit) ; « Se déconnecter » ferme la session. Ce que montre la page suit la portée de la session (`data-needs` : `read`, `add`, `control`, `admin`) et les services de l'hôte (`app`, `files`, `folders`, `remote`) ; un jeton `add` ne voit que l'ajout. Moteur : `subDir` (section 6.5), désormais pour tout téléchargement, résolu au début de l'ajout (le dossier peut rester vide si l'ajout est ensuite refusé) ; `settings.get` cache les réglages `remote.*` à une connexion à jeton ; `/login` ignore les espaces autour du jeton. Tests : page et politique de sécurité, fichiers absents, `Host` falsifié, `/session` (absente, ouverte, fermée) ; `subDir` (sous-dossiers, `..`, profondeur, nom invalide, destination hors du dossier, lien vers ailleurs sous Linux, lot, lien magnet) ; réglages cachés au jeton ; page : jeton de l'ancre et collé, sous-dossier, portées (`node --test`). Essai de la page validé par l'utilisateur le 2026-11-12 sur un moteur d'essai (jetons `control`, `read` et `add`, petit écran) ; reste à l'essayer dans l'application de bureau, qui partage la page, et depuis un vrai téléphone (étape 3e). Restent à l'étape 3e : essai réel depuis un téléphone sur le Wi-Fi (sous Windows, une écoute hors de la boucle locale fait apparaître la fenêtre du pare-feu), délai de négociation essayé face à un client muet.
- **3d, bureau** (2026-11-12, décisions de l'utilisateur du même jour) : rubrique « Accès à distance » des Options (portée `admin` et hôte qui sait faire un QR code : jamais dans la page distante). Case d'activation (`remote.enabled`) ; adresse d'écoute choisie dans une liste : « cet ordinateur seulement » (`127.0.0.1`) et les adresses des interfaces (`addresses` de `remote.status` : nom de l'interface et adresse, IPv4 puis IPv6, hors boucle locale et adresses locales au lien), jamais toutes les interfaces à la fois (une écoute sur `0.0.0.0` réglée par la CLI reste montrée telle quelle), plus le port ; après chaque changement, l'état est relu jusqu'à ce que l'écoute suive les réglages (5 s au plus). Certificat en place : fabriqué ou fourni, empreinte SHA-256 à comparer dans le navigateur de l'appareil, fin de validité ; « Utiliser mon certificat… » (chemins d'un certificat et d'une clé PEM, `remote.tlsCert` et `remote.tlsKey`, relus par le moteur) et « Revenir au certificat d'AuroDownloader ». Appareils : liste des jetons (nom, droits, dernière utilisation), « Ajouter un appareil… » (nom, droits : tout piloter, ajouter seulement ou regarder seulement), « Retirer » (révocation, sessions fermées). À la création, et seulement alors, la boîte montre le QR code de l'adresse de la page avec le jeton dans l'ancre (`https://<adresse>:<port>/#token=auro_…` ; adresses du réseau d'abord, au choix s'il y en a plusieurs ; avertissement si l'écoute est réservée à cet ordinateur ; accès pas encore ouvert : l'adresse qu'aura la page d'après les réglages, même règle que le moteur, sans quoi un appareil ajouté avant l'ouverture n'aurait jamais de QR code, trouvé pendant l'essai) et le jeton à copier ; fermer la boîte les efface de la page. QR code : matrice fabriquée par l'application (`qrcode`, correction moyenne, texte de 512 octets au plus ; commande `qr_code`), dessinée par la page en SVG noir sur blanc avec sa marge de 4 modules, quel que soit le thème. Tests : `addresses` de `remote.status` ; matrice (forme, refus) ; adresse d'écoute lue et écrite (IPv6 entre crochets), lien du QR code relu par la page, adresses du réseau d'abord, tracé, attente de l'écoute, adresse avant l'ouverture (`node --test`). Essai de l'interface validé par l'utilisateur le 2026-11-12 sur la variante d'essai (boucle locale : activation, port, ajout avant et après l'ouverture, QR code lu par un téléphone, retrait) ; trouvé pendant l'essai : le trait des icônes (règle `svg` de la page) épaississait les modules du QR code, rendu illisible.
- **3e, fuzzing et clients lents** (2026-11-12) : `remote::fuzz`. Tests de propriétés des lecteurs de requêtes (2 000 cas chacun) : `Host` n'est accepté que s'il nomme ce serveur, d'après une règle écrite autrement (adresse IPv4 en chiffres, IPv6 entre crochets ou nom connu, au bon port ; une IPv6 sans crochets jamais) ; `Origin` seulement unique et de même origine ; `Authorization` lu seulement après `Bearer` ; le cookie de session seulement sous son nom exact ; `Content-Type` JSON strict ; corps de `/login` (n'importe quels octets, jeton seulement s'il est une chaîne). Le serveur réel face à des requêtes abîmées (page, session, WebSocket, `/rpc`, `/login` avec un faux jeton ; octets remplacés, insérés, répétés, coupure, `Transfer-Encoding`, `Content-Length` démesuré, second `Host` ; 200 cas, `AURO_FUZZ_CASES` pour plus, répartis sur des adresses de 127.0.0.0/8, le blocage valant par adresse) : jamais d'erreur 5xx, de session ouverte, de WebSocket accepté ni de `/rpc` servi, et il sert encore la requête suivante ; la plupart des cas doivent recevoir une réponse (sans quoi le test ne prouverait rien). Clients lents : délai des en-têtes, de la négociation TLS et du corps de `/login` porté par l'écoute (10 s ; 1 s dans les tests, `serve_timed`) ; un client muet est fermé au bout du délai, en clair comme en TLS, des en-têtes envoyés goutte à goutte aussi ; quand 64 muets prennent toutes les places, la connexion suivante est fermée aussitôt, puis servie une fois le délai passé. Relance de l'écoute sur la même adresse (noms, certificat changés ou refaits) : l'écouteur est gardé par le superviseur et partagé avec la tâche qui sert, jamais rendu puis repris ; sous Linux, un test de la CLI lancé avec d'autres avait vu le port pris entre les deux (« Address already in use », trouvé pendant la campagne). Une copie du socket (`try_clone`) ne recevait pas les connexions sous Windows : écarté. Essai réel validé par l'utilisateur le 2026-11-12 : variante d'essai de l'application sous Windows, écoute sur l'adresse Ethernet du PC (port 7391, pare-feu de Windows autorisé pour l'essai puis règles retirées), téléphone Android sur le Wi-Fi de la même box : QR code lu, empreinte du certificat fabriqué comparée dans l'avertissement du navigateur, page entrée d'elle-même, fichier Debian ajouté, arrêté et repris depuis le téléphone, vu sur le PC.

### 7.20 Plugins d'hébergeurs (phase 6, étape 4, conception du 2026-11-12)

F-41. Un plugin apprend au moteur à trouver le lien direct d'un fichier sur la page d'un hébergeur ; le moteur télécharge ensuite lui-même, avec sa reprise et ses contrôles. Décisions de l'utilisateur du 2026-11-12 :

- **Moteur** : WebAssembly interprété par wasmi (prototype de l'étape 4a, section 4.3). Interface en WebAssembly de base (ni WASI ni modèle de composants), messages JSON.
- **Périmètre v1** : hébergeurs seulement (pas d'actions de fin de téléchargement). Windows et Linux ; Android plus tard.
- **Bac à sable** : aucun accès aux fichiers ni au système ; le réseau seulement par le moteur, vers les domaines que le plugin déclare (redirections comprises), réseau public seulement ; limites de calcul, de mémoire, de pile et de nombre de requêtes. Aucun secret de l'utilisateur en v1 : ni cookies, ni mots de passe, ni jetons.
- **Requêtes** : `GET`, et `POST` d'un petit formulaire (64 Kio au plus) ; en-têtes choisis par le plugin parmi `Referer`, `Accept`, `Accept-Language`, `Content-Type`, `X-Requested-With` ; 32 requêtes par résolution, réponses de 8 Mio au plus.
- **Cookies** : ceux que le site pose pendant la résolution (session de l'hébergeur) sont gardés en mémoire par le moteur, renvoyés aux requêtes suivantes du plugin, puis donnés au téléchargement du lien rendu (par le coffre du système, comme ceux de l'extension, section 5.1). Le plugin ne lit jamais leurs valeurs.
- **Format** : un seul fichier `.wasm` ; le manifeste est dans sa section personnalisée `auro-plugin` (JSON) : `id`, `name`, `version`, `author`, `description`, `sites` (domaines des pages prises en charge), `fetch` (domaines que le plugin peut demander), `abi` (1). L'empreinte SHA-256 du fichier couvre tout.
- **Installation** : depuis un fichier choisi par l'utilisateur, jamais d'un catalogue ni d'une mise à jour seule ; manifeste, permissions et empreinte montrés et acceptés ; de nouveau quand une nouvelle version demande plus de sites ou de domaines. Canal local seulement (portée `admin`) : rubrique « Plugins » des Options du bureau, `auro plugin`. La page distante s'en sert à l'ajout, jamais pour en installer.
- **Déclenchement** : « Ajouter » (bureau, CLI, page distante) reconnaît l'adresse d'une page d'un site qu'un plugin activé prend en charge et montre ses liens à choisir, comme pour Metalink ; `task.add` d'une telle page garde le lien s'il est seul. La tâche retient la page : si son lien direct a expiré à la reprise (`403`, `410`), le moteur redemande au plugin un lien neuf, une fois, puis reprend comme « Actualiser l'adresse » (section 7.13).

Conception :

- **Interface d'un plugin (ABI 1)** : exports `memory`, `auro_alloc(taille) -> adresse`, `auro_resolve(adresse, taille) -> (adresse << 32) | taille`. Entrée `{"url": …}`, sortie `{"links": [{"url", "name"?, "size"?, "referer"?}]}` ou `{"error": "…"}`. Imports du module `auro`, et rien d'autre : `fetch(adresse, taille) -> (adresse << 32) | taille` (requête `{"url", "method"?, "headers"?, "form"?}`, réponse `{"status", "url", "headers": {"content-type", "location"}, "body"}` ou `{"error"}`, écrite par l'hôte dans une zone obtenue par `auro_alloc`), `log(adresse, taille)`. Une instance neuve par résolution.
- **Validation à l'installation** : fichier de 8 Mio au plus, WebAssembly valide pour wasmi, imports et exports exactement ceux de l'ABI ; manifeste : `id` de 3 à 64 caractères `[a-z0-9.-]`, 1 à 16 `sites` et 1 à 16 `fetch`, noms de domaine (pas d'adresse IP, pas de joker ; un domaine vaut aussi pour ses sous-domaines), textes de 200 caractères au plus. Fichier gardé sous `plugins/<empreinte>.wasm` à côté de la base, enregistré dans la table `plugins` (migration 10).
- **Exécution** : sur un fil à part (2 résolutions à la fois au plus), 5·10⁹ unités de calcul, 64 Mio de mémoire, 10 000 appels imbriqués, 60 s en tout. Un plugin qui dépasse une limite, s'arrête ou rend une sortie illisible : `PLUGIN_FAILED`, sans effet sur le reste du moteur.
- **Liens rendus** : adresses HTTP(S) seulement, 1 000 au plus ; noms passés par `net::filename::sanitize` ; tâches au réseau public seulement (`publicOnly`), comme celles d'une connexion à jeton ; `Referer` seulement parmi les en-têtes, plus les cookies de la résolution pour l'hôte du lien.
- **4b, moteur et CLI** (2026-11-12) : module `plugins` (`manifest` : lecture de la section du fichier sans le moteur WebAssembly, puis validation ; `runtime` : wasmi, instance neuve, limites, imports `auro.fetch` et `auro.log` seulement ; `fetch` : client propre aux plugins, sans mandataire ni redirection automatique, résolution filtrée au réseau public, redirections suivies par le module vers les domaines déclarés seulement, `303` et `POST` redirigé changés en `GET`, boîte à cookies par résolution, un cookie `Domain` qui ne couvre pas l'hôte refusé, 64 cookies au plus), table `plugins` et colonnes `tasks.plugin_page`, `tasks.plugin_id` (migration 10), fichiers sous `plugins/<empreinte>.wasm` (écrits sous un nom provisoire, vidés, renommés ; empreinte revérifiée à chaque chargement, un fichier modifié est refusé). Ajout : la tâche d'un lien rendu garde le `Referer` (celui du plugin, sinon la page) en clair et les cookies du site par le coffre ; `task.add` d'une page lance son plugin (sauf `plugin: false`, ou des en-têtes, des miroirs, un torrent, une vidéo) ; résolutions gardées 10 minutes pour `plugin.add`. Lien expiré : un superviseur, poignée faible du moteur, écoute les passages en `failed` ; `HTTP_STATUS` avec `403` ou `410` (statut relu dans le détail de l'échec, `net::status_in_detail`) d'une tâche venue d'un plugin : le plugin relit la page, le lien de même nom (ou le seul) remplace l'ancien (en-têtes remplacés en une transaction, nouveaux secrets écrits avant, anciens effacés après ; adresse vérifiée sur le réseau public par « Actualiser l'adresse »), puis la tâche reprend ; une fois, de nouveau 10 minutes plus tard au plus tôt. CLI : `auro plugin install <fichier.wasm>` (manifeste, permissions, empreinte, domaines nouveaux d'une nouvelle version ; installé seulement avec `--yes`), `list`, `remove`, `enable`, `disable`, `resolve <page>`, `add <page> --select`, `auro add --no-plugin`. Tests : plugins d'essai écrits en texte WebAssembly (`testing::plugin_files`) : manifeste et fichiers refusés (import non permis, export manquant ou mal typé, sans manifeste) ; liens vérifiés, noms nettoyés, sorties refusées ; fichier modifié ou absent ; requêtes (domaine non déclaré, redirection hors des domaines, en-tête non permis, méthode, formulaire, réponse trop grande, `file://`), cookies du site rendus au lien de leur domaine seulement ; limites de calcul, de mémoire et de pile ; protocole (installation sur l'empreinte acceptée, portées, jeton `add` qui s'en sert sans voir les cookies, `Referer` en clair et cookie au coffre en base, désactivation, retrait du fichier) ; ajout direct d'une page, `PLUGIN_CHOICE`, `plugin: false`, lien expiré (403) renouvelé puis téléchargement complet vérifié octet par octet ; CLI ; fuzzing du lecteur de manifeste, de la sortie d'un plugin et des `Set-Cookie`.
- **4c, bureau et page** (2026-11-12, décisions de l'utilisateur) : rubrique « Plugins » des Options du bureau (liste, permissions et empreinte de chacun, « Installer un plugin… » : fichier `.wasm` choisi, lu par l'application, montré par `plugin.inspect` avec ses domaines et son empreinte, installé seulement sur « Installer » par `plugin.install` avec cette empreinte ; activer, désactiver, retirer ; absente de la page distante). Dans « Ajouter », bureau et page distante : les adresses qu'un plugin activé prend en charge sont signalées sous le champ (`plugin.match`, sans requête aux sites), avec une case « sans plugin » qui les ajoute telles quelles (`plugin: false`) ; à la validation, chaque page est lue par son plugin (`plugin.resolve`) : un seul lien est ajouté avec le reste, plusieurs (ou aucun) font revenir la boîte avec la liste à cocher et les messages du plugin repliés, comme un Metalink ; `plugin.add` ajoute les liens cochés. Ni empreinte ni miroir pour une page d'hébergeur. Les Propriétés d'une tâche venue d'un plugin montrent sa page et le plugin (`plugin` de `task.get`).
- **4d, plugin d'exemple** (2026-11-12, décisions de l'utilisateur) : `plugins/archive-org`, en Rust (`wasm32-unknown-unknown`, espace de travail Cargo à part, guide de l'auteur dans `plugins/README.md`) : une page `archive.org/details/<élément>` donne les fichiers d'origine de l'élément (métadonnées, vignette, `.torrent` et fichiers réservés écartés), lus par l'interface publique `archive.org/metadata/<élément>`, avec leur taille ; `/download/<élément>/<fichier>` donne ce seul fichier ; un élément retiré (`is_dark`) ou absent est une erreur claire. Le manifeste est rangé dans sa section par la compilation (`#[link_section]`), sans outil après. Module `auro.rs` recopiable (interface version 1) ; une bibliothèque séparée pour les auteurs viendra plus tard. Essai réel sous Windows par la CLI sur un moteur d'essai : installation montrée puis acceptée, résolution d'un dessin animé du domaine public, élément retiré et élément absent refusés. Essai de l'interface (rubrique « Plugins », ajout d'un élément de 22 fichiers et d'un élément d'un seul, Propriétés, « sans plugin ») validé par l'utilisateur sur la variante d'essai le même jour : étape 4 terminée.
- **5, revue de sécurité** (2026-11-12, porte de la phase 6, franchie le même jour, déclarée par l'utilisateur) : rapport dans `docs/SECURITE.md` (modèle de menace, revue point par point de l'interface distante, des plugins et du canal local, risques résiduels). Corrigé, chacun avec son test : tables WebAssembly limitées à 100 000 entrées (une table démesurée, déclarée ou agrandie, faisait allouer des Gio au moteur) ; module à fonction de démarrage refusé (elle s'exécutait à la vérification, avant l'accord) ; canal local vérifié par le client (propriétaire du canal nommé, dossier du socket) ; cookies d'un lien neuf retirés de la tâche si son adresse est refusée ; garde de la table des portées (chaque méthode servie a la sienne) ; 6to4, NAT64 d'usage local et IPv4 traduite jugés sur leur adresse IPv4.

### 7.21 Règles de rangement et commande de fin (phase 7, étape 4, conception du 2026-11-12)

F-32 et F-34. Une règle range le fichier d'une tâche (dossier, nom) et peut lancer une commande quand la tâche est terminée. Décisions de l'utilisateur du 2026-11-12 :

- **Moment** : à la préparation, dès que le nom du fichier est connu et avant la création du `.part` : le fichier naît dans son dossier et sous son nom, rien n'est déplacé à la fin.
- **Dossier choisi** : les règles ne rangent que les tâches ajoutées sans dossier (`destDir` absent : extension, page laissée telle quelle, `auro add --auto`) ; un dossier choisi l'emporte. La CLI garde le dossier courant quand `-o` manque (décision du 2026-11-07) : sans `--auto`, pas de rangement.
- **Critères** : type de fichier (catégories de la page : `compressed`, `documents`, `images`, `music`, `programs`, `video`, `other` ; ou extensions libres), domaine (sous-domaines compris), expression régulière sur l'adresse ou sur le nom. Chacun facultatif, tous doivent correspondre ; une règle sans critère vaut pour toutes les tâches. La première règle activée qui correspond, dans l'ordre, l'emporte, seule.
- **Actions** : dossier (absolu, ou relatif : sous le dossier des téléchargements), modèle de nom à variables, pour HTTP, FTP et vidéos ; les torrents ne reçoivent que le dossier.
- **Commande de fin** (F-34) : une action de la règle ; Windows et Linux seulement. Un programme et sa liste d'arguments, sans shell ; les variables sont remplacées dans chaque argument et données aussi en variables d'environnement `AURO_*`. La tâche attend la commande en `post_processing` (délai propre à la règle, 10 minutes par défaut) ; un code de sortie non nul ou un délai dépassé la met en `failed`, code `POSTPROCESS_FAILED`, fichier gardé, et la relancer relance la commande seule ; un arrêt du moteur pendant la commande la relance au démarrage.
- **Droits** : les règles ne s'écrivent que par le canal local (bureau, CLI : portée `admin`, qu'aucun jeton n'a) ; une connexion à jeton les lit sans leurs commandes. Un dossier de règle libre ou une commande écrits à distance vaudraient une exécution de programme sur la machine.

Conception :

- **Règle** (`rules.list`, `rules.set`) : `ruleId`, `name` (100 caractères au plus), `enabled`, `match` : `categories[]`, `extensions[]` (sans le point, 16 caractères au plus, lettres et chiffres), `domains[]` (noms de domaine, sans adresse IP ni joker), `urlRegex`, `nameRegex` ; `actions` : `folder`, `rename`, `command` (`program`, `args[]`, `timeoutS`). Type : la catégorie du nom est parmi `categories` ou son extension parmi `extensions` (l'un ou l'autre). Expressions : syntaxe de la crate `regex` (temps linéaire, sans retour arrière ; `(?i)` pour ignorer la casse), 1 000 caractères au plus, automate borné. 100 règles au plus, 50 valeurs au plus par liste.
- **Ce qui se compare** : HTTP et FTP : l'adresse de la source principale et son hôte, le nom de la préparation (celui choisi par l'utilisateur, sinon celui du serveur) ; vidéo : l'adresse de la page, le nom tiré du titre ; torrent : le lien magnet (sans hôte : une règle à domaine ne le prend pas), le nom du torrent. Les tâches internes d'une vidéo n'ont pas de règle. Une adresse comparée n'est jamais écrite dans un journal.
- **Dossier** : absolu, ou relatif, parties séparées par `/` (8 au plus, sans `.` ni `..`, chacune un nom valide tel quel) sous le dossier de la tâche à son ajout (le dossier des téléchargements d'alors). Créé s'il manque. Une connexion à jeton n'écrit jamais de règle : le dossier d'une règle vaut aussi pour les tâches qu'elle ajoute.
- **Nom** (`rename`) : modèle de 255 caractères au plus, sans séparateur de dossier ni caractère de contrôle ; variables `{nom}` ou `{name}` (nom sans extension), `{ext}` (extension, sans le point ; `.{ext}` disparaît quand il n'y en a pas), `{hôte}` ou `{host}`, `{date}` (jour de l'ajout, heure locale, `AAAA-MM-JJ`) ; toute autre accolade est refusée à l'écriture. Le résultat passe par `net::filename::sanitize` (il contient le nom du serveur) ; vide, le nom d'origine reste. Un nom choisi par l'utilisateur l'emporte sur le modèle.
- **Tâche ajoutée avec un dossier** : la règle est quand même retenue (sa commande s'exécute), mais ni son dossier ni son modèle de nom ne s'appliquent.
- **Base** : table `rules` (section 5.2 ; colonne `name`, migration 11), `tasks.rule_base` (ajoutée sans dossier : le dossier des téléchargements d'alors, base des dossiers relatifs ; un retéléchargement est rangé de nouveau de la même façon ; effacé quand « Déplacer/Renommer » change le dossier), `tasks.rule_id` (règle retenue à la préparation ; une règle retirée ou désactivée depuis ne lance plus sa commande). `rules.set` remplace toute la liste, dans l'ordre ; une règle sans `ruleId` en reçoit un.
- **Commande** : `program` en chemin absolu, 64 arguments au plus de 4 Kio chacun, `timeoutS` de 1 à 86 400 ; variables `{chemin}`/`{path}` (fichier final, ou dossier d'un torrent à plusieurs fichiers), `{dossier}`/`{dir}`, `{nom}`/`{name}` (nom du fichier, extension comprise), `{taille}`/`{size}` (octets), `{sha256}` (calculé seulement si un argument le demande, ou repris d'une empreinte SHA-256 vérifiée), `{id}` ; mêmes valeurs dans `AURO_PATH`, `AURO_DIR`, `AURO_NAME`, `AURO_SIZE`, `AURO_SHA256` (si connue), `AURO_TASK_ID`. Lancée sans fenêtre, entrée vide, sorties jetées (rien n'en va aux journaux), dans le dossier du fichier ; arbre de processus tué au délai (`media::process`). Torrent : la commande s'exécute quand les fichiers sont complets et à leur nom, avant le partage.
- **4a, règles : moteur, protocole et CLI** (2026-11-16) : module `rules` (critères, modèle de nom, dossier, validation de tout ce qui vient du protocole ; catégories et extensions recopiées de la page, comparées par un test), `store::rules` (table, `place_task`, rangement enregistré dans la transaction qui enregistre le nom : `save_preparation_placed` pour HTTP, `save_resolution` pour une vidéo, `torrents::save_preparation`), migration 11. Le dossier d'une règle est créé avant de prendre le nom ; un retéléchargement est rangé de nouveau, sous la même base (`rule_base`), sans dossier emboîté. Méthodes `rules.list` et `rules.set` (schémas, corpus de la CLI). CLI : `auro rules` (liste), `rules add` (`--type` : catégories ou extensions, `--domain`, `--url-regex`, `--name-regex`, `--folder`, `--rename`, `--exec` et `--arg`, `--timeout`, `--at`, `--disabled`), `remove`, `enable`, `disable`, `move` (règle désignée par son rang, son identifiant ou son nom) ; `--auto` pour `add`, `get` et `video`. Tests : critères, modèles, dossiers refusés, règles refusées (dont une expression démesurée), première règle activée seule, dossier ou nom choisi qui l'emporte, protocole (HTTP rangé et renommé, dossier absolu, retéléchargement, jeton sans commandes ni écriture), torrent (dossier seulement), vidéo (dossier et nom, flux qui suivent), CLI.
- **4b, commande de fin : moteur** (2026-11-16) : module `postproc` (`finish` à la place de `complete_task` à la fin de chaque sorte de tâche : terminée, ou `post_processing` si la règle a une commande ; `run` : programme lancé sans shell ni fenêtre, entrée et sorties fermées, arbre de processus tué au délai ou à l'arrêt du moteur par `media::process`, `{sha256}` calculé hors de la boucle seulement s'il est demandé), lancée par l'exécution du planificateur après la fin de la tâche, et au démarrage pour une tâche restée `post_processing` ; torrent : `torrent::after_command` (partage ou fin), transition `post_processing` → `seeding`. Les flux internes d'une vidéo n'ont jamais de règle. Tests avec de vrais programmes du système (`cmd.exe`, `/bin/sh`) : variables en arguments et dans l'environnement, code non nul, délai dépassé (arbre tué), relance de la commande seule sans rien retélécharger, règle retirée, arrêt du moteur pendant la commande puis relance au démarrage, torrent (commande avant le partage).
- **4c, bureau et page** (2026-11-17) : rubrique « Règles » des Options (liste dans l'ordre avec le résumé « si… alors… » de la règle choisie ; Nouvelle, Modifier, Monter, Descendre, Activer ou Désactiver, Supprimer ; chaque changement enregistre toute la liste par `rules.set`). Boîte d'une règle : nom, activée ; « Si » : types en cases à cocher, extensions, domaines, expressions sur l'adresse et le nom ; « Alors » : dossier (« Parcourir… » au bureau), modèle de nom, commande (programme, arguments un par ligne, délai), au bureau seulement ; le refus du moteur s'affiche dans la boîte, qui reste ouverte. Page distante : la liste et les résumés seulement, sans commandes ni boutons. La boîte « Ajouter » n'envoyait déjà pas de dossier : les règles s'y appliquent. `POSTPROCESS_FAILED` a son libellé (« commande de fin en échec »). Fonctions de la page testées sans navigateur (`ruleFromFields`, `ruleForSet`, `ruleSummary`, `ruleError`, `moveRule`, `splitList`). Le refus d'un champ s'affiche sans son chemin technique (`rules[0] : actions.folder :`), curseur dans le champ ; une extension se lit avec son point dans le résumé.
- **Essai de l'interface** (2026-11-17, variante d'essai, validé par l'utilisateur) : règles et commande de fin (rangement et nom, refus de `../x`, ordre, désactivation, commande réussie puis en échec, reprise sans retéléchargement, retéléchargement d'une tâche rangée), proxy (adresse refusée, faux proxy, exception), journaux et diagnostic, anglais. Deux défauts d'affichage trouvés et corrigés : le `task.updated` de la préparation ne portait pas `destDir`, et les Propriétés montraient le dossier de l'ajout au lieu de celui de la règle (section 6.5) ; l'avancement d'une tâche dont la commande de fin échoue restait au dernier `task.progress` (64,7 % pour un fichier complet) : la page et Android tiennent `doneBytes` pour complet dès `verifying`, `post_processing`, `seeding` et `completed`.

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
| Exposition de l'adresse IP en torrent | Les pairs voient l'adresse de l'utilisateur | Option de liaison à une interface réseau précise (VPN) ; arrêt du torrent si cette interface disparaît (réglage `torrent.interface`, fait le 2026-11-09, section 7.16 : les torrents attendent son retour) |

Aléa cryptographique : TLS repose sur l'aléa du système (`getrandom` sous Linux et Android, le générateur du système sous Windows), sur lequel s'amorce aws-lc, le fournisseur cryptographique de rustls. Depuis le 2026-11-06, aws-lc est compilé sans sa source d'appoint, l'entropie de gigue du processeur (`AWS_LC_SYS_NO_JITTER_ENTROPY=1`, dans `.cargo/config.toml`). Cette source coûtait 56 à 60 ms de processeur au premier tirage de chaque processus (`docs/MESURES.md`). Elle sert surtout aux certifications FIPS, que le projet ne vise pas. L'aléa du système est celui qu'utilisent par défaut OpenSSL, GnuTLS et ring.

### 8.2 Vie privée

- Aucune télémétrie, aucun compte, aucune requête vers un serveur du projet en dehors de la vérification de mises à jour, désactivable.
- Un mode « sans historique » n'écrit ni `history` ni `event_log`.
- Les journaux et rapports de diagnostic remplacent les adresses complètes par le nom d'hôte, sauf demande contraire.
- L'extension navigateur ne lit que les requêtes de téléchargement et les pages que l'utilisateur lui désigne ; elle ne transmet rien ailleurs qu'au moteur local. Les cookies qu'elle transmet sont ceux de l'adresse téléchargée, et seulement si l'option est active ; le moteur les garde dans le coffre du système, jamais dans la base ni dans ses journaux, et les efface à la fin de la tâche.

### 8.3 Cadre légal

Ce qui suit n'est pas un avis juridique ; un juriste doit le valider avant la première publication.

- **L'outil est légal, l'usage dépend du contenu.** Le projet ne doit pas faciliter le contournement de mesures techniques de protection (DRM), que prohibent la plupart des législations, dont la directive européenne 2001/29/CE et le DMCA américain. La v1 exclut donc ce périmètre (section 1) et le projet ne livre aucun plugin de ce type.
- **Composants externes.** yt-dlp et ffmpeg sont appelés comme processus séparés, sans être liés au programme, ce qui garde la licence du projet indépendante de la leur. Un contrôle automatique des licences (de type `cargo-deny`) vérifie à chaque build que chaque dépendance est compatible avec la GPL-3.0.
- **Mise en œuvre du contrôle (2026-11-09, phase 5).** `deny.toml` liste les licences admises : permissives (MIT, MIT-0, 0BSD, ISC, BSD-3-Clause, Zlib, Unicode-3.0, CC0-1.0, Unlicense), Apache-2.0 (avec ou sans l'exception LLVM ; compatible avec la GPL-3.0, pas avec la GPL-2.0 seule), MPL-2.0 (compatible par sa section 3) et CDLA-Permissive-2.0 (licence de données, attribution seule : les certificats racines de Mozilla dans `webpki-root-certs`). `cargo deny check licenses` contrôle l'espace de travail principal et, avec `--manifest-path apps/desktop/Cargo.toml --config deny.toml`, l'application de bureau, toutes cibles comprises (Windows, Linux, Android). Ajouter une licence à la liste demande d'avoir vérifié sa compatibilité et de la noter ici. Sans intégration continue pour l'instant, le contrôle se lance à la main avec les tests ; il rejoindra la CI. Les dépendances Gradle de l'application Android (AndroidX, Jetpack Compose, Kotlin : Apache-2.0 ; partie Kotlin de `rustls-platform-verifier` : MIT ou Apache-2.0) ne passent pas par `cargo-deny` : relevé à la main le même jour.
- **Copies corrigées de dépendances (2026-11-09, étape 3d de la phase 5).** `third_party/` contient `librqbit` 9.1.0 et `librqbit-dualstack-sockets` 0.7.0, modifiées pour la liaison à une interface sous Windows (section 7.16) et branchées par `[patch.crates-io]`. Leur licence, Apache-2.0, permet de les modifier et de les redistribuer : texte complet dans `third_party/LICENSE-APACHE-2.0`, et chaque fichier modifié le signale en tête (section 4 de la licence). `cargo deny` les contrôle comme les autres dépendances. La crate `network-interface` (MIT ou Apache-2.0), déjà tirée par librqbit, est désormais aussi une dépendance directe du moteur.
- **Interface distante, phase 6, étape 3a (2026-11-11).** Nouvelles dépendances du moteur : `tokio-tungstenite` et `tungstenite` (MIT), et leurs dépendances `data-encoding` (MIT) et `httparse` (MIT ou Apache-2.0) ; `hyper`, `hyper-util`, `http-body-util` et `futures-util` (MIT, MIT ou Apache-2.0) étaient déjà dans l'arbre par reqwest. Aucune licence nouvelle.
- **Plugins d'hébergeurs, phase 6, étape 4b (2026-11-12).** Nouvelles dépendances du moteur : `wasmi`, `wasmi_core`, `wasmi_ir`, `wasmi_collections` (MIT ou Apache-2.0), `wasmparser` (Apache-2.0 WITH LLVM-exception, déjà admise) ; `wat` (même licence) pour les plugins d'essai, en développement et dans la fonctionnalité `testing` seulement. Aucune licence nouvelle (`cargo deny check licenses` passe).
- **Accès à distance du bureau, phase 6, étape 3d (2026-11-12).** Nouvelle dépendance de l'application de bureau : `qrcode` (MIT ou Apache-2.0), sans ses fonctions facultatives (aucune dépendance de plus). Aucune licence nouvelle.
- **Interface distante, phase 6, étape 3b (2026-11-11).** `rcgen` (MIT ou Apache-2.0), jusque-là réservée aux tests, devient une dépendance du moteur ; nouvelles : `x509-parser` et ses dépendances `asn1-rs`, `der-parser`, `oid-registry`, `rusticata-macros` (MIT ou Apache-2.0), `nom` 7 et `lazy_static` (MIT, MIT ou Apache-2.0) ; `tokio-rustls`, `time` et `yasna` (MIT ou Apache-2.0) étaient déjà dans l'arbre. Aucune licence nouvelle (`cargo deny check licenses` et sa variante du bureau passent).
- **Presse-papiers du bureau, phase 6, étape 2c (2026-11-11).** Nouvelles dépendances de l'application de bureau : `arboard` (lecture du texte du presse-papiers) et sa dépendance Windows `clipboard-win`, sous Boost Software License 1.0 (BSL-1.0), ajoutée à `deny.toml` : licence permissive (conserver l'avis de licence dans les copies du code source, rien pour les binaires), que la FSF classe compatible avec la GPL ; `tauri-winrt-notification` (MIT ou Apache-2.0, Windows) et `notify-rust` (MIT ou Apache-2.0, Linux) pour les notifications avec clic.
- **Vidéo, étape 4a (2026-11-09).** Nouvelles dépendances du moteur : `zip` (MIT, Windows), `tar` et `xz2` (MIT ou Apache-2.0, Linux ; `xz2` compile liblzma, du domaine public et sous 0BSD), `aws-lc-rs` en direct (déjà présente). yt-dlp (Unlicense) et ffmpeg (builds GPL de BtbN) ne sont ni liés ni fournis : le moteur les télécharge sur la demande de l'utilisateur et les lance comme processus séparés. La clé publique de yt-dlp copiée dans le dépôt vient d'un projet en Unlicense.
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
| Contrat du protocole | Échanges de référence validés par des schémas JSON ; un client de la version précédente doit toujours fonctionner | Oui. Fait le 2026-11-12 (phase 7, étape 1) : schémas JSON Schema 2020-12 de toutes les méthodes et notifications (`docs/protocol`), réponses et notifications de tous les tests du protocole validées, corpus des requêtes réelles de la page, de la CLI, du relais, d'Android et du bureau rejoué contre le moteur, garde de compatibilité avec la version 1 figée |
| Plateformes | Intégration continue sur Windows, Linux et émulateur Android | Oui |
| Accessibilité | Parcours au clavier et avec les lecteurs d'écran NVDA (Windows), Orca (Linux) et TalkBack (Android) | Pour la v1 seulement |

Le banc de reprise sous arrêt brutal (`crates/cli/tests/crash_resume.rs`) tue le processus à un instant aléatoire et relance la tâche. Après chaque arrêt, il vérifie les règles 1 à 4 de la section 5.6 sur la base et le `.part`, puis compare chaque fichier terminé octet par octet au contenu servi. Une version courte tourne à chaque intégration continue ; la version de la porte de phase 1 se règle par variables d'environnement. Tuer le processus laisse intact le cache du système : une coupure de la machine elle-même demande un banc séparé (machine virtuelle coupée net, ou système de fichiers qui simule la perte du cache), à prévoir.

Le banc de reprise du moteur autonome (`crates/daemon/tests/crash_daemon.rs`, phase 2) fait de même avec `aurod`. Plusieurs tâches tournent à la fois et le banc en ajoute par le protocole pour en garder toujours quatre en cours. Un cycle sur cinq finit par un arrêt demandé (`engine.shutdown`) au lieu d'un arrêt brutal. Les contrôles sont communs aux deux bancs (`testing::Checker`). Campagne du 2026-11-06 sous Windows 11, sans aucun défaut : 150 lancements (graine 1793925077934193901), 120 arrêts brutaux et 30 arrêts demandés, 73 fichiers de 4 Mio terminés et vérifiés octet par octet, en 4 min 8 s. Aucun arrêt n'y tombe sur un découpage redécoupé, car chaque tâche garde le nombre de connexions fixé à l'ajout : le redécoupage sous arrêt brutal reste couvert par le banc de la phase 1.

Le banc de reprise des torrents (`crates/daemon/tests/crash_torrent.rs`, phase 5, section 7.16) tue `aurod` pendant qu'il télécharge trois torrents à la fois depuis un pair local, et simule en plus, sur un arrêt sur deux, une écriture perdue dans un `.part`. Chaque torrent terminé est comparé octet par octet au contenu partagé. Campagnes du 2026-11-09, 150 lancements chacune, trois torrents d'environ 2 Mio, sans aucun défaut :
- Linux (WSL, graine 1794236763982147965) : 120 arrêts brutaux et 30 arrêts demandés, 242 torrents terminés et vérifiés, 51 plages effacées dans un `.part`, 6 arrêts entre un renommage et l'enregistrement de la fin, en 4 min 36 s ;
- Windows 11 (graine 1794245565815179301) : 120 arrêts brutaux et 30 arrêts demandés, 227 torrents terminés et vérifiés, 37 plages effacées, en 4 min 59 s.

La première campagne avait échoué par la faute du pair de test, et non du moteur (section 7.16, étape 3c).

Le banc de reprise des vidéos (`crates/daemon/tests/crash_media.rs`, phase 5, section 7.17) tue `aurod` pendant qu'il télécharge trois vidéos à la fois. Chaque vidéo a deux flux : un flux fragmenté, que le faux yt-dlp écrit en une à deux secondes, et un flux direct de 1 Mio, servi en local, bridé, avec plages et ETag, que le moteur télécharge par sa tâche interne. Le faux ffmpeg met une seconde à fusionner, pour que des arrêts tombent aussi pendant la fusion. Après chaque arrêt, le banc vérifie qu'aucune vidéo n'est en échec, que chaque vidéo terminée est juste octet par octet, et qu'elle ne laisse ni flux ni `.part` ; à la fin, que le dossier ne contient que les vidéos.

Campagnes du 2026-11-09, 150 lancements chacune, sans défaut :
- Windows 11 (graine 1794262139524606701) : 120 arrêts brutaux et 30 arrêts demandés, 119 vidéos terminées et vérifiées, 64 arrêts pendant une fusion, en 9 min 22 s ;
- Linux (WSL, graine 1794263431687590387) : 120 arrêts brutaux et 30 arrêts demandés, 141 vidéos terminées et vérifiées, 33 arrêts pendant une fusion, en 9 min 32 s.

Les premières campagnes avaient trouvé deux défauts du moteur, corrigés depuis (section 7.17, étape 4d) : une vidéo arrêtée pendant sa fusion restait bloquée, et, sous Linux, des outils survivaient à un `aurod` tué.

Campagne de la porte de la phase 5 (2026-11-10), sur le code de la fin de l'étape 5, à 150 cycles (300 pour le banc de reprise) :
- Windows 11 : reprise, 70 fichiers vérifiés ; `aurod`, 64 ; torrents, 225 torrents vérifiés et 33 plages effacées ; vidéos : le premier passage a échoué (vidéo `MERGE_FAILED` après un arrêt tombé entre le renommage du fichier final et l'enregistrement de la fin), et trois passages sur huit en tout. Défaut corrigé (section 7.17, `already_merged`), puis onze passages de suite réussis. Un des passages en échec comptait aussi cinq fichiers de trop dans le dossier, sans que leurs noms aient été relevés ; non revu depuis la correction, le banc les nomme désormais.
- Linux (WSL), après la correction : reprise, 71 fichiers ; `aurod`, 71 ; torrents, 221 et 47 plages effacées ; vidéos, 144 vérifiées dont 30 arrêts pendant une fusion.
- Fuzzing des `.torrent` et des liens magnet (`torrent::fuzz`, section 7.16) : il a trouvé deux fichiers d'un torrent écrits au même chemin une fois les noms nettoyés ; ces torrents sont désormais refusés.
- Essais réels : vidéos libres (`real_probe`, `real_free`) sous Windows et Linux ; netinst Debian complet par lien magnet sous Linux (23 s, empreinte juste) ; torrents sur le téléphone Android (section 4.4) ; extension sous Edge (section 4.5).
- Licences : `cargo deny check licenses` sur les deux espaces de travail ; dépendances Gradle inchangées depuis leur relevé du 2026-11-09.

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
| yt-dlp est écrit en Python | Intégration lourde sur Android | Décision de l'utilisateur du 2026-11-09 : la vidéo seulement sous Windows et Linux en v1 ; le torrent va aussi sur Android |
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

État des portes : phase 1 franchie le 2026-10-16. Phase 2 franchie le 2026-11-08, déclarée par l'utilisateur : l'application installée sert tous les jours depuis le 2026-11-07, sans perte de données. Phase 3 franchie le 2026-11-08 : les 9 essais de `docs/ESSAI-EXTENSION.md` validés par l'utilisateur sous Firefox, Chrome et Edge, ainsi que CCleaner Browser (Chromium). Phase 4 franchie le 2026-11-09, déclarée par l'utilisateur : la porte se jugeait sur deux appareils réels, une tablette Teclast T70 (Android 15) et un Galaxy S25 Ultra (Android 16), sans émulateur (décision de l'utilisateur du 2026-11-08) ; Android 8 à 14 (N-08) restent sans essai sur appareil. Essais de la porte faits le 2026-11-09 (section 7.15) : DVD Debian de 3,7 Gio mené à terme avec l'empreinte publiée sur les deux appareils, à travers un redémarrage sur la tablette et un arrêt brutal du processus sur le téléphone, passé entre-temps à Android 17 ; Android 17 accepté par l'utilisateur à la place d'Android 16. Phase 5 franchie le 2026-11-10, déclarée par l'utilisateur après la campagne du même jour (section 9.1 : tests d'intégration vidéo et torrent, bancs, essais réels, fuzzing des `.torrent`, licences) ; restent pour plus tard les essais de l'extension sous Firefox et Chrome, le partage d'un torrent et l'ouverture d'un `.torrent` depuis Fichiers sur Android. Phase 6 franchie le 2026-11-12, déclarée par l'utilisateur sur la revue de sécurité de l'interface distante, des plugins et du canal local (`docs/SECURITE.md`, six constats corrigés) et sur la campagne du même jour (bancs, fuzzing long, essais réels sous Windows et Linux).

Phase 7, plan du 2026-11-12 (décisions de l'utilisateur) : contrat du protocole (schémas JSON, client de la version précédente) ; journaux et diagnostic (N-12) ; exigences S entrées dans la v1 : proxy (F-07), règles de rangement et commande de fin (F-32, F-34), segment fautif (F-12), FTP, FTPS et SFTP (F-08) ; accessibilité (contrôles WCAG 2.2 AA et tests automatiques, essais réels NVDA, Orca et TalkBack par l'utilisateur) ; dépôt GitHub et intégration continue (Windows, Linux, émulateur Android), ARM64 construits sans essai sur matériel réel, installateurs Windows signés par SignPath, mise à jour du bureau signée, proposée au plus une fois par jour et installée sur accord ; guide de l'utilisateur en français et en anglais ; porte : tous les tests bloquants de la section 9.1, dont le banc de 1 000 reprises sur 1 Gio.

En phase 1, la ligne de commande appelle le moteur comme une bibliothèque ; l'API JSON-RPC (section 6) arrive en phase 2, avec l'application de bureau.

Les deux points les plus incertains, l'écriture sur le stockage Android et la capture dans le navigateur, sont testés dans les phases 1 et 3, avant que le reste du produit ne dépende d'eux.

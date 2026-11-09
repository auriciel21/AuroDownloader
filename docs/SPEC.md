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
| N-11 | Licence | Logiciel libre | GPL-3.0 ou ultérieure pour le moteur et les clients (décision de l'utilisateur du 2026-11-09 ; texte dans `LICENSE`, champ `license` des paquets Cargo) |
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
| Torrent | Bibliothèque Rust ou `libtorrent`, derrière une interface interne | À trancher par un prototype ; l'interface permet d'en changer. Prototype du 2026-11-09 (`experiments/torrent/README.md`) : librqbit 9.1.0 face à libtorrent 2.1.1 ; ni l'une ni l'autre ne garantit la règle 1 (5.6) après une coupure, que le module `torrent` devra assurer lui-même. Décision de l'utilisateur du 2026-11-09 : librqbit ; acceptés pour la v1 : pas de chiffrement du protocole, ni sources web, ni torrents v2 ; la liaison à une interface sous Windows sera ajoutée par le projet |
| Vidéo | yt-dlp et ffmpeg en processus externes | Extracteurs maintenus par d'autres, licences séparées, mises à jour indépendantes (F-25) |
| Interface de bureau | Tauri 2 (choisi le 2026-11-06, après deux prototypes comparés sous Windows et Linux : `experiments/desktop/README.md`) | Une seule page web pour le bureau et l'interface distante (F-39), servie par l'hôte de chacune ; client du moteur en Rust, repris du moteur. Qt Quick était plus léger (mémoire, processeur fenêtre ouverte, démarrage), mais aurait demandé une seconde interface |
| Interface Android | Kotlin avec Jetpack Compose, moteur chargé comme bibliothèque native | Partage, services et stockage sont des API Android natives |
| Extension | WebExtension (Chrome, Edge, Firefox) et programme relais natif | Seule voie propre vers le moteur local |
| Plugins | WebAssembly | Bac à sable, plusieurs langages possibles |

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
- **Pas encore** : cookies partitionnés (CHIPS) et posés pendant une redirection, détection des vidéos, publication dans les boutiques (Firefox n'installe une extension non signée que temporairement), Brave, Vivaldi et Opera (CCleaner Browser, autre navigateur Chromium, a passé les essais de la phase 3 le 2026-11-08 avec la même extension et le même relais). Un autre gestionnaire intégré au navigateur (IDM) peut intercepter un téléchargement avant l'extension.

### 4.6 Exécution et distribution

- **Bureau** : l'application démarre le moteur s'il n'est pas lancé et le laisse tourner dans la zone de notification ; démarrage avec la session en option. Un téléchargement ajouté par un autre client (extension, ligne de commande) fait apparaître la fenêtre au premier plan, même repliée dans la zone de notification (préférence de l'interface, activée par défaut, depuis le 2026-11-08) : l'utilisateur voit tout de suite que le téléchargement a été pris.
- **Moteur** : binaire `aurod` (crate `crates/daemon`), sans interface. L'application de bureau le lance s'il ne tourne pas ; `auro` est le client en ligne de commande, et le lance de même, détaché de la console, avec son journal (`aurod.log`) à côté de sa base. `auro direct` exécute une tâche avec la bibliothèque du moteur, sans `aurod`, pour les bancs et le dépannage.
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

Tout l'état durable tient dans une base SQLite unique, dont le moteur est le seul écrivain. Elle contient 16 tables (14 à l'origine, plus `file_moves` et `site_logins` depuis le 2026-11-08) ; c'est elle qui rend possible la reprise après un arrêt brutal (N-04, N-05).

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
| `verifying` | Vérification de l'empreinte du fichier complet | `paused`, `post_processing`, `completed`, `failed` |
| `post_processing` | Rangement, renommage, extraction, commande externe | `completed`, `failed` |
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
6. **Rétention.** `event_log` est purgé au-delà de 30 jours ou de 10 000 lignes ; la durée de l'historique est un réglage.

## 6. Protocole du moteur

Le moteur parle JSON-RPC 2.0, quel que soit le client ou le transport : application de bureau, ligne de commande, extension, interface web ou application Android. Un client qui sait appeler `task.add` fonctionne donc partout, et l'API publique est la même que celle utilisée par l'interface officielle (principe 4).

### 6.1 Principes

- **JSON-RPC 2.0** : requêtes avec `id`, notifications sans `id`, envois groupés (batch) acceptés.
- **Paramètres nommés** : `params` est toujours un objet, jamais un tableau.
- **Noms de méthodes** : `espace.verbe`, en minuscules et casse mixte (`task.add`, `queue.update`).
- **Dates et tailles** : millisecondes Unix en UTC, octets, octets par seconde, tous en entiers.
- **Évolution** : ajouter un champ optionnel ou une méthode ne casse rien ; supprimer ou renommer exige une nouvelle version majeure du protocole.
- **Idempotence** : `task.add` accepte une `idempotencyKey` ; rejouer le même appel ne crée pas de doublon (utile pour l'extension qui réessaie). Mise en œuvre (2026-11-07) : une clé de 1 à 256 octets ; un ajout dont la clé est déjà celle d'une tâche présente rend cette tâche, sans rien changer ni annoncer, même si ses autres paramètres diffèrent ; la clé est libérée quand la tâche est retirée.

### 6.2 Transports et authentification

| Transport | Usage | Authentification |
| --- | --- | --- |
| Socket Unix (Linux) ou canal nommé (Windows) | Application de bureau et CLI sur la même machine | Droits du système de fichiers : seul l'utilisateur propriétaire y accède |
| Appel direct dans le processus (FFI) | Android, où le moteur vit dans l'application | Aucune : même processus |
| WebSocket sur `127.0.0.1` | Interface web, outils de développement | Jeton porteur dans l'en-tête `Authorization` ou dans le premier message |
| WebSocket sur le réseau, en TLS | Moteur distant (NAS, serveur) | Jeton porteur, TLS obligatoire dès que l'écoute n'est pas locale |
| HTTP `POST /rpc` | Scripts simples sans notifications | Jeton porteur |
| Messagerie native du navigateur | Extension, via un petit programme relais | Le relais passe par le canal local et s'y limite à la portée `add` (voir 6.9) |

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

Capacités annoncées par ce moteur : `torrent` depuis le 2026-11-09 (section 7.16) ; avant, aucune.

Aucune plage commune : le moteur répond par l'erreur `PROTOCOL_UNSUPPORTED` et ferme la connexion.

Paramètre facultatif `scope` (depuis le 2026-11-07) : la portée que le client garde pour cette connexion (section 6.9). Elle ne peut que se restreindre ; une portée plus large, ou de côté, est refusée par `UNAUTHORIZED`. La réponse donne la portée de la connexion dans `scope`.

### 6.4 Méthodes

| Méthode | Paramètres principaux | Résultat |
| --- | --- | --- |
| `engine.hello` | `client`, `protocol`, `scope` | Version retenue, capacités, `rev`, `scope` |
| `engine.stats` | aucun | Débit global, tâches actives, espace disque |
| `engine.shutdown` | `graceful` | Arrêt après sauvegarde de l'état |
| `task.add` | `sources[]`, `queueId`, `destDir`, `fileName`, `maxConnections`, `speedLimitBps`, `checksum`, `headers`, `startPaused`, `idempotencyKey` | `taskId` |
| `task.addBatch` | `items[]` de paramètres `task.add` | `taskIds[]` |
| `task.get` | `taskId`, `include` (`segments`, `sources`, `files`) | Tâche complète |
| `task.list` | `queueId`, `states[]`, `limit`, `cursor` | `items[]`, `nextCursor`, `rev` |
| `task.pause` / `task.resume` | `taskIds[]` | Liste des tâches modifiées |
| `task.cancel` | `taskIds[]`, `deleteFiles` | Liste des tâches modifiées |
| `task.remove` | `taskIds[]`, `deleteFiles` | Nombre de tâches retirées |
| `task.restart` | `taskIds[]`, `deleteFiles` | Tâches reparties de zéro, et refus (`errors`) |
| `task.update` | `taskId`, `maxConnections`, `speedLimitBps`, `destDir`, `fileName`, `url`, `checksum` | Tâche modifiée |
| `task.move` | `taskIds[]`, `queueId`, `position`, `priority` | Liste des tâches modifiées |
| `task.addSource` | `taskId`, `url`, `rank` | `sourceId` |
| `task.setFiles` | `taskId`, `selected[]` (torrent) | Tâche modifiée |
| `queue.list` / `queue.create` / `queue.update` / `queue.delete` | `name`, `maxActive`, `speedLimitBps`, `scheduleId` | File ou liste de files |
| `limits.get` / `limits.set` | `downloadBps`, `uploadBps` | Limites globales |
| `schedule.list` / `schedule.set` / `schedule.delete` | `scheduleId`, `name`, `enabled`, `rules` | Planification, avec son ouverture (section 7.11) |
| `login.list` / `login.set` / `login.delete` | `loginId`, `site`, `username`, `password` | Identifiants de sites, sans mot de passe (section 7.14, depuis le 2026-11-08) |
| `engine.cancelEndAction` | aucun | `cancelled` : une action de fin annoncée a été annulée |
| `engine.setNetwork` | `usable` (booléen) | `usable` : le réseau convient, ou non, aux téléchargements (section 7.15, depuis le 2026-11-09) |
| `linkscan.run` | `text` ou `pageUrl`, `filters` | `links[]` avec nom, taille, type |
| `media.probe` | `pageUrl` | `formats[]` disponibles |
| `history.list` / `history.clear` | `query`, `before`, `limit`, `cursor` | Entrées d'historique ; nombre d'entrées effacées (section 6.5, depuis le 2026-11-08) |
| `settings.get` / `settings.set` | `keys[]` ou `values` | Réglages (`values`, par clé) |
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
| `schedule.updated` | Planification créée, modifiée, ou dont l'ouverture vient de changer, comme `schedule.list` la décrit (depuis le 2026-11-07) |
| `schedule.removed` | `scheduleId` (depuis le 2026-11-07) |
| `engine.endAction` | `pending` : action de fin annoncée (`queueId`, `action`, `at`), ou `null` quand elle est exécutée ou annulée ; `error` : refus du système (depuis le 2026-11-07) |
| `engine.stats` | Débit global, toutes les secondes |
| `engine.warning` | `code`, `message` (disque presque plein, nouvelle version de yt-dlp disponible ; `EVENTS_LOST` : client trop lent, des notifications ont été perdues, appeler `sync.snapshot`) |

Sujets d'abonnement : `tasks` (`task.added`, `task.stateChanged`, `task.updated`, `task.removed`), `queues` (`queue.updated`, `limits.updated`, `settings.updated`, `schedule.updated`, `schedule.removed`, `engine.endAction`), `progress` (`task.progress`). Dans `task.progress`, `doneBytes` compte les octets reçus, pas forcément vidés sur disque ; `bps` est mesuré sur les 6 dernières secondes, entre les instants où les octets ont augmenté, et décroît jusqu'à zéro quand rien n'arrive plus ; `etaS` en découle (depuis le 2026-11-07 ; d'un envoi à l'autre auparavant, ce qui donnait, avec des octets arrivés par blocs, tantôt zéro, tantôt le double du vrai débit). Le moteur mesure l'avancement toutes les 500 ms : un intervalle plus court n'apporte rien de plus.

État de la mise en œuvre (phase 2) :
- **Méthodes servies** : `engine.hello`, `engine.shutdown`, `engine.cancelEndAction`, `engine.setNetwork`, `task.add`, `task.get`, `task.list`, `task.pause`, `task.resume`, `task.cancel`, `task.remove`, `task.restart`, `task.update`, `queue.list`, `queue.create`, `queue.update`, `limits.get`, `limits.set`, `settings.get`, `settings.set`, `schedule.list`, `schedule.set`, `schedule.delete`, `login.list`, `login.set`, `login.delete`, `history.list`, `history.clear`, `events.subscribe`, `events.unsubscribe` et `sync.snapshot`. Les autres répondent « méthode inconnue » jusqu'à leur arrivée.
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
  - `task.get` avec `include: ["files"]` ajoute `files[]` : `index`, `path` (chemin dans le torrent, composants nettoyés et séparés par `/`), `length`, `selected`, `doneBytes` (indication). Vide pour une tâche HTTP.
  - `task.progress` porte, pour un torrent, `uploadedBytes` (cumulés) et `uploadBps`.
  - `limits.set` et `limits.get` : `uploadBps`, limite d'envoi globale ; annoncée par `limits.updated`, avec `downloadBps`. La limite globale de réception vaut aussi pour les torrents, par le limiteur de librqbit, à part de celle du HTTP.
  - Réglages (`settings.get`, `settings.set`) : `torrent.ratioLimit` (nombre positif ou nul, `null` pour sans limite ; 1 par défaut ; `settings.updated`) et `torrent.upnp` (booléen, faux par défaut ; vaut à la prochaine ouverture de la session).
  - `engine.setNetwork` accepte `metered` (booléen, faux par défaut) ; `engine.network` et `sync.snapshot` (`networkMetered`) le rendent (section 7.16).
- **Paramètres refusés** : un paramètre prévu mais pas encore pris en charge est refusé par `CAPABILITY_MISSING`, et non ignoré en silence. Cela concerne :
  - dans `task.add` : plusieurs sources (`fileName` est pris en charge depuis le 2026-11-08, section 7.12 ; `checksum` depuis le 2026-11-08, section 7.9) ;
  - dans `task.update` : `maxConnections`. Sont pris en charge `speedLimitBps` ; `destDir` et `fileName` (section 7.12) ; `url` (section 7.13, depuis le 2026-11-08) ; `checksum` (section 7.9, depuis le 2026-11-08). Chacun de ces quatre groupes passe par un appel séparé (`INVALID_PARAMS` sinon) : chaque opération est vérifiée en entier avant de rien changer, mais pas leur combinaison ;
  - (`uploadBps` de `limits.set` était refusé jusqu'au 2026-11-09 ; il est pris en charge depuis, avec les torrents.)
- **Limites de débit** (`speedLimitBps`, `downloadBps`) : un entier d'au moins 1, en octets par seconde.
  - Dans `task.update`, `queue.update` et `limits.set`, `null` retire la limite et un paramètre absent la laisse telle quelle.
  - Une nouvelle limite s'applique aussitôt aux tâches en cours (section 7.6).
  - `task.update` annonce `task.updated`, avec `taskId` et les seuls champs modifiés.
  - Un changement de la limite globale est annoncé par `limits.updated`, sujet `queues`, avec une nouvelle révision (depuis le 2026-11-07 ; avant, rien ne l'annonçait). `sync.snapshot` porte les limites dans `limits`.
- **Nom et taille d'une tâche** (depuis le 2026-11-07) : `task.added` les ignore encore,
  la préparation les apprend (section 7.1). Après chaque préparation, le moteur annonce
  `task.updated` avec `fileName`, `displayName` et `totalBytes`, et une nouvelle révision.
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
    au dernier point de contrôle. Un client qui veut suivre la carte des connexions
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

### 6.9 Portées des jetons

| Portée | Autorise | Typiquement donnée à |
| --- | --- | --- |
| `read` | Méthodes `*.list`, `*.get`, `engine.stats`, abonnements | Un tableau de bord en lecture seule |
| `add` | `task.add`, `task.addBatch`, `media.probe`, `linkscan.run` | L'extension navigateur, un partage Android |
| `control` | Tout ce que fait `add`, plus `task.*` et `queue.*` | L'application de bureau, la CLI, l'interface web |
| `admin` | Tout, dont `settings.set`, `engine.shutdown` et la gestion des jetons | Le propriétaire de la machine |

Mise en œuvre (décision du 2026-11-07, phase 3) :
- **Canal local** : une connexion a d'emblée la portée `admin`, le système réservant le canal à l'utilisateur (section 6.2). Un client peut se restreindre à l'ouverture (`engine.hello`, paramètre `scope`), jamais s'élargir. Le relais de l'extension se limite ainsi à `add`. Aucun jeton : un programme qui pourrait lire celui du relais pourrait aussi bien se connecter lui-même au canal.
- **Jetons** : réservés aux transports réseau (WebSocket, HTTP), avec l'interface distante (F-39, phase 6).
- **Portées exigées** :
  - aucune : `engine.hello` ;
  - `read` : `task.get`, `task.list`, `queue.list`, `limits.get`, `settings.get`, `schedule.list`, `history.list`, `events.subscribe`, `events.unsubscribe`, `sync.snapshot`, `engine.stats` ;
  - `add` : `task.add`, `task.addBatch`, `media.probe`, `linkscan.run` ;
  - `control` : les autres méthodes `task.*` et `queue.*`, `limits.set`, `engine.cancelEndAction`, `engine.setNetwork`, `history.clear` ;
  - `admin` : `engine.shutdown`, `settings.set`, `schedule.set`, `schedule.delete`, et `queue.create` ou `queue.update` qui portent `scheduleId` (une planification peut éteindre la machine) ; `login.list`, `login.set` et `login.delete` (identifiants de sites, section 7.14).
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

Si des empreintes par segment existent (torrent, Metalink), chaque segment est contrôlé dès qu'il est terminé et seul le segment fautif est téléchargé de nouveau (F-12).

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
- Liaison à une interface (VPN, section 8.1) : étape 3d, y compris sous Windows, que librqbit ne gère pas.
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
- **Reste de l'étape 3** : la liaison à une interface réseau, Windows compris (3d). La durée de partage (`torrents.seed_time_limit_s`) n'est pas encore suivie.

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
- L'extension navigateur ne lit que les requêtes de téléchargement et les pages que l'utilisateur lui désigne ; elle ne transmet rien ailleurs qu'au moteur local. Les cookies qu'elle transmet sont ceux de l'adresse téléchargée, et seulement si l'option est active ; le moteur les garde dans le coffre du système, jamais dans la base ni dans ses journaux, et les efface à la fin de la tâche.

### 8.3 Cadre légal

Ce qui suit n'est pas un avis juridique ; un juriste doit le valider avant la première publication.

- **L'outil est légal, l'usage dépend du contenu.** Le projet ne doit pas faciliter le contournement de mesures techniques de protection (DRM), que prohibent la plupart des législations, dont la directive européenne 2001/29/CE et le DMCA américain. La v1 exclut donc ce périmètre (section 1) et le projet ne livre aucun plugin de ce type.
- **Composants externes.** yt-dlp et ffmpeg sont appelés comme processus séparés, sans être liés au programme, ce qui garde la licence du projet indépendante de la leur. Un contrôle automatique des licences (de type `cargo-deny`) vérifie à chaque build que chaque dépendance est compatible avec la GPL-3.0.
- **Mise en œuvre du contrôle (2026-11-09, phase 5).** `deny.toml` liste les licences admises : permissives (MIT, MIT-0, 0BSD, ISC, BSD-3-Clause, Zlib, Unicode-3.0, CC0-1.0, Unlicense), Apache-2.0 (avec ou sans l'exception LLVM ; compatible avec la GPL-3.0, pas avec la GPL-2.0 seule), MPL-2.0 (compatible par sa section 3) et CDLA-Permissive-2.0 (licence de données, attribution seule : les certificats racines de Mozilla dans `webpki-root-certs`). `cargo deny check licenses` contrôle l'espace de travail principal et, avec `--manifest-path apps/desktop/Cargo.toml --config deny.toml`, l'application de bureau, toutes cibles comprises (Windows, Linux, Android). Ajouter une licence à la liste demande d'avoir vérifié sa compatibilité et de la noter ici. Sans intégration continue pour l'instant, le contrôle se lance à la main avec les tests ; il rejoindra la CI. Les dépendances Gradle de l'application Android (AndroidX, Jetpack Compose, Kotlin : Apache-2.0 ; partie Kotlin de `rustls-platform-verifier` : MIT ou Apache-2.0) ne passent pas par `cargo-deny` : relevé à la main le même jour.
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

Le banc de reprise des torrents (`crates/daemon/tests/crash_torrent.rs`, phase 5, section 7.16) tue `aurod` pendant qu'il télécharge trois torrents à la fois depuis un pair local, et simule en plus, sur un arrêt sur deux, une écriture perdue dans un `.part`. Chaque torrent terminé est comparé octet par octet au contenu partagé. Campagnes du 2026-11-09, 150 lancements chacune, trois torrents d'environ 2 Mio, sans aucun défaut :
- Linux (WSL, graine 1794236763982147965) : 120 arrêts brutaux et 30 arrêts demandés, 242 torrents terminés et vérifiés, 51 plages effacées dans un `.part`, 6 arrêts entre un renommage et l'enregistrement de la fin, en 4 min 36 s ;
- Windows 11 (graine 1794245565815179301) : 120 arrêts brutaux et 30 arrêts demandés, 227 torrents terminés et vérifiés, 37 plages effacées, en 4 min 59 s.

La première campagne avait échoué par la faute du pair de test, et non du moteur (section 7.16, étape 3c).

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

État des portes : phase 1 franchie le 2026-10-16. Phase 2 franchie le 2026-11-08, déclarée par l'utilisateur : l'application installée sert tous les jours depuis le 2026-11-07, sans perte de données. Phase 3 franchie le 2026-11-08 : les 9 essais de `docs/ESSAI-EXTENSION.md` validés par l'utilisateur sous Firefox, Chrome et Edge, ainsi que CCleaner Browser (Chromium). Phase 4 franchie le 2026-11-09, déclarée par l'utilisateur : la porte se jugeait sur deux appareils réels, une tablette Teclast T70 (Android 15) et un Galaxy S25 Ultra (Android 16), sans émulateur (décision de l'utilisateur du 2026-11-08) ; Android 8 à 14 (N-08) restent sans essai sur appareil. Essais de la porte faits le 2026-11-09 (section 7.15) : DVD Debian de 3,7 Gio mené à terme avec l'empreinte publiée sur les deux appareils, à travers un redémarrage sur la tablette et un arrêt brutal du processus sur le téléphone, passé entre-temps à Android 17 ; Android 17 accepté par l'utilisateur à la place d'Android 16.

En phase 1, la ligne de commande appelle le moteur comme une bibliothèque ; l'API JSON-RPC (section 6) arrive en phase 2, avec l'application de bureau.

Les deux points les plus incertains, l'écriture sur le stockage Android et la capture dans le navigateur, sont testés dans les phases 1 et 3, avant que le reste du produit ne dépende d'eux.

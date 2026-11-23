# Audit du code avant la version 1 (2026-11-19)

Audit demandé par l'utilisateur avant la création des installateurs (phase 7, avant la
porte de l'étape 10) : bogues, code mort, redondances, simplifications, et optimisations
seulement si un banc les mesure (décisions de l'utilisateur du 2026-11-18 : relecteurs en
parallèle, rapport d'abord, correctifs choisis ensuite).

## Méthode

- Neuf relectures en lecture seule, une par partie : stockage et reprise (S), transfert et
  réseau HTTP (T), FTP, FTPS et SFTP (F), planificateur et fin de tâche (P), protocole, canal
  local, coffre et journaux (R), torrent et vidéo (V), interface distante et plugins (D),
  clients du moteur : CLI, `aurod`, bureau, Android (C), page, extension, scripts et
  workflows (W). Chacune a relu le code de sa partie et les appelants utiles, et cherché le
  code mort par Grep sur tout le dépôt.
- Chaque constat critique ou majeur a été **revérifié dans le code** avant d'entrer ici
  (colonne « Vérifié »). Les constats mineurs et de nettoyage sont repris des relectures ;
  ils seront revérifiés un à un au moment de leur correctif. Aucun constat n'a encore été
  reproduit par un test : le test proposé de chaque correctif le fera d'abord échouer.
- Dépendances : chaque dépendance des `Cargo.toml` sert (vérifié à la main ; celles qui
  semblaient inutilisées s'emploient sous un autre nom ou par un fichier partagé).
- État de départ (campagne du 2026-11-18, code du commit a7704f1) : tout vert. Windows et
  Linux : fmt, clippy, tous les tests, licences, fuzzing long de l'interface distante,
  bancs d'`aurod`, des vidéos et des torrents (150 cycles), **banc de reprise de 1 000
  cycles sur 1 Gio** (Windows : 1 h 43, graine 1795036561871949301, 171 fichiers vérifiés ;
  Linux : 1 h 25, graine 1795036262937980791, 245 fichiers) ; page, extension, scripts,
  guide et bureau sous Linux. Les bancs ne pouvaient pas voir T1 ni P1 : aucun n'enchaîne
  « Actualiser l'adresse » vers un serveur sans plages, aucun ne mesure le processeur au
  repos.

Gravités : **critique** (perte ou corruption de données, ou moteur qui s'emballe dans un cas
courant) ; **majeure** (sécurité, fiabilité, ou fonction qui échoue dans un cas réaliste) ;
**mineure** ; **nettoyage** (code mort, redondance, simplification).

## Synthèse

| Gravité | Nombre | Dont vérifiés dans le code |
| --- | --- | --- |
| Critique | 2 | 2 |
| Majeure | 30 | 28 (S2 et D2 : course lue dans le code, non reproduite) |
| Mineure | 95 environ | à vérifier au correctif |
| Nettoyage et performance | 60 environ | à vérifier au correctif |

Doublons trouvés par deux relectures indépendantes, fusionnés : R3 = C1, R2 = C2, T6 = D10.

## 1. Critiques

| Id | Lieu | Défaut | Scénario | Correctif proposé | Test | Vérifié |
| --- | --- | --- | --- | --- | --- | --- |
| **T1** | `store/tasks.rs:807` (`replace_url`), `transfer.rs:801` et `2282` | « Actualiser l'adresse » vers un serveur sans plages enregistre `accepts_ranges = false` sans fusionner les segments. Chaque segment est ensuite demandé en entier (`Fetch::Whole`, sans `Range`) et ses octets, lus depuis l'octet 0, sont écrits à la place du segment. | Tâche à 4 segments en pause ; nouvelle adresse de même taille vers un serveur qui répond `200` à `bytes=0-0`. Le fichier finit `completed`, faux ; sans empreinte, rien ne le voit. | Ramener les segments à un seul `[0, taille−1]` dans la transaction de `replace_url`, par `segmenter` et `store::segments` (SPEC 7.13 : « repart du début ») ; et refuser dans `pump` un `Fetch::Whole` dont le segment ne commence pas à 0. | Tâche préparée à 4 segments en partie écrits, `set_url` vers un `FileSite` sans plages, fichier comparé octet par octet. Banc `crash_resume`. | oui |
| **P1** | `scheduler.rs:2478` (`next_wake`), `store/tasks.rs:1386` | La boucle du moteur dort jusqu'à la plus proche date de nouvel essai, même passée. Une tâche `retry_wait` échue qui ne peut pas partir reste `retry_wait` : le délai vaut 0 et la boucle tourne sans arrêt (requêtes SQL à chaque tour). | File pleine (une tâche en `retry_wait`, une autre a pris la place) ; file fermée par une plage ; réseau inutilisable (Android démarre sans réseau, « Wi-Fi seulement » sur réseau mobile) ; torrent qui attend son interface. Processeur occupé, batterie vidée. | Ne compter que les dates à venir, ou faire rendre à `start_runnable` la prochaine date d'une tâche qui pourrait vraiment partir. | File `max_active = 1`, une tâche longue et une `retry_wait` échue : `next_wake` > 0 (ou compteur de tours borné). Banc `crash_daemon`. | oui |

## 2. Majeures

### Intégrité des données et reprise

| Id | Lieu | Défaut et scénario | Correctif proposé | Test | Vérifié |
| --- | --- | --- | --- | --- | --- |
| **S1** | `io.rs:200` (`SyncGate::sync`) | Après un vidage raté, les fils qui l'attendaient en refont un, qui réussit. Sous Linux, une erreur de `fsync` n'est rendue qu'une fois et les pages perdues sont marquées propres : les relevés pris avant l'échec sont alors enregistrés pour des octets qui ne sont pas sur le disque (règle 1). Cas : clé USB ou NAS en `EIO`. | Porte définitivement en échec après la première erreur ; la reprise rouvre le fichier depuis les compteurs enregistrés. | `io::tests::gate` : un échec puis des vidages qui réussiraient, tous en échec ; test de `transfer` sans `save_progress` après l'échec. `crash_resume`. | oui (code) ; comportement du noyau documenté, non reproduit |
| **S2** | `scheduler.rs:4264` (`relocate`), `moves.rs:48`, `75` | « Déplacer/Renommer » ne réserve pas le nom visé (`name_claims`) et renomme avec remplacement. Une préparation en parallèle peut prendre le même nom : deux tâches dans le même `.part`, ou, pendant une copie de plusieurs minutes, une autre tâche qui échoue en `DestinationExists` à la fin. | Réserver le nom avant `moves::begin`, libérer dans la transaction de `commit` ou `forget` ; renommage sans remplacement (`MoveFileExW` sans `REPLACE_EXISTING`, `renameat2(RENAME_NOREPLACE)`). | Copie simulée vers un autre volume et `claims::claim` du même nom pendant la copie. `crash_resume`. | non (probable) |
| **S4** | `store/tasks.rs:918` (`change_state_in`) | Tout passage à `queued` remet à zéro les échecs (`retry_count`) et les tours des morceaux faux, y compris les remises en file automatiques (arrêt du moteur, plage, `disk_full`, reprise après plantage). Les limites de 10 échecs (SPEC 7.8) et de 3 tours (SPEC 7.9) se contournent à chaque redémarrage. | Remise à zéro seulement sur une relance de l'utilisateur (champ `manual` dans `Change`). | Tour 2 des morceaux faux, pause de plage puis file : tours gardés ; pause et reprise de l'utilisateur : remis à zéro. | oui |
| **V1** | `torrent.rs:1080`, `894` | La fin d'un torrent scelle les fichiers choisis relus en base, pas ceux que librqbit a vraiment téléchargés ; si `update_only_files` échoue, le nouveau choix n'est jamais retenté. Un fichier coché au moment où le torrent finit prend son nom final incomplet, et la tâche finit `completed`. | `follow` rend le choix appliqué ; à la fin, si le choix en base a changé, le téléchargement reprend ; pas de mise à jour de `selected` en cas d'échec. | `set_selected` juste après `finished` ; aucun fichier incomplet ne prend son nom. `crash_torrent`. | oui (code) |
| **V2** | `torrent.rs:1080`, `1091`, `1107` | Une erreur de la base dans la boucle de `Pass::follow` sort par `?` sans retirer le torrent de la session ni attendre la fermeture de ses fichiers : torrent orphelin qui écrit encore dans les `.part`, relance refusée (`AlreadyManaged`), annulation sur des fichiers ouverts. | Toujours faire le retrait et `wait_closed`, puis propager l'erreur. | Erreur de base injectée pendant un passage, deux torrents dans la session. `crash_torrent`. | oui |
| **V4** | `media/run.rs:850`, `955`, `1030`, `1195` | Les noms des flux fragmentés (`<nom>.f<id>.<ext>`) ne sont jamais réservés ; un fichier déjà présent sous ce nom est pris pour le flux fait, fusionné puis effacé, ou effacé à l'annulation. | Réserver chaque nom de flux par `claims` ; n'effacer que les noms enregistrés comme pris. | Fichier préexistant au nom du flux, vidéo fragmentée puis annulation : le fichier survit. `crash_media`. | oui |

### Moteur et clients

| Id | Lieu | Défaut et scénario | Correctif proposé | Test | Vérifié |
| --- | --- | --- | --- | --- | --- |
| **R2 = C2** | `crates/daemon/src/main.rs:158`, `scheduler.rs:895` | `aurod` démarre le moteur (récupération des noms et des déplacements, reprise des tâches, ramassage des secrets) avant d'ouvrir le point d'accès, seul garde contre un second moteur ; la base n'a pas de verrou. Deux clients qui lancent `aurod` ensemble, ou un client qui échoue pour une raison passagère, font tourner deux moteurs quelques instants sur la même base : deux écrivains sur le même `.part`, secret effacé. Sous Unix, course `remove_file` puis `bind` sur un socket périmé. | Verrou exclusif sur un fichier à côté de la base (`auro.db.lock`) ou point d'accès réservé, avant `Engine::start`. | Deux `aurod` lancés ensemble sur la même base : le second sort sans rien toucher. `crash_daemon`. | oui |
| **R3 = C1** | `paths.rs:33`, `daemon/src/main.rs:78`, `cli/src/client.rs:151`, `cli/src/relay.rs:116` | Le garde « point d'accès d'essai sans `--db` refusé » (incident du 2026-11-08) compare à `paths::endpoint()`, qui rend `AURO_ENDPOINT`. Un navigateur d'essai lancé avec cette variable (procédure de `CLAUDE.md`) fait démarrer par le relais un `aurod` d'essai… sur la base réelle. | `paths::default_endpoint()` sans la variable pour le garde ; avec `AURO_ENDPOINT`, refuser le démarrage sans base d'essai. | `check_test_setup` et le garde du client avec la variable posée. | oui |
| **P2** | `scheduler/planning.rs:93` | La fermeture d'une plage arrête toute exécution de la file, partages compris ; le torrent reste `seeding` et repart au tour suivant (le partage ne suit pas les plages). Arrêt et relance toutes les 15 s, avec revérification complète des morceaux : disque lu en boucle toute la nuit. | Une seule aide « remettre en file » qui épargne `seeding` et `post_processing` (voir P23), utilisée partout. | Torrent en partage, plage qui ferme : aucun redémarrage sur plusieurs contrôles. `crash_torrent`. | oui |
| **P3** | `scheduler/planning.rs:93`, `scheduler.rs:4623`, `4476` | La commande de fin d'une règle (`post_processing`) est tuée par une fermeture de plage, un changement d'interface ou un ajout de miroir, puis relancée depuis le début ; un script qui déplace le fichier ne le retrouve plus (`POSTPROCESS_FAILED`). SPEC 5.5 : « la commande va jusqu'au bout ». | Même aide que P2. | Commande `sleep 2`, plage fermée pendant la commande : une seule exécution, `completed`. `crash_daemon`. | oui |
| **P4** | `scheduler/planning.rs:205` | Un torrent en partage compte comme du travail de la file : l'action de fin (veille, extinction) ne part jamais tant qu'il partage. SPEC 7.11 ne compte pas les partages. | Ne pas compter `seeding` dans `has_work`. | `onDone: shutdown`, torrent qui passe au partage : `engine.endAction` annoncé. | oui (code ; intention à confirmer, voir section 5) |
| **T2** | `transfer.rs:1954`, `1653`, `1887` | Une source lente compte comme relais possible, mais ne reçoit aucun segment. Référence écartée et seul miroir lent : la boucle s'arrête avec des segments à faire, la tâche échoue `INTERNAL` au lieu de finir sur le miroir (SPEC 7.7). | Une seule définition de « source qui peut travailler » ; sans source `Usable`, les lentes redeviennent utilisables. | `a_slow_mirror_gets_no_new_segments` puis fichier de la référence changé : la tâche finit. `crash_resume`. | oui |
| **T3** | `transfer.rs:1965`, `1993` | Un refus `429`/`503` d'un miroir abaisse le nombre de connexions de toutes les sources et plafonne l'hôte de la référence, pour toute la session. | Plafond sur l'hôte de la source fautive, et par source. | Miroir à 2 connexions : aucun plafond sur l'hôte de la référence. | oui |
| **T4** | `transfer.rs:1969` | Avec un nombre de connexions fixé, un seul refus `503` arrête toute l'exécution ; un serveur qui accepte 8 connexions pour 16 demandées ne fait jamais avancer la tâche, qui échoue après 10 essais. | Remettre le segment en attente sans changer le nombre de connexions (ou décider de la règle, section 5). | Serveur à 8 connexions, tâche à 16 : elle finit. | oui |
| **F1** | `scheduler.rs:3394`, `net/ftp.rs:109`, `net/sftp.rs:291` | Une adresse au nom seul (`sftp://moi@hôte/f`) range un en-tête `Authorization` au mot de passe vide : la clé privée ou le mot de passe du site sont ignorés, la tâche échoue `AUTH_FAILED`. Même chose en FTP. Aucun test ne passe par ce chemin. | Ne ranger l'en-tête que si l'adresse porte un mot de passe. | Identifiant de site à clé, ajout de `sftp://moi@…` : la tâche finit. | oui |
| **F2** | `net/sftp.rs:607`, `246` | Une session SFTP abandonnée après un délai dépassé revient à la réserve sans vérification : le segment suivant attend encore tout le délai, ce qui consomme les nouvelles tentatives. | Pas de retour en réserve après une erreur (seulement par `finish`). | Lecture gelée, seconde plage : nouvelle connexion. | oui |
| **V5** | `media/run.rs:594` | Un sous-titre en échec définitif autre que les refus prévus (`ADDRESS_FORBIDDEN`, `TLS_*`, `RANGE_UNSUPPORTED`…) met toute la vidéo en échec alors que ses flux sont complets ; chaque essai rejoue l'échec. | Tout échec définitif d'un sous-titre fait un sous-titre manquant (`SUBTITLES_MISSING`). | Sous-titre en `RANGE_UNSUPPORTED` : vidéo terminée, sous-titre signalé manquant. `crash_media`. | oui |
| **D3** | `scheduler.rs:2156`, `4512` | Le renouvellement d'un lien de plugin range les cookies du lien neuf, puis `set_url` les efface quand l'hôte change (CDN). Le téléchargement reçoit `403` et le renouvellement échoue toujours. | Une seule commande qui remplace adresse et en-têtes, sans effacer les secrets propres au lien neuf. | Lien neuf sur un autre hôte qui exige son cookie : fichier complet. | oui |
| **C4** | `apps/desktop/windows/hooks.nsh:84` | « Supprimer les données » n'efface que la base et `aurod.log` : restent `logs/`, `plugins/`, `tools/` (environ 400 Mo), la clé privée TLS de l'accès à distance, et les secrets du coffre Windows. SPEC 4.6. | Effacer ces dossiers et fichiers ; secrets du coffre effacés par une commande d'`aurod` sur son espace de noms. | Essai d'installateur (`bundle.ps1 -Essai`) avec relevé du dossier et du Gestionnaire d'identification. | oui |

### Sécurité

| Id | Lieu | Défaut et scénario | Correctif proposé | Test | Vérifié |
| --- | --- | --- | --- | --- | --- |
| **D1** | `scheduler.rs:1505`, `4139`, `4243`, `1646` | Une connexion à jeton n'est limitée au dossier des téléchargements qu'à l'ajout et au déplacement vers un autre dossier. Sur une tâche locale existante, elle peut changer l'adresse, retélécharger (`task.restart`) ou renommer sur place : contenu de l'attaquant écrit dans un dossier quelconque (`Outils\outil.exe`). Contraire au modèle de menace C (`docs/SECURITE.md`). | Pour une poignée restreinte, `check_within_downloads(&task.dest_dir)` dans `set_url`, `restart`, `relocate` (renommage compris) et `add_source`. | Jeton `control` sur une tâche hors du dossier : `DEST_FORBIDDEN` pour chacune. | oui |
| **R1** | `rpc/methods.rs:684`, `rpc/session.rs:510` | `settings.get` cache les réglages `remote.*` à une connexion à jeton, mais `sync.snapshot` et les notifications `settings.updated` les donnent : chemins du certificat et de la clé (nom de l'utilisateur), noms, adresse d'écoute. | Filtrer aussi l'instantané et les notifications pour une poignée restreinte. | Jeton abonné, `remote.tlsCert` changé sur le canal local : rien de `remote.*` reçu. | oui |
| **D2** | `rpc/session.rs:176`, `remote/server.rs:448` | Un jeton révoqué entre la vérification d'un WebSocket et le début de son abonnement garde sa connexion ouverte pour toujours (l'événement de révocation est perdu). | S'abonner d'abord, puis revérifier le jeton. | `serve_messages` avec un jeton déjà révoqué : fin immédiate. | non (course lue dans le code) |
| **V3** | `media/extract.rs:229`, `media/run.rs:205`, `332`, `856` | L'extension rendue par yt-dlp entre sans nettoyage dans le nom final, les noms des flux et le modèle `-o` (contraire à « noms venus du serveur : toujours par `sanitize` ») ; `"ext": "mp4/../../x"` sortirait du dossier. Le yt-dlp officiel valide les extensions depuis 2024.07.01 (CVE-2024-38519) ; défense en profondeur. | Extension acceptée seulement de 1 à 10 caractères alphanumériques ASCII ; noms passés par `sanitize`. | `parse_resolution` avec une `ext` hostile ; faux yt-dlp. | oui |
| **F4** | `net/ftp.rs:175` | Un compte FTP nommé `ftp` (ou `anonymous`) avec un vrai mot de passe est pris pour un accès anonyme : le mot de passe part en clair sans « autoriser sans chiffrement ». | Anonyme seulement sans identifiant, ou nom anonyme avec mot de passe vide ou de forme adresse. | `("ftp", "vrai")` non anonyme. | oui |
| **C3** | fin d'écriture du moteur (aucune occurrence) | Aucun fichier reçu ne porte la marque du Web sous Windows (`Zone.Identifier`) : ni SmartScreen ni le Mode protégé d'Office ne s'appliquent, contrairement au même fichier pris par le navigateur. | Écrire `Zone.Identifier` (ZoneId=3) à la fin, sous Windows ; décision à prendre (section 5). | Test Windows de fin de tâche qui lit le flux. | oui (absence) |
| **W1** | `extension/content.js:49` | Le clic sur un lien magnet est capté sans `event.isTrusted` : une page (même une iframe publicitaire) fait `a.click()` sur un lien magnet et le torrent démarre sans action de l'utilisateur, puis partage. | `if (!event.isTrusted) return;` (aussi pour l'Alt+clic, W10). | Test jsdom du script de contenu : un clic synthétique n'envoie rien. | oui |
| **W3** | `.github/workflows/release.yml:57`, `180` | `SIGNPATH_API_TOKEN` est dans l'environnement de tout le job Windows (build.rs des dépendances, actions tierces) ; sous Android, la clé et ses mots de passe restent pendant tout `assembleRelease`. | Secrets seulement aux étapes qui signent ; APK construit non signé, puis signé par `apksigner` dans une étape à part. | Relecture, actionlint. | oui (lecture) |
| **W4** | `release.yml`, `ci.yml` | Actions prises par étiquette mobile (`dtolnay/rust-toolchain@stable` est une branche), `tauri-cli "^2"`, cache restauré dans les constructions de publication. | Actions épinglées par SHA (Dependabot), version exacte de tauri-cli, pas de cache dans `release.yml`. | Relecture. | oui (lecture) |

### Interface

| Id | Lieu | Défaut et scénario | Correctif proposé | Test | Vérifié |
| --- | --- | --- | --- | --- | --- |
| **W2** | `ui/src/web.js:107` | La page distante s'abonne toujours aux événements, ce qu'un jeton `add` n'a pas le droit de faire : refus, connexion fermée, reconnexion sans fin, « Moteur injoignable ». SPEC 7.19 dit le jeton `add` essayé et validé : écart à expliquer. | Ne s'abonner que si la portée rendue par `engine.hello` le permet. | Test de `web.js` (aucun n'existe) avec un faux WebSocket. | oui |
| **W5** | `ui/src/main.js:4786`, `index.html:1097` | La boîte « Connexion à AuroDownloader » de la page distante se ferme à l'envoi puis se rouvre avec l'erreur, ce que la leçon A15 de `docs/ACCESSIBILITE.md` interdit ; l'erreur n'est pas liée au champ. | `preventDefault`, connexion la boîte ouverte, `fieldError` avant le focus. | `live.test.js` avec un faux hôte distant. | oui (lecture) |

## 3. Mineures

Reprises des relectures ; chacune sera revérifiée au moment de son correctif.

### Stockage et reprise

| Id | Lieu | Défaut | Correctif proposé |
| --- | --- | --- | --- |
| S3 | `transfer.rs:683`, `store/claims.rs:76` | Échec de l'enregistrement après une réservation : `.part` vide orphelin, nom suffixé ; `Content-Length` supérieur à `i64::MAX` accepté au sondage. | Défaire la réservation en cas d'échec ; refuser cette taille au sondage. |
| S5 | `store/tasks.rs:1117` | Historique jugé écrit s'il existe une ligne quelconque : un retéléchargement annulé n'y figure pas. | Comparer avec la dernière ligne. |
| S6 | `io.rs:107` | Casse ignorée seulement sous Windows ; Android aussi (stockage partagé). | `cfg!(any(windows, target_os = "android"))` en un seul endroit. |
| S7 | `store/tasks.rs:1083` | Retéléchargement sans règle applicable : dossier de l'ancienne règle au lieu de la base. | `COALESCE(rule_base, dest_dir)` (lecture de la SPEC 7.21 à confirmer). |
| S8 | `store/segments.rs:383` | `find_coverage_errors` n'est jamais appelé au démarrage, contrairement à la SPEC 5.6 et à `CLAUDE.md` (contrôle tâche par tâche à la reprise seulement). | À trancher : l'appeler au démarrage, ou corriger les documents (section 5). |

### Transfert et réseau

| Id | Lieu | Défaut | Correctif proposé |
| --- | --- | --- | --- |
| T5 | `transfer.rs:390` | `Retry-After` sans plafond en `retry_wait` (un an possible). | Plafond, noté en SPEC 7.8. |
| T6 = D10 | `mirrors.rs:166`, `scheduler.rs:4374`, `4428` | « Même hôte » sans protocole ni port : cookies et `Authorization` envoyés en clair vers `http://` du même hôte. | Comparer l'origine ; jamais un secret de https vers http. |
| T7 | `net.rs:1051` | Après redirection vers un autre hôte qui répond `401`, les en-têtes de la tâche (`Cookie`) repartent vers lui. | Retirer les secrets quand l'hôte diffère. |
| T8 | `transfer.rs:656` | Hôte des règles lu par `parse_url` (HTTP seulement) : critère « domaine » jamais vrai en FTP et SFTP. | `host_of`. |
| T9 | `transfer.rs:572` | Champ `host` d'un journal qui reçoit le texte du site (protocole, port, chemin). | Hôte seul. |
| T10 | `net.rs:789` | `416 bytes */N` pris pour un fichier de N octets quel que soit N. | Seulement N = 0. |
| T11 | `net.rs:1175` | Passage de HTTPS à HTTP signalé seulement par la CLI ; SPEC 8.1 demande de le signaler (section 5). | Enregistrer ou annoncer, ou corriger la SPEC. |
| T12 | `net.rs:747` | Erreur de transport du `HEAD` sans repli `GET bytes=0-0` (SPEC 7.1). | Repli sur toute erreur de transport. |
| T13 | `mirrors.rs:150` | `206` plus court que demandé refusé à la validation d'un miroir, qui est écarté pour toujours. | Redemander la suite. |
| T14 | `transfer.rs:632`, `net/sums.rs:130` | Empreinte voisine cherchée en FTP et SFTP par reqwest, qui refuse ces protocoles. | Limiter à HTTP(S), ou passer par `get_all_as`. |
| F3 | `net/dial.rs:211`, `net/socks.rs:60` | Erreurs du SOCKS5 classées à l'envers : proxy injoignable non relancé, identifiant refusé relancé sans fin. | Inverser ; vérifier `status[0] == 1` (RFC 1929). |
| F5 | `net/ftp.rs:581`, `net/dial.rs:183` | Connexion de données FTP dans une tâche détachée, `dial` sans délai : un proxy muet garde socket et tâche à chaque essai. | Délai dans `dial`, tâche annulée avec son handle. |
| F6 | `net/sftp.rs:151` | Clé SSH en attente jamais retirée quand la bonne clé revient ; table non bornée. | Retirer sur `Trusted` et `forget` ; borne. |
| F7 | `net/ftp.rs:504`, `net/dial.rs:205` | FTPS implicite et SOCKS en résolution locale : première adresse seulement. | Parcourir les adresses. |
| F8 | `net/ftp.rs:373` | Connexions FTP au repos jamais fermées activement. | Purge à 30 s. |
| F10 | `net/ftp.rs:762` | Repli `EPSV` vers `PASV` sur tout `500`/`501`/`502`. | Seulement si l'échec vient d'`EPSV`. |

### Planificateur

| Id | Lieu | Défaut | Correctif proposé |
| --- | --- | --- | --- |
| P5 | `scheduler/planning.rs:198` | Action de fin armée qui ne se désarme jamais (pause des tâches restantes le lendemain : extinction). | À décider (section 5). |
| P6 | `scheduler/planning.rs:226`, `power.rs:45` | Veille et extinction lancées de façon bloquante dans la boucle. | `spawn_blocking`, annonce avant. |
| P7 | `scheduler.rs:2529` | Annonces d'exécution sans numéro : une note de l'exécution 1 peut s'appliquer à la 2 ; pause perdue au passage en partage. | Numéroter les exécutions. |
| P8 | `scheduler.rs:3955` | Une pause efface une annulation en cours (`.part` gardé). | Ne pas rétrograder `Cancel`. |
| P9 | `scheduler.rs:3233`, `postproc.rs:99` | `{sha256}` d'une empreinte changée après le renommage, jamais vérifiée. | Refuser le changement, ou recalculer. |
| P10 | `transfer.rs:432` | Pause pour réseau, plage ou arrêt enregistrée cause `user` (SPEC 7.15 : `network`). | Cause passée au `stop`. |
| P11 | `scheduler.rs:4883` | `Paused → Queued` qui échoue sans journal : tâche en pause pour toujours. | Journal et nouvel essai. |
| P12 | `scheduler.rs:2691`, `4935` | Tâches écartées (`broken`) sans événement ; `seeding` vers `failed` hors table. | Avertissement ; `finish_sharing`. |
| P13 | `scheduler.rs:4889` | Annulation perdue si l'exécution échoue en même temps. | Annuler depuis `failed` aussi. |
| P14 | `scheduler.rs:3346` | `rule_base` et pause écrits hors de la transaction de création. | Dans `create_task*`. |
| P15 | `scheduler.rs:4016` | `accept_host_key` s'arrête à la première tâche en erreur. | Continuer et journaliser. |
| P16 | `scheduler.rs:4512` | `replace_url` efface les cookies avant d'enregistrer l'adresse. | Après une écriture réussie. |
| P17 | `scheduler.rs:4345` | `moves::commit` en échec : journal oublié, copie orpheline. | Garder le journal pour `recover`. |
| P18 | `rules.rs:499`, `postproc.rs:136` | `.bat`/`.cmd` lancés par `cmd.exe` (un shell), malgré la règle (section 5). | Refuser à la validation, ou documenter. |
| P19 | `schedule.rs:131` | `nextChangeAt` faux d'une heure autour d'un changement d'heure. | Décalage recalculé au point trouvé. |
| P20 | `scheduler.rs:4180` | Commandes par identifiant qui acceptent une tâche interne de vidéo. | `load_top_task`. |
| P21 | `scheduler.rs:4476` | Ajout d'un miroir qui relance une vérification ou une commande de fin. | Ne relancer qu'une exécution `active`. |

### Protocole, coffre, journaux

| Id | Lieu | Défaut | Correctif proposé |
| --- | --- | --- | --- |
| R4 | `rpc/session.rs:389` | Dernière valeur d'avancement perdue à un changement d'état. | L'envoyer avant `task.stateChanged`. |
| R5 | `rpc/methods.rs:197`, `rpc/scope.rs:58` | Méthode sans portée permise à tous ; garde textuel contournable. | Refuser toute méthode sans portée (sauf `engine.hello`). |
| R6 | `rpc/methods.rs:620` | `settings.set` appliqué à moitié si `downloads.dir` est refusé. | Vérifier avant tout changement. |
| R7 | `media/run.rs:677`, `scheduler.rs:962` | Ramassage des secrets qui peut effacer un secret posé par une vidéo reprise. | Ignorer les identifiants postérieurs au démarrage (UUID v7). |
| R8 | `rpc/session.rs:301` | Requête invalide sans `id` : aucune réponse (JSON-RPC 2.0 demande `-32600`, `id: null`). | Répondre toujours. |
| R9 | `rpc/methods.rs:892` | Texte d'erreur des outils dans le diagnostic sans `scrub_text`. | Le nettoyer. |
| R10 | `logs.rs:378`, `450` | Chemins UNC et `\\?\`, `socks5://`, `ws://`, `file://` non nettoyés. | Les ajouter. |
| R11 | `rpc/session.rs:264` | UTF-8 invalide remplacé en silence. | `-32700`. |
| R13 | `rpc/session.rs:199` | Révocation d'un jeton non vue pendant un appel long. | `select!` avec la révocation. |
| R14 | `rpc/methods.rs:333` | `queue.create` avec planification en deux commandes. | Une seule. |
| R15 | `scheduler.rs:4777` | Proxy changé dans le client avant la base ; coffre bloquant dans la boucle. | Base d'abord. |
| R16 | `logs.rs:147` | Rotation en échec : tous les événements suivants perdus. | Écrire quand même, retenter plus tard. |
| R17 | `rpc/methods.rs:1821` | `task.list` accepte `limit: 0`. | Le refuser comme `history.list`. |
| R18 | `rpc/scope.rs` | `engine.stats`, `task.move`, `queue.delete` décrits (SPEC 6.4) mais non servis ; SPEC 6.9 incomplète (section 5). | Mettre la SPEC à jour. |

### Torrent et vidéo

| Id | Lieu | Défaut | Correctif proposé |
| --- | --- | --- | --- |
| V6 | `third_party/librqbit/src/session.rs:716` | Client reqwest de librqbit sans `.no_proxy()` : annonces aux traqueurs par le proxy du système. | `.no_proxy()` (copie corrigée, `third_party/README.md`). |
| V7 | `torrent.rs:1140` | `wait_closed` dépassé : renommage ou effacement quand même. | Arrêter sans toucher aux fichiers. |
| V8 | `media/run.rs:191` | Exécution d'une vidéo sans le verrou de yt-dlp : mise à jour pendant une vidéo. | Garder le verrou. |
| V9 | `scheduler.rs:4866`, `store/media.rs:302` | Cookies des tâches internes gardés au coffre après la fin. | Les effacer à la fin. |
| V10 | `media/tools.rs:405` | Mise à jour de yt-dlp vers une version plus ancienne acceptée (section 5). | Refuser une étiquette inférieure. |
| V11 | `media/extract.rs:79` | Route directe : `HTTP_PROXY` hérité suivi par ffmpeg sous yt-dlp. | `environment()` toujours appliqué. |
| V12 | `media/extract.rs:135` | Sortie de yt-dlp bornée après lecture complète ; stderr sans borne. | `take()`. |
| V13 | `media/run.rs:1190` | `.ytdl` et `.part-FragN` laissés à l'annulation (à confirmer). | Les effacer. |
| V14 | `media/run.rs:106` | Fusion relancée sans contrôle de durée. | Garder la durée en base. |
| V15 | `media/run.rs:271` | Page relue même quand tout est fait : vidéo complète en `EXTRACTOR_FAILED`. | Sauter la résolution. |
| V16 | `torrent.rs:1303` | Fichier vide créé pour un `.part` effacé : `completed` avec un fichier vide. | Seulement si la taille attendue est 0. |
| V17 | `torrent/storage.rs:214` | Au partage, un fichier final effacé recrée un `.part` qui reste. | Stockage en lecture seule au partage. |
| V18 | `media/process.rs:46` | Petit-processus créé avant l'entrée dans le Job Object. | Lancer suspendu. |
| V19 | `media/process.rs:116` | `kill(-pgid)` après `wait` : numéro de groupe réutilisable (à confirmer). | Tuer avant le `wait`. |
| V20 | `torrent.rs:456` | Collisions de noms comparées en minuscules, pas par la table de NTFS (à confirmer). | Comparaison plus stricte. |
| V21 | `media/extract.rs:428` | Flux « direct » sans contrôle du schéma `http(s)`. | L'exiger. |

### Interface distante et plugins

| Id | Lieu | Défaut | Correctif proposé |
| --- | --- | --- | --- |
| D4 | `remote/server.rs:186`, `482` | Limite de 64 connexions sans effet sur les WebSocket (place rendue à la mise à niveau). | Place gardée par la tâche du WebSocket. |
| D5 | `plugins.rs:216` | Plugin abîmé jamais réécrit ; `.tmp` partagé entre deux installations. | Revérifier, nom provisoire unique. |
| D6 | `remote/server.rs:364` | Table des échecs par adresse exacte, sans borne (IPv6 /64). | Regrouper par /64, borne. |
| D7 | `remote/server.rs:517` | Écriture WebSocket sans délai : client qui ne lit pas garde tout ouvert. | Délai, `abort`. |
| D8 | `plugins/runtime.rs:47` | wasmi sans `enforced_limits` à la lecture du module (à confirmer). | `EnforcedLimits::strict()`. |
| D9 | `net/reach.rs:46` | `64:ff9b:1::/48` jugé comme un /96. | Refuser tout ce préfixe. |
| D11 | `plugins/fetch.rs:64` | Cookies du plugin sans `Secure` ni `Expires`. | Les gérer. |
| D12 | `plugins/fetch.rs:302` | Réponse de 8 Mio jusqu'à environ 48 Mio une fois échappée. | Borner la réponse finale. |
| D13 | `remote/server.rs:649` | Cookie de session sur la boucle locale en clair, envoyé à tout port (à confirmer). | Durée plus courte, ou documenter. |
| D14 | `plugins.rs:239` | Lecture et empreinte du plugin sur l'exécuteur. | `spawn_blocking`. |
| D15 | `docs/SECURITE.md` | « Deux icônes fixes par `innerHTML` » : le guide aussi depuis l'étape 9d. | Mettre la revue à jour. |

### Clients

| Id | Lieu | Défaut | Correctif proposé |
| --- | --- | --- | --- |
| C5 | `apps/desktop/src/files.rs:7` | Liste des extensions jamais ouvertes incomplète (`py`, `chm`, `msc`, `iso`, `vhd`…). | La compléter. |
| C6 | `apps/desktop/src/update.rs:215` | Mise à jour AppImage sans arrêt d'`aurod` (SPEC 4.6). | `engine.shutdown` avant. |
| C7 | `crates/cli/src/main.rs:1413` | `engine start` d'essai sans `--db` : 10 s d'attente et un message faux. | Refuser tout de suite. |
| C8 | `apps/desktop/src/bridge.rs:123` | `aurod` relancé sans limite ; `aurod.log` sans rotation. | Relancer seulement sur « absent », limiter. |
| C9 | `crates/cli/src/remote.rs:546` | `auro watch <id inconnu>` ne rend jamais la main. | Refuser les identifiants absents. |
| C10 | `apps/desktop/src/main.rs:176`, `prefs.rs:60` | Deux écritures concurrentes de `prefs.json.tmp` ; pas de `sync_all`. | Verrou tenu, `sync_all`. |
| C11 | `crates/cli/src/remote.rs:296` | `auro add ftp://moi:secret@…` affiche le mot de passe. | Retirer les identifiants avant l'affichage. |
| C12 | `crates/cli/src/remote.rs:139` | Codes de sortie incohérents (Metalink en partie refusé : 0 ; Ctrl-C : 0). | Une règle unique, 130 à l'interruption. |
| C13 | `crates/cli/src/client.rs:95`, `relay.rs:40` | Erreur à `id` nul ignorée ; `MAX_INCOMING` = `MAX_MESSAGE` sans marge. | Refus traité ; marge. |
| C14 | `apps/desktop/src/main.rs:333` | La page peut faire lire n'importe quel fichier absolu (défense en profondeur). | N'accepter que les chemins choisis ou reçus. |
| C15 | `apps/android/…/MainActivity.kt:60` | Crochet d'essai `--es url` dans l'APK publié : toute application peut ajouter un téléchargement. | Seulement en `BuildConfig.DEBUG`. |
| C16 | `apps/android/…/AuroApp.kt:89` | `.torrent` lu par l'application hors du moteur (ni proxy ni Wi-Fi seulement). | Lu par le moteur. |
| C17 | `apps/android/…/Background.kt:86` | « Wi-Fi seulement » appliqué comme « non facturé » (section 5). | Exiger le Wi-Fi, ou renommer. |
| C18 | `apps/android/…/Downloads.kt:260` | Trois notifications numérotées non comptées : instantané rechargé. | Les ajouter. |
| C19 | `apps/desktop/src/clipboard.rs:248` | Un fil par notification sous Linux (à confirmer). | Un seul fil. |
| C22 | `apps/desktop/src/update.rs:225` | Après une installation ratée, « Installer » ne retrouve plus la version. | Garder la mise à jour en attente. |
| C23 | `tauri.essai.json`, `main.rs:616` | Variante d'essai lancée sans options : point d'accès et base réels. | Exiger les options d'essai. |
| C25 | `apps/android/…/AuroApp.kt:272` | Chemin de chaque fichier indexé dans logcat. | Identifiant seulement. |

### Page, extension, workflows

| Id | Lieu | Défaut | Correctif proposé |
| --- | --- | --- | --- |
| W6 | `ui/src/i18n.js:801` | `TORRENT_FAILED`, `TORRENT_SESSION`, `ADDRESS_FORBIDDEN` sans traduction. | Les ajouter ; test face à la SPEC 6.8. |
| W7 | `ui/src/main.js:838` | « Supprimer » une tâche en `post_processing` ne fait rien, sans message. | Désactiver ou annoncer. |
| W8 | `extension/background.js:174` | Entrée du navigateur effacée même si l'annulation a échoué : double téléchargement. | Effacer après une annulation réussie. |
| W9 | `extension/content.js` | Liens magnet d'une fenêtre privée confiés quel que soit `captureIncognito` (section 5). | Selon la décision. |
| W10 | `extension/content.js:18` | Alt+clic synthétique envoyé. | `isTrusted`. |
| W11 | `ui/src/main.js:2311` | Moteur perdu : « Démarrer » ferme la boîte sans message. | `addFailed`, boîte ouverte. |
| W12 | `ui/src/web.js:225` | Langue lue dans `localStorage`, ignorée s'il est refusé. | Depuis les préférences reçues. |
| W13 | `ui/src/model.js:382` | `task.progress` en avance non pris pour un trou (à confirmer). | `resync`. |
| W14 | `ui/src/main.js:2725` et autres | Erreurs de quatre champs non liées au champ (leçon A15). | `fieldError`. |
| W15 | `ui/src/main.js:4464` | `login.list` et `torrent.interfaces` demandés sans droit d'administration. | Garde `caps.admin`. |
| W17 | `release.yml:218` | `persist-credentials` laissé dans le job qui écrit. | `false`. |

## 4. Nettoyage et performance

### Code mort (vérifié par Grep sur tout le dépôt)

| Id | Lieu | Ce qui est mort |
| --- | --- | --- |
| S9 | migrations 1 et suivantes | `SegmentState::Failed`, `segments.attempts`, `segments.hash`, `tasks.options_json`, table `event_log` jamais écrits ; `started_at` jamais lus ; index `idx_segments_state` inutile (coûte une écriture par `save_progress`). Les colonnes restent (une migration publiée ne change pas) : les marquer « réservées » en SPEC 5, retirer l'index par une migration. |
| S10 | `store/segments.rs:133`, `store/tasks.rs:675`, `io.rs:685`, `store/sources.rs:208`, `pieces.rs:65`, `io.rs:39` | Fonctions servies seulement aux tests. |
| T19 | `net.rs:825`, `409`, `net/headers.rs:84`, `limit.rs:58` | `Client::get_range`, `Probe::strong_etag`, `is_strong_etag`, `Bucket::rate` : tests seulement. |
| F11 | `net/sftp.rs:37` | `sftp::MAX_AUTO_CONNECTIONS` jamais utilisé. |
| V22 | `store/torrents.rs:101` | `ratio_limit` et `seed_time_limit_s` par tâche jamais écrits, alors que la SPEC 7.16 décrit le premier (section 5). |
| V23 | `store/media.rs:272` | Paramètre `total_bytes` toujours `None`. |
| V24 | `media/tools.rs:156` | Empreinte des outils enregistrée, jamais relue (ou la revérifier au lancement). |
| P22 | `scheduler.rs:1342`, `rules.rs:267`, `store/rules.rs:88` | `Engine::remote_listen` (tests seulement) ; deux `pub` à rendre privés. |
| D16 | `plugins/runtime.rs:201` | `Run::fuel` jamais lu. |
| C26 | `crates/cli/src/relay.rs:35` | `relay::HOST_NAME` jamais utilisé. |
| C27 | `EngineClient.kt:42`, `NativeEngine.kt:31`, `jni_glue.rs:83` | `close`, `forget`, `stop` jamais appelés par l'application. |
| W18 | `extension/content.js:64` | Champ `page` du message magnet jamais lu (et contraire au commentaire). |
| W23 | `ui/src/index.html` | Sept identifiants jamais référencés. |

### Redondances et simplifications

| Id | Où | Proposition |
| --- | --- | --- |
| P23, P24, P25, P26 | `scheduler.rs`, `planning.rs` | Le motif « remettre en file et arrêter » recopié 5 fois avec des exclusions différentes (cause de P2, P3, P21) : une seule aide. `noted()`, `delete_part`/`delete_final`, branches d'annulation, fins d'ajout : aides communes. |
| F12 à F16 | `net/ftp.rs`, `net/sftp.rs` | `within`, `Context`, réserve de connexions (`Pool<T>`), décodage des identifiants (3 fois), normalisation de l'hôte (4 fois), fin de `probe` : en commun dans `net`. |
| T20, T21, T22, T23 | `transfer.rs`, `scheduler.rs`, `mirrors.rs` | Lecture des secrets recopiée ; choix des identifiants d'un miroir réécrit (cause de T6) ; `host_of` et `jitter` dans `net` ; intervalles fusionnés, `Options` recopiées toutes les 2 s. |
| S11 à S16 | `store` | Conversions `u64`/`i64` en quatre formes ; adresse de la référence (4 requêtes) ; boucles de lecture des empreintes ; `secret_refs` recopié ; `complete_task` ; chaîne `create_task*`. |
| R19 à R22 | `rpc/methods.rs`, `logs.rs` | Décodages de paramètres recopiés ; aides existantes ignorées ; branche inatteignable ; `MAX_MESSAGE` décalé de 1. |
| V25, V26, V27 | `torrent.rs`, `media/run.rs` | Fin de partage qui refait `postproc::complete` (et appel direct de `complete_task`, contraire à la lettre de la règle) ; `stopping`, `wait_stop` et nouvel essai recopiés trois fois (sans l'aléa de `transfer`) ; ratio déjà atteint. |
| D17 à D20 | `remote/server.rs`, `remote/tls.rs` | Connexion recopiée, transmission inutile, filtre en double, schéma en chaîne. |
| C28 à C31 | clients | `PAGE_EXTENSIONS` recopié trois fois sans test ; limites de taille recopiées ; boucle « connecter ou lancer » en double (à mettre dans `launch`, avec le garde de R3) ; lecture bornée en trois copies. |
| C24 | `crates/daemon/src/main.rs:186` | `eprintln!` hors erreurs de démarrage (règle des journaux). |
| W19 à W22, W24 | page, extension, scripts | `joinListen` ignoré ; sept enregistrements de réglage au même schéma ; trois listes à cocher ; `languageName` et `formatSize` recopiés sans test ; détection du lancement direct. |

### Performance (à mesurer avant tout changement du chemin chaud)

| Id | Où | Coût | Proposition |
| --- | --- | --- | --- |
| P27 | boucle du planificateur | Requêtes à chaque tour, réveils toutes les 15 s et 30 s même sans planification ni `disk_full` (Android). | Travail seulement après un changement ou une échéance. |
| P28 | `rules.rs:250` | Expressions régulières recompilées à chaque comparaison. | Compilées une fois. |
| R12 | `rpc/session.rs:375` | Un `engine.get` par `task.added` et par abonné (1 000 × N pour un lot). | La tâche dans l'événement. |
| T15 | `transfer.rs:2045` | Boucle de coordination bloquée pendant le vidage d'un point de contrôle. | Vidage en futur à part ; gain à mesurer au banc. |
| T16, T17, T18 | `transfer.rs`, `io.rs` | Écriture synchrone sur un fil de tokio ; trois prises de verrou et une copie par morceau ; une transaction par départ de connexion. | À mesurer face à aria2 avant d'y toucher (N-01). |
| F9, F17 | `net/sftp.rs` | Lecture d'avance de 4 Mio au-delà de chaque plage ; tampon mis à zéro par morceau. | Lectures bornées, `read_buf`. |
| V28, V29 | `torrent.rs`, `media/tools.rs` | Un `UPDATE` par fichier toutes les 5 s ; `ffmpeg -version` bloquant. | Écrire ce qui change ; `spawn_blocking`. |
| S17 | `claims.rs:76`, table `history` | Noms relus jusqu'à 1 000 fois ; pas d'index `history(task_id)`. | Une lecture ; index par migration. |
| D6, D12, W16, C18, C20, C21 | divers | Voir les sections précédentes. | |

### Tests qui manquent

Aucun test ne couvre : `set_url` vers un serveur sans plages (T1), une source lente seule
après la perte de la référence (T2), une adresse au nom seul en SFTP (F1), un `PASV` qui
annonce une autre adresse, l'abandon d'une plage FTP (`ABOR`), le FTPS implicite en local,
les réponses SOCKS5 abîmées, le choix des fichiers changé à la fin d'un torrent (V1), une
erreur de base pendant un passage de torrent (V2), une extension hostile (V3), un nom de
flux déjà pris (V4), un sous-titre en échec définitif (V5), les secrets des tâches internes
(V9), `ui/src/web.js` (W2, W12), `auro watch`.

## 5. Décisions demandées à l'utilisateur

Points où le code et la SPEC divergent, ou où le correctif change un comportement décrit :

1. **C3, marque du Web** : écrire `Zone.Identifier` sur les fichiers reçus sous Windows,
   comme un navigateur (SmartScreen, Mode protégé d'Office) ? Toujours, en option active par
   défaut, ou non.
2. **S8, contrôle de couverture au démarrage** : l'ajouter (SPEC 5.6, règle 2), ou corriger
   la SPEC et `CLAUDE.md` (contrôle à la reprise de chaque tâche).
3. **T4, refus `503` avec un nombre de connexions fixé** : remettre le segment en attente
   comme en automatique, ou garder l'échec de l'exécution et l'écrire en SPEC 7.3.
4. **P4 et P5, action de fin** : un partage de torrent ne retient pas l'action de fin ;
   une action armée se désarme si la file se vide par une action de l'utilisateur.
5. **V22, ratio par tâche** : l'écrire (`task.update`), ou retirer la phrase de la SPEC 7.16.
6. **T11, passage de HTTPS à HTTP** : l'enregistrer et l'annoncer dans la tâche (SPEC 8.1),
   ou limiter la SPEC à la CLI.
7. **W2, jeton `add` de la page distante** : la SPEC 7.19 le dit validé alors que la page
   ne peut pas s'y connecter ; corriger la page, et noter l'écart.
8. **W9, liens magnet en navigation privée** : suivre `captureIncognito`, ou non.
9. **P18, commandes `.bat` et `.cmd`** : les refuser, ou documenter qu'elles passent par
   `cmd.exe`.
10. **C17, « Wi-Fi seulement » sur Android** : exiger le Wi-Fi, ou renommer en « réseau non
    facturé ».
11. **V10, mise à jour de yt-dlp** : refuser une version plus ancienne que l'installée.
12. **S7, retéléchargement sans règle** : dossier de base plutôt que l'ancien dossier.
13. **R18** : retirer de la SPEC 6.4 les méthodes non servies (`engine.stats`, `task.move`,
    `queue.delete`) et compléter la liste des portées de la SPEC 6.9.

## 6. Plan de correction proposé

Par lots, chacun avec ses tests, puis fmt, clippy et tests sous Windows et Linux, et le banc
de sa partie ; un commit par lot. À la fin, la campagne de la porte (étape 10) est refaite
sur le code corrigé, banc de 1 000 cycles compris.

1. **Critiques** : T1, P1 (`crash_resume`, `crash_daemon`).
2. **Intégrité et reprise** : S1, S2, S4, V1, V2, V4, R2 (`crash_resume`, `crash_daemon`,
   `crash_torrent`, `crash_media`).
3. **Sécurité** : D1, R1, D2, R3, V3, F4, W1, W10, puis T6, T7, D4 à D9, D11, D12.
4. **Fonctions qui échouent** : P2, P3, P4, T2, T3, T4, F1, F2, V5, D3, C4, W2, W5, avec
   l'aide commune P23.
5. **Mineures restantes**, par partie.
6. **Nettoyage** : code mort, redondances, simplifications, sans changement de comportement.
7. **Performance** : seulement ce qu'un banc mesure (T15 à T18 face à aria2, P27 et R12 par
   un compteur), avec relevé dans `docs/MESURES.md`.
8. **Workflows** : W3, W4, W17 (avant le premier envoi sur GitHub).

## 7. Non relu

- Le contenu détaillé des fichiers de tests : seuls leurs noms et leurs usages ont été
  parcourus pour juger de la couverture.
- `torrent/fuzz.rs`, `testing/*` (survolés), `third_party` (hors du client reqwest).
- Une partie de `crates/cli/src/remote.rs` (règles, plugins, jetons, accès distant).
- `ui/Screens.kt` et `ui/Format.kt` en détail, `style.css` ligne à ligne, `bundle.ps1`,
  `bundle.sh` et `ci-instrument.sh`.
- Aucun constat n'a été essayé sur un appareil Android ni en conditions réelles.

## 8. Suivi des correctifs

| Lot | Constats | Commit | Vérification |
| --- | --- | --- | --- |
| 1 | T1, P1 | 3908efb | Tests qui échouaient sans le correctif (`a_split_task_moved_to_a_server_without_ranges_restarts_whole`, `a_due_retry_that_cannot_start_does_not_spin_the_loop`) ; fmt, clippy, tests sous Windows et Linux ; banc de reprise (300 cycles) et banc d'`aurod` (150 cycles) sous les deux. Sous Windows, `busy_server_halves_connections_and_the_download_goes_on` a échoué une fois, machine chargée par les deux campagnes (plafond 7 au lieu de 3 : montée plus rapide que prévu), puis réussi 5 fois sur 5 seul : test sensible au temps, à revoir avec T3 et T4. |
| 2 | S1, S2, S4, V1, V2, V4, R2 | ab7e3c5 | Tests nouveaux ou adaptés : `a_failed_sync_fails_every_later_sync`, `a_part_whose_sync_failed_is_not_sealed`, `waiters_fail_after_a_failed_sync` (S1) ; `a_move_reserves_its_target_name` (S2 : nom réservé refusé, réservation libérée ; la course elle-même, trop brève avec une copie de test, n'est pas reproduite) ; tests des tentatives et des tours adaptés (S4 : automatique gardé, relance de l'utilisateur remise à zéro) ; `a_file_chosen_as_the_download_ends_is_fetched_too` (V1, par un crochet de test ; échoue sans le correctif : fichier rempli de zéros) ; `a_file_already_named_like_a_stream_is_left_alone` (V4, échoue sans le correctif) ; `a_second_engine_on_the_same_base_is_refused` et `store::lock` (R2). V2 (erreur de la base pendant un passage de torrent) sans test dédié : relu, et couvert par le banc des torrents. Le rebond sans remplacement de S2 n'est pas fait : la réservation suffit contre les tâches du moteur, un autre programme reste hors de portée (comme pour `SealedPart::finish`). fmt, clippy, tests sous Windows et Linux ; bancs de reprise (300), d'`aurod`, des torrents et des vidéos (150) sous les deux. |
| 3 | D1, R1, D2, R3, V3, V21, F4, W1, W10, W18, T6 (D10), T7, D4, D5, D6, D7, D8, D9, D11, D12, D15 | 952230a | Tests qui échouaient sans le correctif : `a_token_cannot_rewrite_a_task_outside_the_download_folder` (D1), extension du test des sous-dossiers à l'instantané et aux annonces (R1), jeton révoqué servi (D2, dans le test des connexions à jeton), `crates/daemon/tests/test_endpoint.rs` et `auro_endpoint_without_a_database_is_refused` (R3), `hostile_extensions_and_non_http_streams_are_refused` (V3, V21), cas `ftp:vrai` (F4), `extension/tests/content.test.cjs` (W1, W10), cas en clair et autre port de `page_headers_stay_with_their_host`, `same_origin_compares_scheme_host_and_port` (T6), `task_secrets_do_not_follow_a_redirect_into_a_challenge` (T7), `websockets_have_their_own_limit` (D4), réinstallation qui répare (D5), `a_64_network_is_one_offender_and_the_table_stays_bounded` (D6), `a_module_with_too_many_functions_is_refused_at_inspection` (D8), cas NAT64 /48 (D9), `secure_cookies_stay_on_https_and_expired_ones_go` (D11). Sans test dédié : D7 (délai d'écriture d'un WebSocket), D12 (borne de la réponse d'`auro.fetch`). D15 : `docs/SECURITE.md` mis à jour, complément du 2026-11-19. fmt, clippy, tests sous Windows et Linux (clippy relancé après la fusion de deux branches identiques) ; bancs de reprise (300), d'`aurod` et des vidéos (150) sous les deux ; 143 tests de la page, de l'extension et des scripts. |
| 4 | P2, P3, P4, P23, T2, T3, T4, F1, F2, V5, D3, C4, W2, W5 | 8b7848b | Tests qui échouaient sans le correctif : `a_closing_window_stops_downloads_only` (P2, P3, aide commune P23 : `Running::requeue_unless`, utilisée par les plages, l'ajout d'un miroir, le changement d'interface et de réseau), `sharing_does_not_hold_the_end_action_back` (P4 ; le désarmement de P5 vient au lot 5), `revive_slow` dans `the_best_rate_per_connection_gets_the_next_one` (T2), plafond par source (T3), `imposed_connections_wait_out_a_refusal` (T4, décision 3 ; `a_busy_server_is_retried_automatically` adapté : seul un serveur qui refuse encore après trois épisodes fait échouer l'exécution), cas du nom seul dans `credentials_leave_the_address` et `credentials_come_from_the_task_then_the_address_then_the_site`, `an_sftp_address_with_a_user_only_takes_the_site_password` (F1), `a_subtitle_failing_for_any_reason_is_reported_missing` (V5), `a_fresh_plugin_link_keeps_its_own_cookies_on_another_host` (D3), `erasing_a_base_removes_its_secrets_only` (C4, `aurod --erase-secrets`), `ui/tests/web.test.js` (W2), `ui/tests/remote-login.test.js` (W5). Sans test dédié : F2 (`abandon` d'une lecture SFTP en erreur). C4 : crochets de `hooks.nsh` relus, pas encore essayés dans un installateur d'essai (à faire avec la campagne de la porte). fmt, clippy, tests sous Windows et Linux ; bancs de reprise (300), d'`aurod`, des torrents et des vidéos (150) sous les deux ; 143 tests de la page, de l'extension et des scripts. Sous Linux, `anonymous_probe_ranges_and_whole_file` (FTP, chemin que le lot ne touche pas) a échoué une fois sous la charge de la campagne (deux connexions de commande au lieu d'une : `ABOR` en tâche de fond pas revenu à temps), puis réussi 10 fois sur 10 seul : test sensible au temps. |
| 5a | S3, S5, S6, S7, S8, T5, T8, T9, T10, T11, T12, T13, T14, F3, F5, F6, F7, F8, F10, C3 | 1f799ce | Décisions de la section 5 : 1 (C3, marque du Web toujours écrite sous Windows, zone 3, sans l'adresse), 2 (S8, contrôle de couverture au démarrage : `failed` `INTERNAL` quand l'état le permet), 6 (T11, migration 15 `tasks.insecure_redirect`, `insecureRedirect` de `task.get`, `engine.warning` `INSECURE_REDIRECT`), 12 (S7, retéléchargement sous `rule_base`). Choix proposés et faits : `Retry-After` plafonné à une heure (T5) ; empreinte voisine limitée à HTTP(S) (T14) ; repli du `HEAD` sur délai ou coupure, pas sur un serveur injoignable (T12). Tests qui échouent sans le correctif : `a_preparation_that_cannot_be_saved_undoes_its_claim` et `content_length_is_strict` (S3), cas S5 de `only_finished_tasks_are_removed_and_their_history_stays`, cas S7 de `a_finished_task_restarts_from_zero_with_its_address`, `broken_segments_are_found_at_start` (S8), `retry_delays_double_up_to_the_cap` (T5), `the_host_of_an_ftp_task_reaches_the_rules` (T8), `a_416_with_a_size_is_not_taken_for_that_file` (T10), `a_redirect_from_https_to_http_is_noted_and_announced` (T11), `a_head_cut_short_falls_back_to_a_get` (T12), `a_mirror_that_sends_short_ranges_is_accepted` (T13), `proxy_errors_are_classed_by_who_refuses` (F3), `pending_keys_leave_and_stay_bounded` (F6), `idle_command_connections_close_after_their_rest` (F8), `a_finished_file_carries_the_mark_of_the_web` (C3, Windows). Tests sans contre-épreuve possible ici : S6 (la constante vaut pour Android, non lancé sur un appareil), `socks_with_local_resolution_tries_each_address` (F7 : dépend de la résolution de `localhost`), `only_an_epsv_refusal_falls_back_to_pasv` (F10, test de la fonction de décision). Sans test : T9 (champ d'un journal), T14 (sans effet observable : reqwest refusait déjà ces adresses), F5 (délai de 30 s et arrêt de la tâche de données). SPEC 5 (migration 15), 5.6, 6.5, 7.1, 7.7, 7.8, 7.9, 7.21, 7.22, 8.1 mises à jour. fmt, clippy, tests sous Windows (587) et Linux (585) ; bancs de reprise (300), d'`aurod`, des torrents et des vidéos (150) sous les deux ; bureau (fmt, clippy, 23 tests) sous Windows. |
| 5b | P5, P6, P7, P8, P9, P10, P11, P12, P13, P14, P15, P16, P17, P18, P19, P20 | 847c6bf | Décisions de la section 5 : 4 (P5, désarmement par une pause ou une annulation de l'utilisateur, réarmement par une fin), 9 (P18, `.bat` et `.cmd` refusés à l'écriture d'une règle). Choix faits : annulation pendant un échec par `failed` → `queued` → `cancelled` (la table de la SPEC 5.5 inchangée, P13) ; tâche créée en pause dans la transaction de création, le passage `queued` → `paused` toujours annoncé (P14) ; cause de pause `schedule` ou `network`, l'arrêt du moteur et l'ajout d'un miroir restant `user` (P10) ; avertissement `TASK_SET_ASIDE` (P12) ; tâche interne vue comme absente (`TASK_NOT_FOUND`, P20). Tests qui échouent sans le correctif (contre-épreuve groupée) : `a_user_pause_disarms_the_end_action` (P5), `the_loop_answers_while_the_end_action_runs` (P6), `run_notes_belong_to_their_run` (P7), `a_pause_keeps_a_cancel_in_progress` (P8), `the_checksum_of_a_renamed_file_stays` (P9), cas P10 de `stop_request_pauses_then_the_task_resumes` et de `a_closing_window_stops_downloads_only`, `a_refused_requeue_is_tried_again` (P11), `set_aside_tasks_are_announced_and_sharing_ends` (P12), `a_cancel_survives_a_failure_at_the_same_time` (P13), `an_added_task_starts_paused_and_ruled_in_one_write` (P14), cas P16 de `a_fresh_plugin_link_keeps_its_own_cookies_on_another_host`, `a_copy_the_base_refuses_keeps_its_journal` (P17), cas P18 de `bad_rules_are_refused`, `the_next_change_across_a_clock_change_keeps_its_local_hour` (P19, sur une machine à heure d'été), `internal_tasks_are_out_of_reach` (P20). Sans test : P15 (boucle qui continue après une tâche non relancée). SPEC 5.5, 6.5, 7.11, 7.12, 7.15, 7.21 mises à jour. fmt, clippy, tests sous Windows (599) et Linux (596, puis le test de P14 corrigé : il comptait sur un dossier « Téléchargements », absent sous WSL ; repassé avec les 62 tests du planificateur) ; bancs de reprise (300), d'`aurod`, des torrents et des vidéos (150) sous les deux ; bureau sous Windows. Reste noté : le coffre est encore appelé depuis la boucle (ajout d'une tâche à secrets, effacement, proxy), au plus quelques millisecondes avec le coffre du système. |
| 5c | R4, R5, R6, R7, R8, R9, R10, R11, R13, R14, R15, R16, R17, R18 | (ce commit) | Décision 13 (R18) : `engine.stats`, `task.move` et `queue.delete` retirés de la SPEC 6.4 et des portées, liste de la SPEC 6.9 complétée. Choix faits : avancement anticipé avec la révision précédant le changement d'état (R4) ; rotation retentée après une minute (R16) ; proxy : ancien réglage remis en base si le client refuse le nouveau (R15). Tests qui échouent sans le correctif (contre-épreuve un par un pour ceux qu'un même test regroupe) : `the_last_progress_comes_before_the_state_change` (R4), cas R6, R8, R11 et R17 de `malformed_messages_get_standard_errors`, `the_secret_collection_spares_fresh_secrets` (R7), cas R10 de `the_diagnostic_removes_addresses_tokens_hosts_and_paths`, `a_revoked_token_cuts_a_call_in_progress` (R13), `a_proxy_the_base_refuses_is_not_used` (R15), `a_failed_rotation_keeps_the_events` (R16). Sans test dédié : R5 (garde défensive : aucune méthode servie n'est sans portée, ce que vérifie `every_served_method_has_a_scope`), R9 (erreur des outils dans le diagnostic : pas de faux outils en erreur dans les tests du protocole), R14 (la course entre deux commandes, supprimée, ne se reproduit pas en test). R12 (événement `task.added` sans relecture) relève du lot 7 (performance mesurée). SPEC 6.4, 6.5 (N-12), 6.7, 6.9, 6.10 mises à jour. fmt, clippy, tests sous Windows (604) et Linux (602) ; banc d'`aurod` (150) sous les deux ; bureau sous Windows. |

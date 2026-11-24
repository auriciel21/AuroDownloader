# Revue de sécurité : interface distante et plugins

Porte de la phase 6 (étape 5), revue du 2026-11-12. Elle porte sur l'interface web distante
(F-39, SPEC 7.19), les plugins d'hébergeurs (F-41, SPEC 7.20) et le canal local du
protocole (SPEC 6.2), par lequel passent les clients. Méthode, décidée par l'utilisateur :
- un modèle de menace ;
- la lecture du code point par point ;
- chaque point vérifié par un test ou un essai hostile ;
- les failles avérées corrigées au fil, chacune avec un test qui la couvre.

## 1. Modèle de menace

Ce qu'il faut protéger :
- les fichiers de l'utilisateur ;
- les secrets gardés au coffre du système : cookies des tâches, identifiants des sites ;
- le réseau interne de la machine (box, NAS, services de la boucle locale) ;
- la disponibilité du moteur ;
- la machine elle-même : aucune exécution de code venu de l'extérieur.

| Attaquant | Ce qu'il a | Ce qu'il ne doit pas obtenir |
|---|---|---|
| A. Réseau local, sans jeton | Accès au port de l'écoute | Aucun appel au moteur ; ni deviner un jeton, ni épuiser le moteur |
| B. Page web hostile dans le navigateur de l'utilisateur | Requêtes du navigateur vers l'écoute ou la boucle locale | Aucun appel (CSRF, DNS rebinding), aucune lecture de réponse |
| C. Détenteur d'un jeton (appareil perdu) | Sa portée : `read`, `add` ou `control` | `admin`, un fichier hors du dossier des téléchargements, le réseau interne, un secret |
| D. Auteur d'un plugin hostile, installé par l'utilisateur | Le code du plugin, ses domaines déclarés | Fichiers, système, autres domaines, réseau interne, secrets, arrêt du moteur |
| E. Site d'hébergement hostile (plugin honnête) | Les réponses aux requêtes du plugin | Plus que D |
| F. Autre compte de la même machine | Un accès local sans privilège | Les messages des clients de l'utilisateur, son moteur |

Hors périmètre : un administrateur de la machine, ou un programme qui tourne déjà sous le
compte de l'utilisateur. Ils ont de toute façon tous ses droits.

## 2. Revue point par point

### Interface distante

| Point | Ce qui est fait | Vérifié par |
|---|---|---|
| Jetons | 32 octets de l'aléa du système ; seule leur empreinte SHA-256 est en base ; rendus une seule fois ; jamais la portée `admin` | `remote::tests`, `store::remote::tests` |
| Sessions | Cookie aléatoire (empreinte en base), `HttpOnly`, `SameSite=Strict`, `Secure` en TLS, 30 jours (12 heures sans TLS, D13) ; supprimé avec son jeton | `server_tests` |
| Révocation | Ferme aussitôt les connexions en cours du jeton (WebSocket compris), même après des événements perdus | `rpc::tests` (jeton révoqué) |
| Authentification | `Bearer` partout ; cookie seulement pour le WebSocket, `/session` et `/logout`, avec un `Origin` de même origine ; jamais pour `POST /rpc` (CSRF) | `server_tests` |
| `Host` | Adresse IP écrite telle quelle, ou nom connu, avec le bon port (DNS rebinding) | `server_tests`, fuzzing |
| `Origin` | Même origine, une seule valeur ; aucun en-tête CORS | `server_tests`, fuzzing |
| Essais refusés | Plus de 10 échecs en une minute : l'adresse est refusée une minute | `server_tests`, fuzzing (adresses 127.0.0.x) |
| Épuisement | 64 connexions ; négociation TLS et en-têtes en 10 s ; corps de `/login` 4 Kio, de `/rpc` 24 Mio en 120 s ; tampon de 64 Kio | `remote::fuzz` (clients muets et lents) |
| TLS | Hors de la boucle locale, toujours ; certificat fabriqué, ou fourni ; empreinte montrée pour la vérifier | `remote::tls::tests` |
| Clé privée | Créée lisible par l'utilisateur seul : `0600`, ou un descripteur Windows à une seule entrée | `remote::tls::tests` (deux systèmes) |
| Portées | Table `rpc::scope::required` ; `admin` (réglages, jetons, plugins, identifiants, arrêt) inaccessible par jeton | `scope::tests` ; **garde nouvelle**, voir C5 |
| Dossier | Une tâche d'un jeton reste sous le dossier des téléchargements, chemin résolu (`canonicalize` : liens et jonctions) | `rpc::tests` (sous-dossier, `..`, lien vers ailleurs) |
| Réseau interne (SSRF) | Résolveur filtrant, à chaque connexion et à chaque redirection ; adresses écrites telles quelles refusées ; sans mandataire ; pour toute la vie de la tâche | `net::reach::tests`, `net::tests`, `rpc::tests` ; **6to4 et NAT64 local ajoutés**, voir C6 |
| Secrets | Jamais rendus par le protocole ; les cookies d'une tâche ne suivent pas un changement d'hôte ; réglages `remote.*` cachés à un jeton | `rpc::tests` |
| Page | Liste blanche de fichiers intégrés ; politique de sécurité stricte ; `X-Frame-Options`, `nosniff`, `no-referrer`, `no-store` ; aucune donnée insérée par `innerHTML` (seulement deux icônes fixes, et la copie du guide, fragment figé dans le binaire, produit avec échappement et vérifié sans script ni attribut `on*` par `scripts/tests/guide.test.mjs` ; mise à jour de l'audit du 2026-11-19, D15) | `remote::page::tests`, lecture de `ui/src` |
| Canal local | Canal nommé réservé au SID de l'utilisateur, ou socket `0600` dans un dossier `0700` ; **le client vérifie désormais le propriétaire**, voir C3 | `winsec::tests`, `rpc::local::tests` |

### Plugins

| Point | Ce qui est fait | Vérifié par |
|---|---|---|
| Installation | Depuis un fichier choisi ; manifeste, domaines et empreinte montrés ; installé seulement sur l'empreinte acceptée ; revérifié à chaque chargement ; portée `admin` | `plugins::tests`, `rpc::tests`, CLI, essai de l'interface |
| Imports | `auro.fetch` et `auro.log` seulement : ni fichiers, ni système, ni WASI, ni horloge | `plugins::tests` |
| Limites | Calcul 5·10⁹, mémoire 64 Mio, 10 000 appels imbriqués, 60 s, deux résolutions à la fois, instance neuve à chaque fois ; **tables limitées**, voir C1 | `limits_stop_a_hostile_plugin`, `a_huge_table_is_refused` |
| Exécution | Le code ne tourne que par `auro_resolve` : **fonction de démarrage refusée**, voir C2 | `a_start_function_is_refused` |
| Requêtes | Domaines déclarés seulement, redirections comprises ; réseau public ; sans mandataire ; `GET` et `POST` de formulaire ; liste fermée d'en-têtes (valeurs sans retour à la ligne) ; 32 requêtes ; réponses de 8 Mio | `requests_stay_within_declared_domains…`, `fetch::tests` |
| Cookies du site | Gardés par le moteur, jamais montrés au plugin ; un cookie ne va qu'aux hôtes qu'il couvre ; 64 au plus ; donnés au téléchargement par le coffre | fuzzing `cookies_never_leak_to_other_hosts`, `rpc::tests` |
| Liens rendus | `http(s)` seulement, 1 000 au plus ; noms nettoyés ; tâches au réseau public seulement | `links_are_checked_and_names_cleaned`, fuzzing de la sortie |
| Lien expiré | Redemandé une fois, de nouveau 10 minutes plus tard au plus tôt ; jamais à un plugin désactivé ou retiré ; **cookies du lien neuf retirés s'il est refusé**, voir C4 | `rpc::tests` |
| Textes du plugin | Manifeste et messages sans caractère de contrôle (ni séquence d'échappement de terminal), longueur bornée, toujours affichés comme du texte | `manifest::tests`, lecture de `ui/src` |
| Résolutions gardées | Identifiant aléatoire (UUID v4), 32 au plus, 10 minutes ; jamais leurs cookies dans une réponse | `resolutions_are_kept_for_a_while` |
| Fonctions de test | Le réseau interne et les noms imposés (`plugins_allow_private`, `plugins_resolve_to`) n'existent qu'avec la fonctionnalité `testing`, prise seulement par les dépendances de développement | `Cargo.toml` des crates |

## 3. Constats et corrections

Chaque correction est couverte par un test. Celui de C4 a été lancé sans le correctif, et
il échoue.

**C1. Tables WebAssembly sans limite de taille.** Gravité : élevée pour la disponibilité.
- Scénario (D) : les limites du magasin de wasmi bornaient le *nombre* de tables, pas leur
  taille. Un plugin qui déclare `(table 4294967295 funcref)`, ou qui agrandit une table sans
  fin, fait allouer au moteur des dizaines de Gio. L'allocation échoue et le processus
  s'arrête. Il suffisait d'un `plugin.inspect`, avant même l'accord de l'utilisateur.
- Correction : 100 000 entrées par table au plus (`MAX_TABLE_ELEMENTS`). Un plugin Rust en a
  quelques centaines.
- Test : `a_huge_table_is_refused` (table déclarée, agrandie, et une table ordinaire qui
  reste permise).

**C2. Fonction de démarrage exécutée à l'inspection.** Gravité : moyenne.
- Scénario (D) : `plugin.inspect` instancie le module pour le vérifier. Une section `start`
  faisait donc tourner du code avant que l'utilisateur ait accepté le plugin (dans le bac à
  sable et ses limites, mais contre la règle « rien ne s'exécute avant l'accord »).
- Correction : tout module qui a une fonction de démarrage est refusé, à la vérification
  comme à l'exécution (`manifest::refuse_start`) ; le code ne tourne que par
  `auro_resolve`.
- Test : `a_start_function_is_refused`.

**C3. Canal local usurpable par un autre compte de la machine.** Gravité : moyenne, sur une
machine à plusieurs comptes.
- Scénario (F) : sous Windows, le client ouvrait `\\.\pipe\aurodownloader-<nom>` sans
  vérifier qui l'avait créé. Un autre compte qui le crée le premier, avant le démarrage du
  moteur, reçoit les messages des clients, dont les cookies transmis par le relais de
  l'extension. Sous Linux sans `XDG_RUNTIME_DIR`, le socket est sous `/tmp`, avec le même
  risque.
- Correction :
  - sous Windows, le client vérifie que le propriétaire du canal est l'utilisateur
    (`winsec::owned_by_current_user`). Le moteur fait de l'utilisateur le propriétaire de
    son canal, même s'il tourne élevé ;
  - sous Linux, le client refuse un dossier du socket qui n'appartient pas à l'utilisateur
    ou qui est ouvert à d'autres.
- Tests : `the_owner_of_an_object_is_read_from_its_handle` (un fichier à soi, un fichier du
  système), `a_client_refuses_an_endpoint_folder_open_to_others` ; tous les tests de la CLI
  passent par le contrôle.

**C4. Cookies d'un lien neuf laissés sur l'ancienne adresse.** Gravité : faible.
- Scénario (D, E) : quand un lien expire, le moteur range les en-têtes du lien neuf
  (cookies compris), puis vérifie son adresse. Si cette vérification échouait, la tâche
  gardait son ancienne adresse avec les cookies d'un autre hôte, et une reprise à la main
  les y envoyait.
- Correction : sur un refus, seuls les en-têtes en clair restent.
- Test : `a_refused_fresh_link_leaves_no_cookie_on_the_old_address`. Sans le correctif, le
  cookie de `autre.exemple` partait vers l'ancien hôte.

**C5. Table des portées ouverte par défaut.** Gravité : renforcement, aucune faille actuelle.
- Constat : une méthode servie mais oubliée dans `rpc::scope::required` aurait été permise
  à toute portée, un jeton `read` compris. Toutes y étaient.
- Correction : le test `every_served_method_has_a_scope` lit les méthodes servies par
  `rpc/methods.rs` et exige la portée de chacune.

**C6. Filtre du réseau public.** Gravité : faible.
- Constat : les adresses 6to4 (`2002::/16`), NAT64 d'usage local (`64:ff9b:1::/48`) et IPv4
  traduite (`::ffff:0:a.b.c.d`) n'étaient pas jugées sur leur adresse IPv4.
- Correction : elles le sont désormais.
- Test : `internal_addresses_are_not_public`.

## 4. Risques résiduels acceptés

- **yt-dlp** fait ses propres requêtes. Seule l'adresse de la page est vérifiée pour un
  jeton. Les traqueurs et les pairs des torrents ne sont pas filtrés (SPEC 7.19, limites
  connues).
- **Résolutions d'un plugin** : leur identifiant vaut pour toute connexion de portée
  `add`. Il est aléatoire (122 bits) et n'est rendu qu'au client qui l'a demandé. Une tâche
  ajoutée ainsi garde les limites de la connexion qui l'ajoute.
- **Domaines d'un plugin** : un suffixe public partagé (`co.uk`, `github.io`) est accepté
  comme domaine. Il est montré à l'installation, que l'utilisateur accepte ou non.
- **Détenteur d'un jeton** : il peut occuper les deux places de résolution des plugins
  (60 s chacune), ou lancer des téléchargements. Un jeton se retire à tout moment.
- **Messages d'un plugin** : texte libre, affiché sous la mention « Messages du plugin ».
- **Certificat fabriqué** : le navigateur de l'appareil le signale ; l'utilisateur compare
  l'empreinte.
- **Essais refusés** : la limite est par adresse. Une plage IPv6 permet d'en changer, mais
  un jeton de 256 bits ne se devine pas.
- **Session expirée** : un WebSocket ouvert par cookie dure jusqu'à sa fermeture. Le retrait
  du jeton, ou la fermeture de l'écoute, le ferme aussitôt.

## Complément du 2026-11-12 : proxy (F-07, phase 7)

- **Connexions limitées au réseau public.** Un proxy HTTP ou HTTPS résout lui-même les
  noms (`CONNECT hôte`). Un proxy d'entreprise à DNS séparé pourrait donc joindre une
  adresse interne qu'un nom public cache, hors du contrôle du moteur.
  - Ces connexions (jetons distants, plugins, leurs tâches) ne suivent qu'un proxy SOCKS5,
    interrogé en `socks5://`. Le moteur y résout le nom par son résolveur filtrant, refuse
    une adresse interne, puis donne l'adresse IP au proxy.
  - Avec un proxy HTTP, ou celui du système, elles sortent en direct (décision de
    l'utilisateur).
  - Tests : `public_connections_never_go_through_an_http_proxy`, et
    `socks5_proxies_resolve_where_asked_and_public_connections_stay_checked` (une adresse
    interne est refusée avant que le proxy soit joint).
- **Mot de passe du proxy.**
  - Il est rangé au coffre du système ; la base n'en garde que la référence, comptée dans
    le ramassage des secrets du démarrage.
  - Il n'est jamais rendu par le protocole ni écrit dans un journal.
  - yt-dlp le reçoit par son environnement, jamais en argument : les arguments d'un
    processus se lisent par les autres comptes de la machine.
  - Test : `downloads_follow_the_proxy_and_its_password_stays_in_the_vault` (il n'apparaît
    ni dans la base ni dans son journal WAL).

## Complément du 2026-11-16 : règles de rangement et commande de fin (F-32, F-34)

- **Écriture des règles.** Une règle choisit un dossier (absolu) et un nom, et peut lancer
  un programme : écrite à distance, elle vaudrait une exécution de code sur la machine
  (un fichier `.bat` rangé dans le dossier de démarrage de Windows, ou une commande).
  - `rules.set` a la portée `admin`, qu'aucun jeton n'a : seul le canal local (bureau,
    CLI) écrit les règles (décision de l'utilisateur).
  - `rules.list` est permise à toute portée, mais une connexion à jeton n'y voit pas les
    commandes, seulement qu'il y en a une (leurs arguments peuvent porter un secret).
  - Les règles écrites localement valent pour toutes les tâches, y compris celles qu'un
    jeton ajoute : une tâche distante peut donc finir hors du dossier des téléchargements,
    là où l'utilisateur l'a voulu. Le jeton ne choisit toujours pas le dossier.
  - Test : `rules_place_tasks_added_without_a_folder`.
- **Noms venus du serveur.** Le modèle de nom contient le nom du serveur : le résultat
  passe par `net::filename::sanitize`, et un modèle ne peut pas contenir de séparateur de
  dossier. Un dossier relatif n'a ni `.` ni `..`.
- **Expressions régulières.** Crate `regex` : temps linéaire, sans retour arrière ;
  automate borné à 1 Mio, une expression plus grosse est refusée à l'écriture
  (test `bad_rules_are_refused`).
- **Commande de fin.**
  - Programme en chemin absolu et arguments, jamais de shell : un nom de fichier venu du
    serveur ne peut rien injecter, chaque variable reste un seul argument.
  - Entrée fermée, sorties jetées ; ni ses arguments ni ses sorties ne vont aux journaux
    (seulement son issue et l'identifiant de la tâche).
  - Délai propre à chaque règle ; à son terme, ou à l'arrêt du moteur, tout l'arbre de
    processus est tué (Job Object sous Windows, groupe sous Linux).
  - Pas de commande sous Android.

## Complément du 2026-11-19 : audit du code avant la version 1 (`docs/AUDIT.md`, lot 3)

Constats de sécurité de l'audit, corrigés avec un test qui échouait avant le correctif
(sauf mention) :

- **D1, jeton et tâches locales.** La poignée d'une connexion à jeton ne contrôlait le
  dossier qu'à l'ajout et au déplacement vers un autre dossier : sur une tâche locale
  existante, elle pouvait changer l'adresse, retélécharger, renommer sur place ou ajouter
  un miroir, et faire écrire son contenu dans un dossier quelconque. Ces quatre actions
  exigent désormais une tâche sous le dossier des téléchargements (`DEST_FORBIDDEN`,
  `Engine::guard_folder`). Le renouvellement d'un lien de plugin passe par une poignée
  « réseau public seulement » du moteur (`public_network`), sans ces limites de jeton.
  Test : `a_token_cannot_rewrite_a_task_outside_the_download_folder`.
- **R1, réglages de l'écoute.** `settings.get` les cachait au jeton, mais `sync.snapshot`
  et `settings.updated` les donnaient (chemins du certificat et de sa clé, noms). Les trois
  les cachent ; l'annonce garde sa révision, sans valeur. Test étendu :
  `a_sub_folder_is_created_under_the_destination_and_never_escapes_it`.
- **D2, révocation pendant l'ouverture d'un WebSocket.** Le jeton se relit après
  l'abonnement aux événements : une révocation arrivée entre la vérification et
  l'abonnement ne laisse plus une connexion ouverte pour toujours.
- **D4, limite des WebSocket.** hyper rend la place d'une connexion à sa mise à niveau :
  la limite de 64 ne valait pas pour les WebSocket. Ils ont la leur (64, au-delà `503`).
  Test : `websockets_have_their_own_limit`.
- **D6, table des échecs.** Comptés par réseau /64 en IPv6, table bornée, ménage amorti.
- **D7, client qui ne lit plus.** Chaque envoi vers un WebSocket a un délai de 30 s (sans
  test dédié : il faudrait remplir les tampons du système).
- **R3, `AURO_ENDPOINT`.** Les gardes contre un moteur d'essai ouvert sur la base réelle
  comparaient au point d'accès effectif, qui rend la variable : un navigateur d'essai
  faisait démarrer par le relais un `aurod` d'essai sur la base réelle. Ils comparent
  désormais à `paths::default_endpoint()` (`aurod`, CLI, relais). Test :
  `crates/daemon/tests/test_endpoint.rs`.
- **T6, T7, origine des secrets.** `Cookie` et `Authorization` ne suivent une adresse que
  de même origine (protocole, hôte, port : `net::same_origin`), ni sa variante en clair,
  ni un autre port, y compris après une redirection qui demande à s'identifier.
- **F4, compte FTP nommé `ftp`.** Avec un vrai mot de passe, ce n'est plus un accès
  anonyme : le mot de passe ne part plus en clair sans autorisation.
- **V3, V21, sorties de yt-dlp.** Une extension hostile devient `bin` ; un flux n'est direct
  (tâche interne) que si son adresse est en HTTP(S).
- **W1, W10, extension.** Un clic fabriqué par la page (`a.click()`) ne confie plus de lien
  magnet à AuroDownloader, et ne passe plus pour un Alt+clic. Test :
  `extension/tests/content.test.cjs`.
- **Plugins.** D5 : un fichier de plugin abîmé est réécrit à la réinstallation ; D8 : la
  lecture d'un module est bornée par `EnforcedLimits::strict` de wasmi ; D9 : le préfixe
  NAT64 local `64:ff9b:1::/48` est toujours refusé ; D11 : un cookie `Secure` ne repart
  qu'en HTTPS, une date `Expires` passée l'efface ; D12 : la réponse d'`auro.fetch` est
  bornée à 12 Mio une fois encodée.
- **D13, session sans TLS (lot 5e, 2026-11-24).** Sans TLS, l'écoute ne s'ouvre que sur
  la boucle locale, mais le cookie de session, en clair, part vers tout port de l'hôte :
  un autre service local pouvait le recevoir. La session y dure 12 heures au lieu de
  30 jours, en base comme dans `Max-Age`. Test étendu :
  `a_browser_session_opens_the_websocket_from_its_own_origin_only` (et 30 jours en TLS :
  `the_network_listener_serves_tls_with_its_certificate`).
- **D14, chargement d'un plugin.** La lecture du fichier et le contrôle de son empreinte
  se font hors des fils de l'exécuteur (`spawn_blocking`) ; le contrôle lui-même est
  inchangé (sans test dédié : les tests des plugins couvrent fichier absent et modifié).
- **C14, fichiers lus pour la page (bureau, lot 5f2, 2026-11-24).** `read_torrent`,
  `read_plugin` et `read_list` lisaient tout chemin absolu donné par la page (défense en
  profondeur : une page détournée aurait fait lire un fichier de l'utilisateur). Elles ne
  lisent plus que les fichiers choisis dans un sélecteur ou reçus du système (arguments,
  glisser-déposer) : `opened::Granted`. Test : `only_granted_files_may_be_read`.
- **C5, fichiers jamais ouverts.** Liste complétée (scripts Python, aide compilée, consoles,
  raccourcis, images de disque montées par Windows). Test étendu.

## 5. Vérifications

- Campagne de la porte, sous Windows et sous Linux :
  - `fmt`, `clippy -D warnings`, tous les tests ;
  - licences, bureau et page ;
  - bancs `crash_resume` (300 cycles) et `crash_daemon` (150 cycles) ;
  - fuzzing long des lecteurs de requêtes de l'interface distante (5 000 cas).
- Fuzzing ordinaire, à chaque `cargo test` :
  - requêtes de l'interface distante ;
  - manifeste d'un plugin, sortie d'un plugin, cookies posés par un site (2 000 cas
    chacun).
- Essais réels :
  - interface distante depuis un vrai téléphone sur le Wi-Fi (étape 3e) ;
  - plugin d'exemple Internet Archive par la CLI, puis dans l'interface (étape 4), et de
    nouveau après les corrections C1 et C2.

## 6. Porte

Porte de la phase 6 franchie le 2026-11-12, déclarée par l'utilisateur sur ce rapport et sur
la campagne du même jour.

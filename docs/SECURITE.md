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
| Sessions | Cookie aléatoire (empreinte en base), `HttpOnly`, `SameSite=Strict`, `Secure` en TLS, 30 jours ; supprimé avec son jeton | `server_tests` |
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
| Page | Liste blanche de fichiers intégrés ; politique de sécurité stricte ; `X-Frame-Options`, `nosniff`, `no-referrer`, `no-store` ; aucune donnée insérée par `innerHTML` (seulement deux icônes fixes) | `remote::page::tests`, lecture de `ui/src` |
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

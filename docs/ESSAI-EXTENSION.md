# Essai de l'extension dans les navigateurs (porte de la phase 3)

Porte de la phase 3 (SPEC 9.3) : « capture fiable sur Firefox, Chrome et Edge, y compris
sous Manifest V3 ». Cet essai se fait à la main, dans les navigateurs habituels, avec
l'application AuroDownloader installée et ouverte. Compter une demi-heure par navigateur.

Les téléchargements essayés vont dans le dossier des téléchargements de l'application,
comme en usage normal. Les fichiers proposés sont publics (miroir Debian).

## Avant de commencer

- AuroDownloader est installé (version du 2026-11-08 ou plus récente) et ouvert.
- **IDM** : désactiver son module d'intégration dans le navigateur essayé (page des
  extensions, « IDM Integration Module », interrupteur sur arrêt). Sinon IDM prend les
  téléchargements avant AuroDownloader et l'essai ne veut rien dire. Le réactiver après
  si besoin.
- Noter la version du navigateur (menu Aide, « À propos »).

## 1. Installer l'extension

L'extension n'est pas encore publiée dans les boutiques : elle se charge depuis le dossier
`C:\Dev\AuroDownloader\extension`.

**Chrome** (version 121 ou plus) :
1. Ouvrir `chrome://extensions`.
2. Activer « Mode développeur » (en haut à droite).
3. « Charger l'extension non empaquetée », choisir le dossier `extension`.
4. Vérifier l'identifiant affiché sous AuroDownloader : `blpdooihcodaljmhaldncnkgchpaaheo`.
   Un autre identifiant empêcherait la liaison avec l'application : le noter.

**Edge** :
1. Ouvrir `edge://extensions`.
2. Activer « Mode développeur » (en bas à gauche).
3. « Charger l'élément décompressé », choisir le dossier `extension`.
4. Même identifiant attendu : `blpdooihcodaljmhaldncnkgchpaaheo`.

**Firefox** (version 128 ou plus) :
1. Ouvrir `about:debugging#/runtime/this-firefox`.
2. « Charger un module complémentaire temporaire… », choisir le fichier
   `extension\manifest.json`.
3. L'extension reste jusqu'à la fermeture de Firefox (Firefox n'installe durablement que
   des extensions signées) : la recharger à chaque redémarrage.
4. Dans `about:addons`, AuroDownloader, onglet « Permissions » : autoriser l'accès à tous
   les sites. Sans lui, les cookies ne peuvent pas être lus (les options le signalent).

## 2. Liaison avec l'application

Cliquer sur l'icône AuroDownloader de la barre d'outils du navigateur : les options
s'ouvrent (depuis l'étape 5d, l'icône ouvre une fenêtre : y choisir « Réglages »).
Bouton « Vérifier », sous « Liaison avec AuroDownloader ».

- Attendu : « Relais présent (version 0.1.0) ».
- Sinon, noter le message exact.

## 3. Les essais

Pour chaque essai, noter **OK** ou **pas OK**, avec ce qui s'est passé. Quand un envoi
échoue, les options montrent « Dernier envoi refusé… » : recopier ce texte.

| N° | Essai | Ce qu'il faut faire | Résultat attendu |
| --- | --- | --- | --- |
| 1 | Gros fichier, clic | Ouvrir `https://deb.debian.org/debian/dists/bookworm/main/binary-amd64/` et cliquer sur `Packages.xz` (8,4 Mio) | Badge « ✓ » sur l'icône ; le téléchargement disparaît de la liste du navigateur et apparaît dans AuroDownloader ; il se termine ; le fichier fait 8 790 396 octets |
| 2 | Clic avec Alt | Même lien, en tenant la touche Alt pendant le clic | Le navigateur télécharge lui-même ; rien dans AuroDownloader |
| 3 | Menu contextuel | Clic droit sur un lien de la même page (par exemple `Release`), « Télécharger avec AuroDownloader » | Le fichier arrive dans AuroDownloader, même petit |
| 4 | Petit fichier | Ouvrir `https://deb.debian.org/debian/pool/main/h/hello/` et cliquer sur un fichier `.deb` (moins de 1 Mio) | Le navigateur le garde (seuil de 1 Mio). À noter si AuroDownloader le prend quand même : la taille n'est parfois pas connue au départ |
| 5 | Site exclu | Options : mettre `deb.debian.org` dans « Sites exclus », Enregistrer ; refaire l'essai 1 ; puis retirer l'exclusion | Le navigateur garde le téléchargement |
| 6 | Navigation privée | Fenêtre de navigation privée (sous Chrome et Edge, autoriser d'abord l'extension en navigation privée dans sa page de détails), essai 1 | Option « Aussi en navigation privée » décochée : le navigateur garde le fichier. Cochée : AuroDownloader le prend |
| 7 | Site où l'on est connecté | Sur un site où vous êtes connecté (débrideur, espace personnel…), lancer un téléchargement de plus de 1 Mio réservé aux membres | AuroDownloader télécharge le vrai fichier (pas une page de connexion de quelques Kio) ; l'ouvrir pour vérifier |
| 8 | Lien qui redirige | Un lien de téléchargement qui passe par une redirection (bouton « Télécharger » d'un site d'éditeur, par exemple) | AuroDownloader prend le fichier final, avec son vrai nom |
| 9 | Application fermée | Quitter AuroDownloader (menu de la zone de notification, « Quitter » en arrêtant le moteur), refaire l'essai 1 | Badge « ✓ » : le relais redémarre le moteur (sans ouvrir la fenêtre), qui télécharge ; en rouvrant AuroDownloader, la tâche est là. Si le relais n'y arrive pas : badge « ! », le navigateur garde le téléchargement, rien n'est perdu |

## 4. Ce qu'il faut me renvoyer

Pour chaque navigateur :

- sa version ;
- l'identifiant affiché (Chrome, Edge) ;
- le résultat de la liaison (étape 2) ;
- pour chaque essai, OK ou pas OK, et ce qui s'est passé ; les messages exacts en cas
  d'échec.

Une capture d'écran aide pour tout ce qui est inattendu.

## Après l'essai

- Réactiver l'intégration d'IDM si on s'en sert encore.
- L'extension chargée peut rester (Chrome, Edge) ; sous Firefox, elle part à la fermeture.

## Résultat

2026-11-08 : les 9 essais validés par l'utilisateur sous Firefox, Chrome et Edge, ainsi que
CCleaner Browser (Chromium) : « tout fonctionne correctement ». Porte de la phase 3
franchie (SPEC 9.3).

## 5. Étape 5d (phase 5) : liens magnet, fichiers `.torrent`, vidéo de la page

À faire avec une version d'AuroDownloader qui contient la phase 5 (torrents et vidéo) : le
relais et le moteur installés doivent la connaître. Recharger l'extension (page des
extensions, bouton de rechargement ; sous Firefox, la charger de nouveau). Après le
rechargement, un clic sur l'icône ouvre une petite fenêtre (et non plus les options).

Torrents : seulement le netinst Debian. Si AuroDownloader ne tourne pas par le VPN,
choisir son interface dans Options, rubrique BitTorrent, avant l'essai 10. Le premier
torrent peut faire apparaître la fenêtre du pare-feu de Windows. Supprimer chaque torrent
de l'essai une fois vu dans la liste (inutile de le laisser finir). Vidéos : seulement
les pages libres ci-dessous. Les outils vidéo doivent être installés (Options, Vidéo).

| N° | Essai | Ce qu'il faut faire | Résultat attendu |
| --- | --- | --- | --- |
| 10 | Lien magnet, clic | Sur la page d'essai fournie (lien magnet du netinst Debian), cliquer sur le lien | Badge « ✓ » ; un torrent `debian-…-netinst.iso` apparaît dans AuroDownloader ; le navigateur n'ouvre rien d'autre |
| 11 | Lien magnet avec Alt | Même lien, avec Alt | Le navigateur le traite lui-même (il propose d'ouvrir un programme, ou rien) ; rien de nouveau dans AuroDownloader |
| 12 | Option magnet coupée | Options : décocher « Confier les liens magnet à AuroDownloader » ; refaire l'essai 10 ; puis recocher | Comme l'essai 11 |
| 13 | Fichier `.torrent` | Ouvrir `https://cdimage.debian.org/debian-cd/current/amd64/bt-cd/` et cliquer sur le `.torrent` du netinst | Badge « ✓ » ; un torrent (et non un fichier `.torrent`) apparaît dans AuroDownloader ; rien dans la liste du navigateur |
| 14 | Vidéo, fenêtre | Ouvrir `https://framatube.org/w/4Tg9KUhgBQSYB383W7VUKV`, cliquer sur l'icône AuroDownloader | « Lecture de la page… », puis le titre (« Bruce Schneier… »), la durée, les qualités avec leur taille, un sous-titre « anglais (en) ». Choisir une qualité, cocher le sous-titre, « Télécharger » : « Ajoutée à AuroDownloader. », badge « ✓ » ; la vidéo se termine, avec `….en.vtt` à côté |
| 15 | Vidéo, menu contextuel | Ouvrir `https://video.blender.org/w/55bb3q2JuqERvT88aLEYCb`, clic droit sur la page, « Télécharger la vidéo de cette page avec AuroDownloader » | Badge « ✓ » ; la vidéo arrive dans AuroDownloader, en meilleure qualité |
| 16 | Page sans vidéo | Cliquer sur l'icône sur `https://deb.debian.org/debian/`, puis sur la page des extensions du navigateur | Debian : « Aucune vidéo trouvée sur cette page », et « Réessayer ». Page des extensions : « Ce n'est pas une page web » |
| 17 | Réglages | Dans la fenêtre, « Réglages » | Les options s'ouvrent, avec la case des liens magnet |

Pour ces essais, renvoyer aussi les messages exacts de la fenêtre quand ils diffèrent.

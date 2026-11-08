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
s'ouvrent. Bouton « Vérifier », sous « Liaison avec AuroDownloader ».

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

2026-11-08 : les 9 essais validés par l'utilisateur sous Firefox, Chrome et Edge, et un
autre navigateur (non précisé) : « tout fonctionne correctement ». Porte de la phase 3
franchie (SPEC 9.3).

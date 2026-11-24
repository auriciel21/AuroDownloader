# Extension de navigateur

L'extension confie à AuroDownloader les téléchargements du navigateur, les liens magnet et la
vidéo de la page. Elle marche avec Firefox (128 et plus), Chrome (121 et plus) et Edge, avec
l'application de bureau installée (elle passe par son relais, `auro-relay`).

## Installer

L'extension n'est pas encore dans les boutiques des navigateurs ; elle se charge depuis le
dossier `extension` du projet :

- **Chrome** : `chrome://extensions`, « Mode développeur », « Charger l'extension non
  empaquetée », dossier `extension`.
- **Edge** : `edge://extensions`, « Mode développeur », « Charger l'élément décompressé ».
  Edge signale un avertissement sur `background.scripts` (pour Firefox) : il est sans effet.
- **Firefox** : `about:debugging#/runtime/this-firefox`, « Charger un module complémentaire
  temporaire… », fichier `extension/manifest.json` (à recharger à chaque démarrage de
  Firefox). Puis, dans `about:addons`, AuroDownloader, Permissions : autoriser l'accès à tous
  les sites, sans quoi les cookies ne peuvent pas être lus.

Sous Linux, le relais n'est déclaré aux navigateurs que par le paquet `.deb`, pas par
l'AppImage.

Si IDM ou un autre gestionnaire de téléchargement est installé, désactivez son extension :
sinon il prend les téléchargements avant AuroDownloader.

## Ce qu'elle fait

- **Téléchargements** : ceux de plus de 1 Mio (réglable) vont à AuroDownloader. Le navigateur
  garde le sien tant qu'AuroDownloader ne l'a pas accepté ; l'icône montre « ✓ » ou « ! ».
- **Clic avec Alt** : laisse ce téléchargement, ou ce lien magnet, au navigateur.
- **Liens magnet** : un clic les confie à AuroDownloader (option active par défaut). Les
  fichiers `.torrent` téléchargés deviennent des torrents.
- **Menu contextuel** : « Télécharger avec AuroDownloader » sur un lien, une image, une vidéo
  ou un son ; « Télécharger la vidéo de cette page avec AuroDownloader » (meilleure qualité).
- **Fenêtre de l'icône** : la vidéo de la page, avec le choix de la qualité, du son seul et des
  sous-titres, puis « Télécharger ».

## Réglages

Fenêtre de l'icône, « Réglages » :

- Confier les téléchargements à AuroDownloader, seulement à partir d'une taille ;
- Sites exclus (un par ligne ; leurs sous-domaines aussi) ;
- Aussi en navigation privée (téléchargements et liens magnet des fenêtres privées) ;
- **Transmettre les cookies et la page d'origine** : pour les téléchargements qui demandent
  d'être connecté ; AuroDownloader range les cookies dans le coffre du système ;
- Confier les liens magnet à AuroDownloader ;
- **Liaison avec AuroDownloader**, « Vérifier » : « Relais présent » si tout va bien.

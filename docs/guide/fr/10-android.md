# Android

L'application Android contient le même moteur que la version de bureau : téléchargements
HTTP(S), FTP(S), SFTP et torrents, avec la même reprise sûre. La vidéo de sites (yt-dlp) n'y
est pas.

## Ajouter

- **Ajouter**, puis l'adresse : `https://…`, `ftp://…`, `sftp://…` ou un lien magnet.
- **Partager** un lien depuis une autre application (navigateur, messagerie), ou **ouvrir**
  un lien magnet ou un fichier `.torrent` avec AuroDownloader : il est ajouté directement.

Les fichiers vont dans `Download/AuroDownloader`, où les autres applications les voient.

## La liste et les détails

Touchez un téléchargement pour ses détails : statut, taille, débit, temps restant, empreinte ;
Pause, Reprendre, Relancer, Annuler le téléchargement, Retirer de la liste, Ouvrir ;
pour un torrent, le choix de ses fichiers ; pour un serveur SFTP inconnu, l'empreinte de sa
clé et « Accepter la clé… ».

## En arrière-plan

Un téléchargement continue quand l'application est fermée : une notification montre
l'avancement, avec « Tout mettre en pause ». Android peut demander d'autoriser les
notifications au premier téléchargement.

## Réglages

- **Dossier des téléchargements** (affiché : `Download/AuroDownloader`) et **Limite de débit
  globale**.
- **Chercher l'empreinte à côté du fichier**, **Réserver la place de chaque fichier**.
- **Wi-Fi seulement** : téléchargements seulement en Wi-Fi (ou Ethernet), jamais sur les
  données mobiles. Un Wi-Fi facturé, comme le partage de connexion d'un téléphone, compte
  comme du Wi-Fi.
- **Reprendre après le redémarrage de l'appareil** : les téléchargements inachevés
  reprennent, ou une notification propose de les reprendre (Android 14 et plus).
- **BitTorrent** : partage après le téléchargement, limite d'envoi. Rien n'est partagé sur une
  connexion facturée au volume.

Pour passer par un VPN, utilisez celui d'Android : il vaut pour toute l'application.

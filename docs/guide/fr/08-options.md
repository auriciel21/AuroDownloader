# Options

« Options » ouvre les réglages, en rubriques. Les flèches gauche et droite passent d'une
rubrique à l'autre.

## Général

- **Langue** : celle du système, français ou anglais.
- **Démarrer avec la session, dans la zone de notification**.
- **Fermer la fenêtre la replie dans la zone de notification** : sinon, fermer la fenêtre
  quitte l'application (les téléchargements continuent, par le moteur).
- **Afficher la fenêtre au premier plan quand un téléchargement est ajouté** (par le
  navigateur ou la ligne de commande).
- **Afficher la fenêtre au premier plan quand un téléchargement en file démarre** : quand
  une place se libère dans sa file, ou à l'ouverture de sa plage horaire. Pas pour une
  nouvelle tentative après une erreur passagère, ni pour les téléchargements qui reprennent
  à l'ouverture de la session.
- **Proposer d'ajouter les liens copiés** : voir [Analyse de liens](06-liens.md#liens-copiés).
- **Mises à jour** : « Rechercher les mises à jour » (au plus une fois par jour, auprès de
  GitHub, qui voit l'adresse IP et la version utilisée, rien d'autre) et « Rechercher
  maintenant ». Une version nouvelle s'affiche dans « Nouvelle version », avec ses
  nouveautés : « Installer et redémarrer » (Windows, AppImage), « Ouvrir la page de la
  version » (paquet `.deb`), ou « Plus tard ». La mise à jour est vérifiée par sa signature ;
  rien ne s'installe sans votre accord. Pendant l'installation, l'application se ferme, puis
  redémarre, et les téléchargements reprennent.

## Enregistrement

- **Dossier des nouveaux téléchargements**, quand aucun n'est choisi (extension, liens
  copiés, interface distante).
- **Réserver la place de chaque fichier dès le départ** : le disque plein se voit tout de
  suite, et le fichier est moins morcelé.
- **Chercher l'empreinte à côté du fichier (SHA256SUMS, .sha256…)** : quand vous n'en donnez
  pas, AuroDownloader cherche l'empreinte publiée à côté du fichier, et la vérifie.

## Règles

Les [règles de rangement](07-organisation.md#règles-de-rangement) : Nouvelle…, Modifier…,
Monter, Descendre, Désactiver, Supprimer.

## Connexion

- **Limite de débit globale**.
- **Proxy** : celui du système (par défaut), aucun (toujours en direct), ou « Celui-ci » :
  adresse (`http://`, `https://` ou `socks5://`), adresses à joindre en direct, identifiant
  et mot de passe (rangé dans le coffre du système). Les torrents ne passent jamais par le
  proxy ; la recherche des mises à jour prend celui du système.

## BitTorrent

Partage, limite d'envoi, interface réseau (VPN), UPnP, association des liens magnet et des
`.torrent` : voir [Torrents](04-torrents.md#réglages-options-bittorrent).

## Vidéo

Installation de yt-dlp et ffmpeg, et leur mise à jour : voir [Vidéos](05-video.md).

## Identifiants de sites

- **Identifiants** d'un site (Nouveau…, Modifier…, Supprimer) : site, nom d'utilisateur, mot
  de passe ou clé privée SSH (SFTP), « FTP : autoriser sans chiffrement ». Les mots de passe
  vont au coffre du système et ne ressortent jamais.
- **Clés des serveurs SSH** acceptées, avec leur empreinte ; « Oublier la clé ».

## Plugins

Un plugin d'hébergeur transforme la page d'un hébergeur de fichiers en liens directs.

- « Installer un plugin… » : choisissez un fichier `.wasm`. Avant l'installation,
  AuroDownloader montre son nom, son auteur, les pages qu'il prend en charge, les domaines
  qu'il contactera et son empreinte : n'installez que des plugins de confiance.
- Un plugin tourne isolé : il ne voit ni vos fichiers, ni vos mots de passe, ni vos cookies,
  et ne contacte que ses domaines déclarés.
- Désactiver, Retirer. Une nouvelle version qui demande plus de domaines redemande votre
  accord.

## Accès à distance

Voir [Accès à distance](11-acces-distant.md).

## À propos

- La version d'AuroDownloader.
- **Détail des journaux** : Erreurs seulement, Erreurs et avertissements, Ordinaire
  (conseillé), Détaillé, pour un dépannage.
- **Exporter un diagnostic…** : voir [Dépannage](14-depannage.md#diagnostic).
- **Quitter et arrêter les téléchargements**.

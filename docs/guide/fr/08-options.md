# Options

« Options » ouvre les réglages, en rubriques. Les flèches gauche et droite passent d'une
rubrique à l'autre.

## Général

- **Langue** : « Langue du système », Français ou English ; la fenêtre change aussitôt.
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
  « Nouveautés » : « Installer et redémarrer » (Windows, AppImage), « Ouvrir la page de la
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

- **Limite de débit globale** : la même que dans la barre d'état.
- **Proxy** : « Celui du système » (par défaut), « Aucun (toujours en direct) », ou
  « Celui-ci : » avec :
  - **Adresse** : `http://hôte:port`, `https://hôte:port` ou `socks5://hôte:port` ;
  - **En direct pour (séparés par des virgules)** : les hôtes joints sans proxy
    (`localhost, nas.maison`) ;
  - **Identifiant (facultatif)** et **Mot de passe** (rangé dans le coffre du système) ;
  - **Appliquer** : enregistre le proxy.

  Les torrents ne passent jamais par le proxy ; la recherche des mises à jour prend celui du
  système.

## BitTorrent

- **Après le téléchargement, partager** : voir [Partage](04-torrents.md#partage).
- **Limite d'envoi** : le débit d'envoi aux autres pairs.
- **Interface réseau des torrents (VPN)** : « Actualiser la liste » relit les interfaces de
  l'ordinateur.
- **Ouvrir le port d'écoute sur le routeur (UPnP)**.
- **Ouvrir les liens magnet et les fichiers .torrent avec AuroDownloader** : l'association,
  désactivée par défaut.

Le détail : [Torrents](04-torrents.md#réglages-options-bittorrent).

## Vidéo

- La version de yt-dlp et de ffmpeg ; « Installer yt-dlp et ffmpeg » tant qu'ils manquent,
  puis « Réinstaller » pour les télécharger de nouveau (par exemple s'ils sont abîmés).
- **Mettre à jour yt-dlp de lui-même** : au plus une fois par jour, signature vérifiée.

Le détail : [Vidéos](05-video.md).

## Identifiants de sites

- **Identifiants** d'un site (Nouveau…, Modifier…, Supprimer), dans la boîte « Identifiants
  d'un site » : **Site**, **Nom d'utilisateur**, **Mot de passe**, **Clé privée SSH (SFTP,
  facultative)** (« Parcourir… »), et « FTP : autoriser sans chiffrement ». Les mots de passe
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

- **Piloter les téléchargements depuis un autre appareil**, **Adresse d'écoute**
  (« Actualiser la liste ») et **Port**.
- Le certificat en place et son empreinte ; « Utiliser mon certificat… » (boîte « Certificat
  fourni » : « Certificat (chaîne comprise) » et « Clé privée », puis « Utiliser ») ;
  « Revenir au certificat d'AuroDownloader ».
- **Appareils** : chacun avec ses droits et sa dernière utilisation ; « Ajouter un
  appareil… », « Retirer ».

Le détail : [Accès à distance](11-acces-distant.md).

## À propos

- La version d'AuroDownloader.
- **Détail des journaux** : Erreurs seulement, Erreurs et avertissements, Ordinaire
  (conseillé), Détaillé, pour un dépannage.
- **Exporter un diagnostic…** : voir [Dépannage](14-depannage.md#diagnostic).
- **Quitter et arrêter les téléchargements**.

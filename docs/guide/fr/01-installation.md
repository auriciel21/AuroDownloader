# Installation

Les paquets sont sur la page des
[publications](https://github.com/aurodownloader/aurodownloader/releases) du projet. Chaque
publication donne aussi `SHA256SUMS`, l'empreinte de chaque fichier, pour vérifier un
téléchargement.

## Windows 10 et 11

1. Téléchargez `AuroDownloader_<version>_x64-setup.exe` (la plupart des ordinateurs) ou
   `_arm64-setup.exe` (ordinateurs à processeur ARM).
2. Lancez-le. L'installation se fait pour votre seul compte, sans droits d'administrateur,
   dans `%LOCALAPPDATA%\AuroDownloader`. Si WebView2 manque (rare sous Windows 11),
   l'installateur le télécharge.
3. Si Windows affiche « Windows a protégé votre ordinateur » et « éditeur inconnu » :
   l'installateur n'est pas signé. Choisissez « Informations complémentaires », puis
   « Exécuter quand même », si vous l'avez bien pris sur la page des publications.

L'installateur déclare aussi le relais de l'[extension](09-extension.md) aux navigateurs.

## Linux

Il faut WebKitGTK 4.1 : Ubuntu 22.04, Debian 12 ou plus récents.

- **Paquet `.deb`** (Debian, Ubuntu et dérivées) :

  ```
  sudo apt install ./AuroDownloader_<version>_amd64.deb
  ```

  (`_arm64.deb` sur un ordinateur ARM). Il déclare le relais de l'extension aux navigateurs
  et installe les pages de manuel (`man auro`).
- **AppImage** : rendez le fichier exécutable (`chmod +x AuroDownloader_<version>_amd64.AppImage`)
  et lancez-le. L'AppImage se met à jour elle-même, sur accord ; elle ne déclare pas le relais
  de l'extension.

## Android 8 et plus

Téléchargez `AuroDownloader_<version>_arm64.apk` sur l'appareil et ouvrez-le. Android demande
d'autoriser l'installation depuis cette source (le navigateur ou le gestionnaire de fichiers).
Voir [Android](10-android.md).

## Extension de navigateur

Voir [Extension de navigateur](09-extension.md).

## Mises à jour

L'application de bureau cherche ses mises à jour au plus une fois par jour, et propose une
version nouvelle ; rien ne s'installe sans votre accord (Options, Général, « Mises à jour »).
Le paquet `.deb` ne fait que prévenir, avec le lien de la page de la version : installez le
nouveau paquet comme le premier. Sous Android, installez le nouvel APK par-dessus l'ancien.

## Désinstallation

- **Windows** : Paramètres, Applications, AuroDownloader, Désinstaller. Vos téléchargements
  ne sont jamais touchés. La liste des téléchargements, l'historique et les préférences sont
  gardés, sauf si vous cochez « supprimer les données » : elle efface aussi les journaux, les
  plugins, les outils de la vidéo, le certificat de l'accès à distance, et les cookies et mots
  de passe rangés dans le coffre de Windows.
- **Linux** : `sudo apt remove auro-downloader` pour le `.deb` ; supprimez simplement le
  fichier AppImage.
- **Android** : comme toute application. Les fichiers déjà téléchargés restent dans
  `Download/AuroDownloader`.

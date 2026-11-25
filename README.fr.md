# AuroDownloader

**Gestionnaire de téléchargement libre et gratuit pour Windows, Linux et Android.**

[English version](README.md)

Un moteur de téléchargement unique et fiable, dans son propre processus, piloté par une API
JSON-RPC, avec par-dessus une application de bureau, une application Android, une interface
web, une ligne de commande et une extension de navigateur. Aucune ne contient de logique de
téléchargement : toutes parlent au même moteur.

> **État : version 1 en préparation.** Rien n'est encore publié. Les fonctions ci-dessous
> sont écrites et testées ; les paquets seront publiés sur la page des
> [publications](https://github.com/auriciel21/AuroDownloader/releases).

## Fonctions

- **Téléchargements rapides et sûrs** : HTTP et HTTPS découpés en segments sur plusieurs
  connexions, repris après un plantage ou une coupure de courant sans perdre ni abîmer un
  octet ; FTP, FTPS et SFTP (clé du serveur confirmée).
- **Intégrité** : empreinte attendue (SHA-256, SHA-1, MD5), fichiers d'empreintes trouvés à
  côté du téléchargement, morceaux faux d'un Metalink téléchargés de nouveau.
- **Miroirs et Metalink** : le même fichier depuis plusieurs serveurs, réparti selon leur
  débit, avec bascule automatique.
- **BitTorrent** : liens magnet et fichiers `.torrent`, choix des fichiers, partage jusqu'à
  un ratio, liaison facultative à une interface réseau (VPN).
- **Vidéo** (Windows et Linux) : pages de sites de vidéos par yt-dlp et ffmpeg, installés à
  la demande et vérifiés ; qualité, format et sous-titres.
- **Organisation** : files et priorités, limites de débit (globale, par file, par
  téléchargement), plages horaires avec veille ou extinction à la fin, règles qui rangent
  les téléchargements dans des dossiers et les renomment, commande lancée à la fin d'un
  téléchargement.
- **Analyse de liens** : tous les liens de fichiers d'une page web ou d'une liste, filtrés
  par type et par taille.
- **Extension de navigateur** pour Firefox, Chrome et Edge : téléchargements, liens magnet et
  vidéo de la page, confiés à AuroDownloader.
- **Interface web distante** : la même fenêtre depuis un téléphone ou un autre ordinateur,
  avec jetons d'accès et TLS.
- **Plugins d'hébergeurs** : des plugins WebAssembly, isolés et limités à leurs sites
  déclarés, transforment la page d'un hébergeur de fichiers en liens directs.
- **Proxy** : celui du système ou un proxy choisi, HTTP, HTTPS ou SOCKS5.
- **Accessibilité** : clavier et lecteurs d'écran, essayés avec NVDA, Orca et TalkBack
  (revue WCAG 2.2 AA dans [`docs/ACCESSIBILITE.md`](docs/ACCESSIBILITE.md)).
- **Vie privée** : aucune télémétrie. L'application de bureau cherche ses mises à jour au
  plus une fois par jour sur GitHub (qui voit l'adresse IP et la version), n'installe rien
  sans votre accord, et cette recherche se coupe.
- **Langues** : français et anglais.

## Installation

Une fois la version 1 sortie, depuis la page des
[publications](https://github.com/auriciel21/AuroDownloader/releases) :

| Système | Paquet |
| --- | --- |
| Windows 10 et 11 (x64, ARM64) | `AuroDownloader_<version>_x64-setup.exe` ou `_arm64-setup.exe`, installé pour l'utilisateur courant |
| Linux (x86_64, aarch64) | paquet `.deb` ou `.AppImage` (WebKitGTK 4.1 : Ubuntu 22.04, Debian 12 ou plus récents) |
| Android 8 et plus (arm64) | `AuroDownloader_<version>_arm64.apk` |

L'extension de navigateur n'est pas encore dans les boutiques des navigateurs ; elle se
charge depuis le dossier `extension`.

`SHA256SUMS` donne l'empreinte de chaque fichier d'une publication. Si l'installateur
Windows n'est pas signé, SmartScreen avertit d'un « éditeur inconnu ».

## Documentation

- Guide de l'utilisateur : [`docs/guide/fr`](docs/guide/fr) (français),
  [`docs/guide/en`](docs/guide/en) (anglais) ; l'application de bureau en contient une
  copie.
- Pages de manuel de la ligne de commande : [`docs/man`](docs/man), installées par le paquet
  `.deb` (`man auro`, `man auro-add`, `man aurod`).
- Spécification, protocole du moteur et décisions de conception :
  [`docs/SPEC.md`](docs/SPEC.md), [`docs/protocol`](docs/protocol).

## Compiler depuis les sources

Il faut la chaîne Rust stable (1.88 ou plus récente). Sous Windows, les outils de
compilation C++ de Visual Studio (Build Tools) : SQLite est compilé avec le projet. Sous
Linux, pour l'application de bureau, WebKitGTK 4.1 et ses fichiers de développement
(`libwebkit2gtk-4.1-dev` et les paquets listés dans
[`.github/workflows/ci.yml`](.github/workflows/ci.yml)).

```
cargo test --workspace
cargo run -p auro-cli -- get https://deb.debian.org/debian/README -o .
```

L'application de bureau est un espace de travail Cargo à part :

```
cd apps/desktop
cargo test
```

Les installateurs, l'application Android, les bancs d'essai et toutes les autres commandes
sont décrits dans [`CLAUDE.md`](CLAUDE.md), et la procédure de publication dans
[`docs/PUBLICATION.md`](docs/PUBLICATION.md).

## Organisation du dépôt

| Dossier | Contenu |
| --- | --- |
| `crates/engine` | Le moteur (`auro-engine`) : stockage, segmentation, réseau, files, API |
| `crates/daemon` | Le moteur en processus autonome, `aurod` |
| `crates/cli` | La ligne de commande `auro` et le relais de l'extension `auro-relay` |
| `crates/android` | Le moteur dans l'application Android |
| `apps/desktop` | L'application de bureau (Tauri 2) |
| `apps/android` | L'application Android (Kotlin, Jetpack Compose) |
| `ui` | La page de l'application de bureau, servie aussi comme interface web distante |
| `extension` | L'extension de navigateur |
| `plugins` | Plugin d'hébergeur d'exemple et guide de l'auteur de plugins |
| `docs` | Spécification, protocole, guide, pages de manuel, revues et mesures |

## Sécurité

Merci de signaler les failles en privé, par les
[avis de sécurité](https://github.com/auriciel21/AuroDownloader/security/advisories/new)
de GitHub, et non dans les tickets publics. La revue de sécurité de l'interface distante, des
plugins et du canal local est dans [`docs/SECURITE.md`](docs/SECURITE.md).

## Licence

AuroDownloader est un logiciel libre, sous la licence publique générale GNU, version 3 ou (à
votre choix) toute version ultérieure : voir [`LICENSE`](LICENSE).

# Prototype torrent : librqbit face à libtorrent-rasterbar

Phase 5, étape 2 (2026-11-09). La SPEC (4.2, 9.2) laisse le choix de la bibliothèque de
torrent à un prototype ; le module `torrent` du moteur la cachera derrière une interface
interne. Ce dossier ne fait pas partie du produit.

- `rqbit/` : librqbit 9.1.0 (Rust, Apache-2.0), variante `rust-tls` (rustls et aws-lc,
  sans OpenSSL). Projet Cargo hors de l'espace de travail.
- `libtorrent/lt_proto.py` : libtorrent 2.1.1 (C++, BSD) par ses liaisons Python
  officielles (`pip install libtorrent==2.1.1`). Tout le travail se fait dans le code C++ ;
  Python ne fait que piloter. Le coût d'une liaison depuis Rust s'estime à part.
- `run.sh <rqbit|lt> <nom> [fraction] [corrupt]` : un téléchargement sous
  `/usr/bin/time -v`. Avec une fraction, il tue l'outil par `kill -9` à cette part du
  téléchargement, puis le relance. Avec `corrupt`, il efface en plus 4 Mio à 64 Mio avant
  la relance : c'est l'effet d'écritures perdues dans une coupure de courant.
- `magnet.sh <rqbit|lt> <nom>` : la même chose par un lien magnet sans tracker (DHT seul).

Les deux prototypes ont les mêmes réglages :
- envoi limité à 100 Kio/s ;
- aucun port ouvert sur le routeur (ni UPnP ni NAT-PMP) ;
- pas de découverte sur le réseau local ;
- reprise d'un lancement à l'autre ;
- arrêt dès la fin du téléchargement, sans partager ensuite.

## Conditions

- **Torrent** : `debian-13.7.0-amd64-netinst.iso` (792 723 456 octets, morceaux de
  256 Kio). Empreinte publiée par Debian : `a7ef94ac…e355`, vérifiée après chaque essai.
- **Machine et réseau** : WSL 2, distribution Ubuntu 24.04, système de fichiers Linux,
  derrière la traduction d'adresses de WSL, donc sans connexion entrante. Les mesures sont
  faites sous WSL pour éviter, sous Windows, la fenêtre du pare-feu que provoque un port
  d'écoute.
- **Débit** : celui de l'essaim Debian à ce moment-là. Il varie d'un passage à l'autre :
  trois passages par outil, en alternant les outils.
- **Données envoyées** : rien dans tous les passages, sauf 96 Kio pour libtorrent pendant
  une reprise.

## Mesures

Téléchargement complet par le fichier `.torrent`. Le temps processeur est la somme du
temps utilisateur et du temps système ; la mémoire est le maximum résident. Chaque
colonne d'outil donne la médiane de trois passages, puis le détail des trois.

| Grandeur | librqbit | libtorrent |
| --- | --- | --- |
| Durée | **16,2 s** (14,1 ; 16,2 ; 19,1) | **44,3 s** (38,5 ; 45,7 ; 44,3) |
| Débit moyen | 47 Mio/s | 17 Mio/s |
| Temps processeur | **22,3 s** (22,3 ; 29,1 ; 21,8), surtout du temps système | **16,5 s** (14,8 ; 17,3 ; 16,5) |
| Processeur par Gio | 30 s | 22 s |
| Mémoire | **31 Mo** | **55 Mo** (dont environ 10 Mo pour Python) |

Le premier passage de libtorrent a duré 6 minutes, à 2–4 Mo/s. La cause : par défaut,
`rate_limit_ip_overhead` compte le surcoût IP de la réception (les accusés de réception
TCP) dans la limite d'envoi. Avec une limite de 100 Kio/s, c'est donc le téléchargement
qui était freiné. Le réglage est coupé dans le prototype ; une intégration devra faire de
même.

Les réglages par défaut de libtorrent montent en régime plus lentement. Ses durées
pourraient sans doute baisser avec des réglages ajustés, ce que je n'ai pas essayé. Le
débit n'est donc pas un critère décisif ; le processeur, lui, est à son avantage.

### Lien magnet (DHT seul, sans tracker)

| | librqbit | libtorrent |
| --- | --- | --- |
| Métadonnées reçues | vers 1 s | vers 3 s |
| Durée totale | 28,0 s | 45,7 s |
| Empreinte | juste | juste |

### Arrêt brutal (`kill -9` à 40 %), puis relance

| | librqbit | libtorrent |
| --- | --- | --- |
| Reçu au moment de l'arrêt | ≥ 350,0 Mo | ≥ 324,9 Mo |
| Gardé à la relance | 352,3 Mo, tout | 171,7 Mo : ce qui suit ses dernières données de reprise (enregistrées toutes les 5 s) est retéléchargé |
| Durée de la relance | 1 s, puis 12,0 s jusqu'à la fin | 34,3 s jusqu'à la fin |
| Empreinte finale | juste | juste |

Un `kill -9` laisse les données écrites dans le cache du système : il ne montre pas ce
qu'une coupure de courant ferait perdre.

### Écritures perdues : arrêt brutal, 4 Mio effacés, puis relance

**Les deux bibliothèques finissent avec un fichier faux, tout en le déclarant terminé.**
librqbit donne `96d69b45…`, libtorrent `31254278…`.

- **librqbit** vide sur disque sa liste des morceaux reçus (`bitv.rs`, `sync_all`), mais
  jamais les données du torrent. Au redémarrage, il ne revérifie qu'un échantillon tiré au
  hasard des morceaux qu'il dit avoir (`initializing.rs`, `validate_fastresume`).
- **libtorrent** fait confiance à ses données de reprise sans revérifier les morceaux.
- **Aucune des deux** ne revérifie le fichier entier à la fin.

Conséquence pour le moteur, quelle que soit la bibliothèque. La règle 1 de la SPEC (5.6 :
octets vidés sur disque avant d'être comptés) et l'intégrité (7.9) devront venir de notre
code :
- le stockage du torrent passe par le module `io` (écritures positionnées, `SyncGate`) ;
- la liste des morceaux reçus ne s'enregistre en base qu'après le vidage des données ;
- tous les morceaux sont revérifiés à la fin, dans un état `verifying` comme pour F-11.

### Android

- **librqbit** : le prototype se compile pour `aarch64-linux-android` (NDK 27, API 26)
  sans aucune modification, avec la configuration de l'application. Lancé sur le Galaxy
  S25 Ultra (Android 17) depuis `/data/local/tmp`, il a téléchargé le même fichier en
  23 s en Wi-Fi, empreinte juste.
- **libtorrent** : pas essayé. Il faudrait compiler Boost, OpenSSL et libtorrent par le
  NDK, plus une liaison C ou C++ vers Rust. D'autres le font : libtorrent4j fournit des
  binaires Android à LibreTorrent, mais ce n'est pas vérifié ici.

## Comparaison

| Critère | librqbit 9.1.0 | libtorrent 2.1.1 |
| --- | --- | --- |
| Langage, intégration | Rust, une dépendance Cargo | C++ et Boost : une liaison à écrire et à maintenir (cxx ou C) |
| Licence | Apache-2.0 ; tout son arbre passe notre `deny.toml` sans ajout | BSD (OpenSSL et Boost : licences compatibles) |
| Pile partagée avec le moteur | tokio, reqwest, rustls, aws-lc | aucune |
| Taille | 12,2 Mo, binaire complet sans symboles (tokio, reqwest et rustls compris, déjà dans le moteur) | 25,8 Mo, module Python (bibliothèque liée statiquement) |
| Android | compilé et essayé sur un appareil | non essayé |
| Stockage remplaçable | trait Rust `TorrentStorage` (`pread_exact`, `pwrite_all`, longueur des fichiers) | `disk_interface` en C++, au travers de la liaison |
| État de reprise remplaçable | traits Rust `SessionPersistenceStore` et `BitVFactory` | données de reprise sérialisées, gérées par nous |
| Magnet, DHT, PEX, trackers, choix des fichiers, limites de débit | oui | oui |
| Chiffrement du protocole (MSE/PE) | **non** | oui |
| Sources web (BEP 19, `url-list`) | **non** | oui |
| Torrents v2 (BEP 52) | **non** | oui |
| uTP | expérimental (TCP seul par défaut) | oui |
| Liaison à une interface réseau (VPN, SPEC 8.1) | Linux et macOS seulement ; **pas sous Windows** (proxy SOCKS possible) | toutes plateformes |
| Maturité | jeune, un mainteneur principal, versions fréquentes | référence (qBittorrent, Deluge), longue histoire |

## Recommandation

Je recommande **librqbit**, derrière l'interface interne du module `torrent`. Ce qui
penche pour lui :
- il s'intègre en Rust, sur la même pile que le moteur ;
- il fonctionne sous Android sans travail supplémentaire, ce qui est vérifié ;
- son stockage et son état de reprise se remplacent par des traits Rust, ce qu'exige
  notre règle d'écriture, de toute façon à fournir nous-mêmes ;
- il est plus rapide et plus léger en mémoire dans ces essais.

Ses manques, à arbitrer :
- **Pas de liaison à une interface sous Windows.** C'est la parade de la SPEC (8.1) à
  l'exposition de l'adresse IP. Il faut soit l'ajouter (`IP_UNICAST_IF`, contribution au
  projet ou correctif local), soit passer par un proxy SOCKS.
- **Pas de chiffrement du protocole.** Certains fournisseurs d'accès freinent le trafic
  BitTorrent non chiffré, et certains pairs l'exigent.
- **Ni sources web ni torrents v2.** Pour la v1, c'est acceptable à mon sens.
- **Un seul mainteneur principal.** L'interface interne permet de changer de bibliothèque
  (SPEC 9.2).
- **Plus de processeur** : 30 s par Gio, contre 22 s pour libtorrent.

libtorrent est plus complet et plus économe en processeur. En revanche, il impose une
liaison C++ à écrire et à maintenir, une chaîne de compilation C++ et Boost pour Android
et Windows, et un stockage personnalisé en C++ au travers de cette liaison.

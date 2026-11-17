# Prototype FTP, FTPS et SFTP

Phase 7, étape 6a (F-08, SPEC 7.22), décisions de l'utilisateur du 2026-11-17 : bibliothèques
Rust pures, essayées face à de vrais serveurs avant d'écrire le module du moteur. Ne fait pas
partie du produit ; espace de travail Cargo à part.

## Ce qui est essayé

- `suppaftp` 13.0.0 (fonctions `tokio-rustls-aws-lc-rs` et `deprecated`, pour le TLS
  implicite) : FTP, FTPS explicite (`AUTH TLS`) et implicite (`ftps://`, port 990), mode
  binaire, `SIZE`, reprise par `REST`, arrêt d'une plage par `ABOR`. Certificats vérifiés
  par `rustls-platform-verifier`, comme le moteur.
- `russh` 0.65.2 et `russh-sftp` 3.0.1 : SFTP par mot de passe ou clé privée (ed25519),
  clé du serveur montrée par son empreinte SHA-256 et refusée sauf si elle est acceptée.
- `libunftp` 0.25.0 et `unftp-sbe-fs` 0.4.1 : serveur FTP local du banc (commande
  `serve-ftp`), candidat pour les tests du moteur.
- Un fichier est téléchargé sur N connexions, une plage chacune, écrite à sa place ;
  empreinte SHA-256 vérifiée.

```
cargo build --release
target/release/proto ftp  ftp://[user[:pass]@]hôte/chemin [-c N] [-o fichier] [--tls|--implicit] [--sha256 HEX]
target/release/proto sftp sftp://user[:pass]@hôte/chemin  [-c N] [-o fichier] [--key fichier] [--accept SHA256:…]
target/release/proto serve-ftp <dossier> [127.0.0.1:2121]
```

Variables : `AURO_PASV=1` (mode `PASV` au lieu d'`EPSV`), `AURO_SEP=1` (SFTP : une connexion
SSH par plage). Bancs locaux : `ftp-banc.sh` et `sftp-banc.sh` du bloc-notes de la session
(WSL Ubuntu : `sshd` lancé en simple utilisateur sur 127.0.0.1:2222 avec des clés d'essai,
serveur `serve-ftp` sur 127.0.0.1:2121, fichier aléatoire de 512 Mio).

## Essais réels (2026-11-17, Windows)

| Serveur | Mode | Résultat |
| --- | --- | --- |
| `test.rebex.net` (démonstration publique, compte `demo`) | FTP, 2 connexions | juste |
| | FTPS explicite, 2 connexions | juste |
| | FTPS implicite, 2 connexions | juste après correction (ci-dessous) |
| | SFTP, mot de passe, 1 et 2 connexions | juste ; clé `ssh-ed25519 SHA256:d7Te2D…` refusée tant qu'elle n'est pas acceptée |
| `ftp.gnu.org`, anonyme | FTP, 2 connexions | juste après le passage à `EPSV` |
| `ftp.acc.umu.se`, anonyme, 52 Mio | FTP, 1 à 3 connexions | 4,4, 5,4 et 5,6 Mio/s ; 4 connexions : refus `421 too many connections from your internet address` |

Les cinq modes sur Rebex donnent le même fichier (même empreinte), de même 1, 2 et 3
connexions sur `ftp.acc.umu.se`.

## Mesures locales (2026-11-17, WSL Ubuntu, système de fichiers Linux, 512 Mio, 3 passages)

| | 1 connexion | 2 | 4 |
| --- | --- | --- | --- |
| FTP, suppaftp, serveur libunftp | 54 à 59 Mio/s | 189 à 207 | 304 à 357 |
| FTP, `curl` (référence) | 50 à 62 Mio/s | | |
| SFTP, russh, plages sur une seule connexion SSH | 96 à 106 Mio/s | 100 à 106 | 96 à 106 |
| SFTP, russh, une connexion SSH par plage | 93 à 103 Mio/s | 157 à 171 | 200 à 219 |
| SFTP, client `sftp` d'OpenSSH (référence) | 92 à 112 Mio/s | | |

Empreinte SHA-256 juste à chaque passage.

## Ce qu'il en ressort pour le moteur

- **Bibliothèques** : les trois tiennent. suppaftp sur une connexion vaut `curl` ; russh-sftp
  vaut le client d'OpenSSH. Licences : `cargo deny check licenses` avec le `deny.toml` du
  projet, toutes acceptées (MIT, Apache-2.0).
- **EPSV d'abord** (RFC 2428) : `PASV` n'annonce qu'une adresse IPv4 ; sur un serveur joint
  en IPv6 (GNU, Umeå), la connexion de données échoue (`os error 10049`). Repli sur `PASV`,
  adresse prise de la connexion de commande (serveurs derrière une traduction d'adresses).
- **TLS implicite** : suppaftp n'envoie pas `PBSZ 0` et `PROT P` après la connexion ; sans
  eux, la connexion de données part en clair et TLS échoue (`InvalidContentType`). Le moteur
  les envoie lui-même.
- **Connexions** : les serveurs publics limitent par adresse (3 à Umeå) et répondent `421` :
  le plafond de 4 et la descente sur refus (décisions de l'utilisateur) sont nécessaires ;
  `421` doit compter comme un refus, comme le `503` de HTTP (SPEC 7.3).
- **SFTP** : une connexion SSH par segment (pas des canaux sur une même connexion : un seul
  chiffrement, pas de gain).
- **Arrêt d'une plage** : `ABOR` fonctionne sur les trois serveurs FTP ; la dernière plage
  finit normalement (`226`).
- **Proxy SOCKS5** : possible des deux côtés : russh prend un flux déjà ouvert
  (`connect_stream`), suppaftp une connexion de commande (`connect_with_stream`) et un
  constructeur des connexions de données (`passive_stream_builder`), qui peuvent être des
  tunnels SOCKS5 ouverts par le moteur.
- **Version de Rust** : suppaftp 10 et plus demande Rust 1.88 (la 9, 1.85.1) ; le projet
  déclare 1.85. À décider avant l'intégration.

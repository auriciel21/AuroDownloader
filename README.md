# AuroDownloader

Gestionnaire de téléchargement libre et gratuit pour Windows, Linux et Android : un moteur
unique et fiable, piloté par une API JSON-RPC, avec une application de bureau, une
application Android, une interface web, une ligne de commande et une extension de navigateur.

**État : phase 1 (moteur minimal).** Le stockage SQLite et ses migrations sont en place ;
le moteur de téléchargement lui-même reste à écrire.

La spécification complète est dans [`docs/SPEC.md`](docs/SPEC.md).

## Compiler et tester

Il faut Rust 1.85 ou plus récent (`rust-toolchain.toml` demande la version stable). Sous
Windows, l'outil de compilation C de Visual Studio (Build Tools) est nécessaire : SQLite est
compilé avec le projet.

```
cargo test --workspace
cargo run -p auro-cli -- db migrate auro.db
```

## Organisation

| Dossier | Contenu |
| --- | --- |
| `crates/engine` | Le moteur (`auro-engine`) : stockage, puis segmentation, réseau, files, API |
| `crates/cli` | La ligne de commande `auro` |
| `docs` | La spécification |

## Licence

À confirmer avant la première publication (la spécification propose la GPL-3.0 ou
ultérieure, section 3, exigence N-11).

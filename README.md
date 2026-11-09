# AuroDownloader

Gestionnaire de téléchargement libre et gratuit pour Windows, Linux et Android : un moteur
unique et fiable, piloté par une API JSON-RPC, avec une application de bureau, une
application Android, une interface web, une ligne de commande et une extension de navigateur.

**État : phase 5 (vidéo et torrent), commencée le 2026-11-09.** Les phases 1 à 4 sont
franchies : moteur segmenté avec reprise sûre, application de bureau Windows et Linux,
extension pour Firefox, Chrome et Edge, application Android.

La spécification complète est dans [`docs/SPEC.md`](docs/SPEC.md).

## Compiler et tester

Il faut Rust 1.85 ou plus récent (`rust-toolchain.toml` demande la version stable). Sous
Windows, l'outil de compilation C de Visual Studio (Build Tools) est nécessaire : SQLite est
compilé avec le projet.

```
cargo test --workspace
cargo deny check licenses
cargo run -p auro-cli -- get https://deb.debian.org/debian/README -o .
```

Les autres commandes (application de bureau, application Android, installateurs, bancs
d'essai) sont décrites dans [`CLAUDE.md`](CLAUDE.md).

## Organisation

| Dossier | Contenu |
| --- | --- |
| `crates/engine` | Le moteur (`auro-engine`) : stockage, segmentation, réseau, files, API |
| `crates/daemon` | Le moteur en processus autonome, `aurod` |
| `crates/cli` | La ligne de commande `auro` et le relais de l'extension `auro-relay` |
| `crates/android` | Le moteur dans l'application Android |
| `apps/desktop` | L'application de bureau (Tauri) |
| `apps/android` | L'application Android (Kotlin, Jetpack Compose) |
| `ui` | La page de l'application de bureau |
| `extension` | L'extension de navigateur |
| `docs` | La spécification et les relevés de mesures |

## Licence

GPL-3.0 ou ultérieure ([`LICENSE`](LICENSE)). Chaque dépendance Rust est contrôlée par
`cargo deny check licenses` ([`deny.toml`](deny.toml)) : seules les licences compatibles
avec la GPL-3.0 sont admises. yt-dlp et ffmpeg, prévus pour la vidéo, seront appelés comme
programmes séparés, sans être liés au projet (SPEC, section 8.3).

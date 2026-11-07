#!/bin/sh
# Paquets Linux (.deb et AppImage, non signés ; SPEC 4.6).
#
# Construit `aurod` et `auro` (espace de travail du moteur), les place dans `binaries/`
# sous le nom que Tauri attend (`<nom>-<triplet>`), puis l'application et ses paquets.
# `tauri.bundle.json` ajoute ces deux binaires à la configuration : ils ne sont pas dans
# `tauri.conf.json`, sinon `cargo build` et `cargo test` exigeraient qu'ils existent.
# Demande WebKitGTK 4.1 (Ubuntu 22.04 et plus) et
# `cargo install tauri-cli --version "^2" --locked`.
#
# Résultat : target/release/bundle/deb/*.deb et target/release/bundle/appimage/*.AppImage
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
triple=$(rustc -vV | sed -n 's/^host: //p')

cargo build --release --manifest-path "$root/Cargo.toml" -p auro-daemon -p auro-cli
mkdir -p "$here/binaries"
for name in aurod auro; do
  cp "$root/target/release/$name" "$here/binaries/$name-$triple"
done

cd "$here"
cargo tauri build --bundles deb,appimage --config tauri.bundle.json

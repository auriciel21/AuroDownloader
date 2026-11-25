#!/bin/sh
# Version d'AuroDownloader (phase 7, étape 8c ; docs/PUBLICATION.md).
#
#   scripts/version.sh             affiche la version de chaque fichier
#   scripts/version.sh 0.2.0       la règle partout (puis : Cargo.lock par cargo, commit)
#   scripts/version.sh --check 0.2.0
#                                  vérifie que tout porte cette version (CI de publication)
#
# Fichiers : Cargo.toml (espace de travail), apps/desktop/Cargo.toml,
# apps/desktop/tauri.conf.json, extension/manifest.json, apps/android/app/build.gradle.kts
# (versionName, et versionCode = majeur × 10000 + mineur × 100 + correctif).
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)

workspace() { sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' "$root/Cargo.toml"; }
desktop() { sed -n 's/^version = "\(.*\)"/\1/p' "$root/apps/desktop/Cargo.toml" | head -1; }
tauri() { sed -n 's/^  "version": "\(.*\)",$/\1/p' "$root/apps/desktop/tauri.conf.json"; }
extension() { sed -n 's/^  "version": "\(.*\)",$/\1/p' "$root/extension/manifest.json"; }
android() { sed -n 's/^ *versionName = "\(.*\)"$/\1/p' "$root/apps/android/app/build.gradle.kts"; }
android_code() { sed -n 's/^ *versionCode = \([0-9]*\)$/\1/p' "$root/apps/android/app/build.gradle.kts"; }

show() {
  echo "Cargo.toml (espace de travail) : $(workspace)"
  echo "apps/desktop/Cargo.toml        : $(desktop)"
  echo "tauri.conf.json                : $(tauri)"
  echo "extension/manifest.json        : $(extension)"
  echo "Android versionName            : $(android) (versionCode $(android_code))"
}

valid() {
  echo "$1" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || { echo "version invalide : « $1 » (attendu : 0.2.0)" >&2; exit 2; }
}

code_of() {
  major=${1%%.*}; rest=${1#*.}; minor=${rest%%.*}; patch=${rest#*.}
  [ "$minor" -lt 100 ] && [ "$patch" -lt 100 ] || { echo "mineur et correctif sous 100 (versionCode)" >&2; exit 2; }
  echo $((major * 10000 + minor * 100 + patch))
}

case "${1:-}" in
  "")
    show
    ;;
  --check)
    want=${2:?version attendue}
    want=${want#v}
    valid "$want"
    code=$(code_of "$want")
    bad=0
    for got in "$(workspace)" "$(desktop)" "$(tauri)" "$(extension)" "$(android)"; do
      [ "$got" = "$want" ] || bad=1
    done
    [ "$(android_code)" = "$code" ] || bad=1
    show
    [ "$bad" = 0 ] || { echo "versions différentes de $want (versionCode attendu : $code)" >&2; exit 1; }
    echo "tout est en $want"
    ;;
  *)
    want=${1#v}
    valid "$want"
    code=$(code_of "$want")
    # Une seule ligne visée dans chaque fichier ; sed -i.bak puis effacement, pour GNU et BSD.
    sed -i.bak "/^\[workspace.package\]/,/^\[/s/^version = \".*\"/version = \"$want\"/" "$root/Cargo.toml"
    sed -i.bak "1,/^version = /s/^version = \".*\"/version = \"$want\"/" "$root/apps/desktop/Cargo.toml"
    sed -i.bak "s/^  \"version\": \".*\",$/  \"version\": \"$want\",/" "$root/apps/desktop/tauri.conf.json"
    sed -i.bak "s/^  \"version\": \".*\",$/  \"version\": \"$want\",/" "$root/extension/manifest.json"
    sed -i.bak -e "s/^\( *versionName = \)\".*\"$/\1\"$want\"/" -e "s/^\( *versionCode = \)[0-9]*$/\1$code/" \
      "$root/apps/android/app/build.gradle.kts"
    rm -f "$root/Cargo.toml.bak" "$root/apps/desktop/Cargo.toml.bak" "$root/apps/desktop/tauri.conf.json.bak" \
      "$root/extension/manifest.json.bak" "$root/apps/android/app/build.gradle.kts.bak"
    show
    echo "Ensuite : cargo update --workspace --offline, et de même dans apps/desktop, puis commit."
    ;;
esac

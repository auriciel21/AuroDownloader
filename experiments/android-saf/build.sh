#!/usr/bin/env bash
# Construit et installe l'essai SAF sans Gradle, avec les outils du SDK Android.
# Usage : experiments/android-saf/build.sh [--install]
# Demande : SDK Android (build-tools 35.0.0, platforms/android-35), JDK 17, adb pour
# --install. Le résultat va dans experiments/android-saf/build/ (ignoré par git).
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
SDK="${ANDROID_HOME:-${LOCALAPPDATA:-$HOME}/Android/Sdk}"
TOOLS="$SDK/build-tools/35.0.0"
JAR="$SDK/platforms/android-35/android.jar"
OUT="$HERE/build"
EXE=""
BAT=""
case "$(uname -s)" in MINGW* | MSYS* | CYGWIN*) EXE=".exe"; BAT=".bat" ;; esac

rm -rf "$OUT" && mkdir -p "$OUT/classes" "$OUT/dex"
javac --release 11 -encoding UTF-8 -classpath "$JAR" -d "$OUT/classes" \
    "$HERE/src/org/aurodownloader/essai/MainActivity.java"
"$TOOLS/d8$BAT" --min-api 26 --lib "$JAR" --output "$OUT/dex" \
    $(find "$OUT/classes" -name '*.class')
"$TOOLS/aapt2$EXE" link -o "$OUT/unsigned.apk" -I "$JAR" \
    --manifest "$HERE/AndroidManifest.xml"
(cd "$OUT/dex" && jar uf "$OUT/unsigned.apk" classes.dex)
"$TOOLS/zipalign$EXE" -f 4 "$OUT/unsigned.apk" "$OUT/aligned.apk"
"$TOOLS/apksigner$BAT" sign --ks "$HOME/.android/debug.keystore" \
    --ks-pass pass:android --key-pass pass:android \
    --out "$OUT/auro-essai-saf.apk" "$OUT/aligned.apk"
echo "APK : $OUT/auro-essai-saf.apk"

if [ "${1:-}" = "--install" ]; then
    adb install -r "$OUT/auro-essai-saf.apk"
    adb shell am start -n org.aurodownloader.essai/.MainActivity
fi

#!/bin/sh
# Tests instrumentés de l'application (accessibilité, étape 7c) sur l'émulateur de
# l'intégration continue (étape 8), sans connectedAndroidTest : les deux APK installés,
# `am instrument` lancé, son résultat lu (il rend 0 même quand un test échoue).
# Les APK viennent de `./gradlew assembleDebug assembleDebugAndroidTest -PauroAbis=x86_64`.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out="$here/app/build/outputs/apk"
adb install -r "$out/debug/app-debug.apk"
adb install -r "$out/androidTest/debug/app-debug-androidTest.apk"
adb shell am instrument -w -r org.aurodownloader.test/androidx.test.runner.AndroidJUnitRunner \
  | tee "$here/app/build/instrument.txt"
grep -q '^OK (' "$here/app/build/instrument.txt" || { echo "tests instrumentés en échec"; exit 1; }

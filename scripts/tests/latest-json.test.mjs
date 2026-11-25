// latest.json des mises à jour (étape 8c) : node --test scripts/tests/
import { test } from "node:test";
import assert from "node:assert/strict";
import { latestJson, PLATFORMS } from "../latest-json.mjs";

const FILES = [
  "AuroDownloader_0.2.0_x64-setup.exe", "AuroDownloader_0.2.0_x64-setup.exe.sig",
  "AuroDownloader_0.2.0_arm64-setup.exe", "AuroDownloader_0.2.0_arm64-setup.exe.sig",
  "AuroDownloader_0.2.0_amd64.AppImage", "AuroDownloader_0.2.0_amd64.AppImage.sig",
  "AuroDownloader_0.2.0_aarch64.AppImage", "AuroDownloader_0.2.0_aarch64.AppImage.sig",
  // Publiés aussi, sans mise à jour d'eux-mêmes.
  "AuroDownloader_0.2.0_amd64.deb", "AuroDownloader_0.2.0_arm64.deb", "AuroDownloader_0.2.0_arm64.apk", "SHA256SUMS",
];
const sig = (name) => `signature de ${name}\n`;
const DATE = new Date("2026-11-20T10:00:00.123Z");

test("une entrée par plateforme, adresse de la publication et signature", () => {
  const json = latestJson("0.2.0", FILES, sig, "Corrections.\n", DATE);
  assert.equal(json.version, "0.2.0");
  assert.equal(json.notes, "Corrections.");
  assert.equal(json.pub_date, "2026-11-20T10:00:00Z");
  assert.deepEqual(Object.keys(json.platforms), Object.keys(PLATFORMS));
  assert.deepEqual(json.platforms["windows-x86_64"], {
    signature: "signature de AuroDownloader_0.2.0_x64-setup.exe.sig",
    url: "https://github.com/auriciel21/AuroDownloader/releases/download/v0.2.0/AuroDownloader_0.2.0_x64-setup.exe",
  });
  assert.match(json.platforms["linux-aarch64"].url, /_aarch64\.AppImage$/);
});

test("un paquet ou une signature manquante arrête la publication", () => {
  assert.throws(() => latestJson("0.2.0", FILES.filter((f) => !f.startsWith("AuroDownloader_0.2.0_arm64-setup.exe")), sig, "", DATE),
    /windows-aarch64 : 0 paquet/);
  assert.throws(() => latestJson("0.2.0", FILES.filter((f) => f !== "AuroDownloader_0.2.0_amd64.AppImage.sig"), sig, "", DATE),
    /linux-x86_64 : signature .* absente/);
  assert.throws(() => latestJson("0.2.0", FILES, () => " \n", "", DATE), /signature vide/);
  assert.throws(() => latestJson("0.2", FILES, sig, "", DATE), /version invalide/);
});

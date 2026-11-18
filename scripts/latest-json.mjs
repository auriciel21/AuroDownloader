// latest.json des mises à jour de l'application de bureau (SPEC 4.6, étape 8c) : lu par le
// greffon de Tauri à releases/latest/download/latest.json. Pour chaque plateforme, l'adresse
// du paquet dans la publication et sa signature (fichier .sig voisin, clé de l'utilisateur).
//
//   node scripts/latest-json.mjs <version> <dossier des paquets> <notes.md> > latest.json
//
// Paquets attendus (noms de Tauri) : *_x64-setup.exe, *_arm64-setup.exe (NSIS),
// *_amd64.AppImage, *_aarch64.AppImage. Un paquet manquant, ou sans signature, est une
// erreur : une publication incomplète laisserait une plateforme sans mise à jour.
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

export const REPO = "https://github.com/aurodownloader/aurodownloader";

export const PLATFORMS = {
  "windows-x86_64": /_x64-setup\.exe$/,
  "windows-aarch64": /_arm64-setup\.exe$/,
  "linux-x86_64": /_amd64\.AppImage$/,
  "linux-aarch64": /_aarch64\.AppImage$/,
};

export function latestJson(version, files, readSig, notes, date) {
  if (!/^\d+\.\d+\.\d+$/.test(version)) throw new Error(`version invalide : ${version}`);
  const platforms = {};
  for (const [platform, pattern] of Object.entries(PLATFORMS)) {
    const found = files.filter((name) => pattern.test(name));
    if (found.length !== 1) throw new Error(`${platform} : ${found.length} paquet(s) au lieu d'un`);
    const [name] = found;
    if (!files.includes(`${name}.sig`)) throw new Error(`${platform} : signature ${name}.sig absente`);
    const signature = readSig(`${name}.sig`).trim();
    if (!signature) throw new Error(`${platform} : signature vide`);
    platforms[platform] = { signature, url: `${REPO}/releases/download/v${version}/${encodeURIComponent(name)}` };
  }
  return { version, notes: notes.trim(), pub_date: date.toISOString().replace(/\.\d{3}Z$/, "Z"), platforms };
}

if (import.meta.url === `file://${process.argv[1]}` || process.argv[1]?.endsWith("latest-json.mjs")) {
  const [version, dir, notesFile] = process.argv.slice(2);
  if (!version || !dir || !notesFile) {
    console.error("usage : node scripts/latest-json.mjs <version> <dossier> <notes.md>");
    process.exit(2);
  }
  const files = readdirSync(dir);
  const json = latestJson(version.replace(/^v/, ""), files, (name) => readFileSync(join(dir, name), "utf8"),
    readFileSync(notesFile, "utf8"), new Date());
  process.stdout.write(`${JSON.stringify(json, null, 2)}\n`);
}

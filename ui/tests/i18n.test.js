// Textes et formats (N-10).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { EN, STATES, PAUSE_REASONS, ERRORS, translator, stateText, bytes, duration } from "../src/i18n.js";

const fr = translator("fr");
const en = translator("en");

test("chaque texte de la page a sa traduction anglaise", () => {
  const source = (name) => readFileSync(new URL(`../src/${name}`, import.meta.url), "utf8");
  const used = new Set();
  // Textes passés à t("…") dans le code, et attributs data-i18n* de la page.
  for (const m of source("main.js").matchAll(/\bt\("((?:[^"\\]|\\.)*)"/g)) used.add(m[1]);
  for (const m of source("main.js").matchAll(/^\s+\w+: "([^"]*\{0\}[^"]*)",$/gm)) used.add(m[1]);
  for (const m of source("index.html").matchAll(/data-i18n(?:-placeholder|-label|-title)?="([^"]*)"/g)) used.add(m[1]);
  // Tables de libellés de main.js (`const …_LABELS = { clé: "texte", … }`).
  for (const block of source("main.js").matchAll(/const \w+_LABELS = \{([^}]*)\}/g)) {
    for (const m of block[1].matchAll(/: "([^"]*)"/g)) used.add(m[1]);
  }
  for (const text of [...Object.values(STATES), ...Object.values(PAUSE_REASONS), ...Object.values(ERRORS)]) used.add(text);
  const missing = [...used].filter((text) => !(text in EN));
  assert.deepEqual(missing, []);
  assert.ok(used.size > 40, `${used.size} textes trouvés`);
});

test("remplacement des paramètres", () => {
  assert.equal(fr("{0} en échec : {1}", "a.bin", "délai"), "a.bin en échec : délai");
  assert.equal(en("{0} en échec : {1}", "a.bin", "timeout"), "a.bin failed: timeout");
  assert.equal(en("texte inconnu {0}", 3), "texte inconnu 3");
});

test("états avec leur cause", () => {
  assert.equal(stateText({ state: "paused", pauseReason: "user" }, fr), "En pause");
  assert.equal(stateText({ state: "paused", pauseReason: "disk_full" }, fr), "En pause (disque plein)");
  assert.equal(stateText({ state: "paused", pauseReason: "disk_full" }, en), "Paused (disk full)");
  assert.equal(stateText({ state: "completed", moving: true }, en), "Moving the file…");
  assert.equal(stateText({ state: "failed", errorCode: "NET_TIMEOUT" }, fr), "Échec : délai dépassé");
  assert.equal(stateText({ state: "failed", errorCode: "NET_TIMEOUT" }, en), "Failed: timed out");
  assert.equal(stateText({ state: "failed", errorCode: "NOUVEAU_CODE" }, fr), "Échec : NOUVEAU_CODE");
  assert.equal(stateText({ state: "inconnu" }, fr), "inconnu");
  // F-11 : part du fichier lue par la vérification de l'empreinte.
  assert.equal(stateText({ state: "verifying" }, fr), "Vérification de l'empreinte");
  assert.equal(stateText({ state: "verifying", verifiedBytes: 50, totalBytes: 200 }, fr), "Vérification de l'empreinte (25 %)");
  assert.equal(stateText({ state: "verifying", verifiedBytes: 50, totalBytes: 200 }, en), "Verifying the checksum (25%)");
  assert.equal(stateText({ state: "failed", errorCode: "CHECKSUM_MISMATCH" }, fr), "Échec : empreinte différente de celle attendue");
});

test("tailles et durées", () => {
  assert.equal(bytes(512, "en", en), "512 B");
  assert.equal(bytes(1536, "en", en), "1.5 KiB");
  assert.equal(bytes(5 * 1024 ** 3, "en", en), "5.0 GiB");
  assert.equal(bytes(1536, "fr", fr), "1,5 Kio");
  assert.equal(bytes(null, "fr", fr), "?");
  assert.equal(bytes(-1, "fr", fr), "?");
  assert.equal(duration(42, fr), "42 s");
  assert.equal(duration(125, fr), "2 min 5 s");
  assert.equal(duration(7300, fr), "2 h 1 min");
  assert.equal(duration(90000, en), "1 d 1 h");
  assert.equal(duration(null, fr), "");
});

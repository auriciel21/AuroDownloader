// Décision de capture (extension/decide.js), sans navigateur : node --test extension/tests/
const test = require("node:test");
const assert = require("node:assert/strict");
require("../decide.js");

const { decide, settings, requestId, ALT_WINDOW_MS } = globalThis.AuroDecide;
const MiB = 1024 * 1024;
const item = (extra = {}) => ({
  id: 7,
  url: "https://exemple.org/fichier.iso",
  totalBytes: 50 * MiB,
  state: "in_progress",
  incognito: false,
  startTime: "2026-11-07T10:00:00.000Z",
  ...extra,
});

test("par défaut, tout ce qui dépasse 1 Mio est capturé", () => {
  assert.equal(decide(item(), {}).capture, true);
  assert.equal(decide(item({ totalBytes: 512 * 1024 }), {}).reason, "trop petit");
  // Taille inconnue : capturé, le moteur la trouvera.
  assert.equal(decide(item({ totalBytes: -1, fileSize: 0 }), {}).capture, true);
  assert.equal(decide(item({ totalBytes: 512 * 1024 }), { minSizeMiB: 0 }).capture, true);
});

test("ce qui reste au navigateur", () => {
  const now = 1_000_000;
  const cases = [
    [item(), { capture: false }, {}, "capture désactivée"],
    [item({ url: "blob:https://exemple.org/1" }), {}, {}, "adresse non HTTP"],
    [item({ url: "data:text/plain,x" }), {}, {}, "adresse non HTTP"],
    [item({ incognito: true }), {}, {}, "navigation privée"],
    [item(), {}, { now, altClickAt: now - 500 }, "clic avec Alt"],
    [item({ byExtensionId: "moi" }), {}, { selfId: "moi" }, "rendu au navigateur"],
    [item({ state: "complete" }), {}, {}, "déjà fini"],
    [item({ url: "https://cdn.exemple.org/a.iso" }), { excludedSites: ["exemple.org"] }, {}, "site exclu"],
    [item(), { excludedSites: ["*.exemple.org"] }, {}, "site exclu"],
    [item(), { excludedSites: ["https://exemple.org/page"] }, {}, "site exclu"],
  ];
  for (const [it, stored, opts, reason] of cases) {
    const verdict = decide(it, stored, opts);
    assert.equal(verdict.capture, false, reason);
    assert.equal(verdict.reason, reason);
  }
  // Le clic avec Alt ne vaut qu'un moment ; un site voisin n'est pas exclu.
  assert.equal(decide(item(), {}, { now, altClickAt: now - ALT_WINDOW_MS }).capture, true);
  assert.equal(decide(item(), { excludedSites: ["autre-exemple.org"] }).capture, true);
  assert.equal(decide(item({ url: "https://pasexemple.org/a" }), { excludedSites: ["exemple.org"] }).capture, true);
  assert.equal(decide(item({ incognito: true }), { captureIncognito: true }).capture, true);
  // L'adresse finale (après redirection) compte.
  assert.equal(decide(item({ finalUrl: "https://dl.exemple.org/x" }), { excludedSites: ["dl.exemple.org"] }).capture, false);
});

test("réglages abîmés : valeurs par défaut", () => {
  const s = settings({ minSizeMiB: "beaucoup", excludedSites: "exemple.org", capture: "oui" });
  assert.equal(s.minSizeMiB, 1);
  assert.deepEqual(s.excludedSites, []);
  assert.equal(s.capture, true);
});

test("une demande garde la même clé pour le même téléchargement", () => {
  assert.equal(requestId(item()), requestId(item()));
  assert.notEqual(requestId(item()), requestId(item({ id: 8 })));
});

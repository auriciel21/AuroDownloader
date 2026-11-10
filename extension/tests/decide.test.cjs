// Décision de capture (extension/decide.js), sans navigateur : node --test extension/tests/
const test = require("node:test");
const assert = require("node:assert/strict");
require("../decide.js");

const { decide, settings, requestId, ALT_WINDOW_MS, refererFor, cookieHeader, pageHeaders, cookieStore } =
  globalThis.AuroDecide;
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

test("Referer : ce que le navigateur enverrait lui-même", () => {
  const page = "https://exemple.org/page?q=1#ancre";
  assert.equal(refererFor(page, "https://exemple.org/f.iso"), "https://exemple.org/page?q=1");
  // Autre origine : l'origine seule.
  assert.equal(refererFor(page, "https://cdn.exemple.org/f.iso"), "https://exemple.org/");
  // De HTTPS vers HTTP : rien.
  assert.equal(refererFor(page, "http://exemple.org/f.iso"), null);
  assert.equal(refererFor("http://exemple.org/p", "https://exemple.org/f"), "http://exemple.org/");
  // Identifiants retirés ; pages non HTTP et adresses illisibles ignorées.
  assert.equal(refererFor("https://moi:secret@exemple.org/p", "https://exemple.org/f"), "https://exemple.org/p");
  assert.equal(refererFor("about:blank", "https://exemple.org/f"), null);
  assert.equal(refererFor("pas une adresse", "https://exemple.org/f"), null);
});

test("en-tête Cookie : chemins les plus longs d'abord", () => {
  const cookies = [
    { name: "a", value: "1", path: "/" },
    { name: "b", value: "2", path: "/telechargements" },
    { name: "", value: "nu", path: "/t" },
  ];
  assert.equal(cookieHeader(cookies), "b=2; nu; a=1");
  assert.equal(cookieHeader([]), "");
  assert.equal(cookieHeader(undefined), "");
});

test("en-têtes transmis, sauf si l'option est coupée", () => {
  const page = {
    url: "https://cdn.exemple.org/f.iso",
    page: "https://exemple.org/p",
    userAgent: "Navigateur/1",
    cookies: [{ name: "s", value: "1", path: "/" }],
  };
  assert.deepEqual(pageHeaders({}, page), {
    Referer: "https://exemple.org/",
    "User-Agent": "Navigateur/1",
    Cookie: "s=1",
  });
  assert.deepEqual(pageHeaders({}, { url: page.url, cookies: [] }), {});
  assert.deepEqual(pageHeaders({ sendCookies: false }, page), {});
  assert.equal(settings({}).sendCookies, true);
});

test("magasin de cookies d'un téléchargement", () => {
  assert.equal(cookieStore(item({ cookieStoreId: "firefox-container-2" })), "firefox-container-2");
  assert.equal(cookieStore(item({ incognito: true })), "1");
  assert.equal(cookieStore(item()), undefined);
});

// Étape 5d : fichiers .torrent et liens magnet.
test("un fichier .torrent est capturé comme torrent, quelle que soit sa taille", () => {
  const { isTorrentDownload } = globalThis.AuroDecide;
  const small = { totalBytes: 20 * 1024 };
  for (const it of [
    item({ ...small, mime: "application/x-bittorrent" }),
    item({ ...small, mime: "application/x-bittorrent; charset=binary" }),
    item({ ...small, filename: "C:\Users\moi\Downloads\debian.iso.torrent" }),
    item({ ...small, url: "https://exemple.org/debian-13.iso.torrent?x=1" }),
    item({ ...small, url: "https://exemple.org/dl?id=4", finalUrl: "https://cdn.exemple.org/a.TORRENT" }),
  ]) {
    assert.ok(isTorrentDownload(it), JSON.stringify(it));
    assert.deepEqual(decide(it, {}), { capture: true, reason: "", torrent: true });
  }
  assert.equal(isTorrentDownload(item()), false);
  assert.equal(isTorrentDownload(item({ url: "https://exemple.org/torrent/page.html" })), false);
  // Les autres règles passent avant : site exclu, Alt, navigation privée, capture coupée.
  const torrent = item({ ...small, mime: "application/x-bittorrent" });
  assert.equal(decide(torrent, { excludedSites: ["exemple.org"] }).reason, "site exclu");
  assert.equal(decide(torrent, { capture: false }).capture, false);
  assert.equal(decide({ ...torrent, incognito: true }, {}).reason, "navigation privée");
  const now = 5_000_000;
  assert.equal(decide(torrent, {}, { now, altClickAt: now - 10 }).reason, "clic avec Alt");
});

test("contenu d'un .torrent vérifié avant l'envoi, puis en base64", () => {
  const { torrentProblem, toBase64, MAX_TORRENT_BYTES } = globalThis.AuroDecide;
  const bytes = new TextEncoder().encode("d4:infod4:name1:xee");
  assert.equal(torrentProblem(bytes), null);
  assert.equal(toBase64(bytes), Buffer.from(bytes).toString("base64"));
  assert.ok(torrentProblem(new Uint8Array(0)));
  assert.ok(torrentProblem(new TextEncoder().encode("<!doctype html>")));
  assert.ok(torrentProblem(new Uint8Array(MAX_TORRENT_BYTES + 1).fill(0x64)));
  // Gros contenu : par morceaux, identique au base64 de Node.
  const big = new Uint8Array(3 * 1024 * 1024 + 7).map((_, i) => (i * 31) % 256);
  assert.equal(toBase64(big), Buffer.from(big).toString("base64"));
});

test("liens magnet : reconnus, option active par défaut", () => {
  const { isMagnet } = globalThis.AuroDecide;
  assert.ok(isMagnet("magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567"));
  assert.ok(isMagnet("MAGNET:?dn=x"));
  assert.equal(isMagnet("https://exemple.org/magnet:?x"), false);
  assert.equal(isMagnet(undefined), false);
  assert.equal(settings({}).captureMagnet, true);
  assert.equal(settings({ captureMagnet: false }).captureMagnet, false);
});

test("lien magnet refusé : rendu au navigateur seulement si AuroDownloader est injoignable", () => {
  const { givesBackMagnet } = globalThis.AuroDecide;
  assert.equal(givesBackMagnet({ ok: true, taskId: "t" }), false);
  assert.equal(givesBackMagnet({ ok: false, code: "RELAY_MISSING" }), true);
  assert.equal(givesBackMagnet({ ok: false, code: "ENGINE_UNAVAILABLE" }), true);
  // Refus du moteur : torrent déjà dans la liste, lien invalide.
  assert.equal(givesBackMagnet({ ok: false, code: "INVALID_PARAMS" }), false);
  assert.equal(givesBackMagnet({ ok: false, code: "BAD_MESSAGE" }), false);
});

// Accessibilité de l'extension (WCAG 2.2 AA, N-09) : fenêtre de l'icône et options, avec
// leurs scripts et un faux `chrome.*` (messages réels de _locales), contrôlées par axe-core
// dans jsdom ; contrastes des couleurs d'options.css, thèmes clair et sombre.
// axe-core et jsdom sont les dépendances de développement de la page (cd ui && npm ci).
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { createRequire } = require("node:module");

const fromUi = createRequire(path.join(__dirname, "..", "..", "ui", "package.json"));
const axe = fromUi("axe-core");
const { JSDOM } = fromUi("jsdom");

const dir = path.join(__dirname, "..");
const read = (name) => fs.readFileSync(path.join(dir, name), "utf8");

const OPTIONS = {
  runOnly: { type: "tag", values: ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa", "best-practice"] },
  rules: { "color-contrast": { enabled: false } },
  resultTypes: ["violations", "incomplete"],
};

function messages(lang) {
  return JSON.parse(read(path.join("_locales", lang, "messages.json")));
}

/** Un `chrome.*` d'essai : messages de la langue, réglages vides, relais qui répond `reply`. */
function fakeChrome(lang, nativeReply) {
  const catalog = messages(lang);
  return {
    i18n: {
      getMessage: (key, subs = []) => (catalog[key]?.message ?? "").replace(/\$(\d)/g, (_, n) => subs[n - 1] ?? ""),
      getUILanguage: () => lang,
    },
    storage: {
      sync: { get: async () => ({}), set: async () => {} },
      local: { get: async () => ({ last: { at: 0, ok: true, url: "https://exemple.org/f.iso" } }) },
    },
    runtime: {
      sendNativeMessage: async () => nativeReply,
      sendMessage: async () => ({ ok: true }),
      openOptionsPage: () => {},
    },
    tabs: { query: async () => [{ url: "https://video.exemple.org/v/1" }] },
    permissions: { contains: async () => false, request: async () => false },
  };
}

async function open(page, scripts, lang, nativeReply) {
  const dom = new JSDOM(read(page), { runScripts: "outside-only", pretendToBeVisual: true, url: "https://extension.invalid/" });
  const { window } = dom;
  window.document.elementFromPoint = () => null;
  window.document.elementsFromPoint = () => [];
  window.HTMLElement.prototype.scrollIntoView ??= () => {};
  window.chrome = fakeChrome(lang, nativeReply);
  for (const script of scripts) window.eval(read(script));
  for (let i = 0; i < 5; i++) await new Promise((resolve) => setTimeout(resolve, 10));
  window.eval(axe.source);
  return window;
}

async function audit(window) {
  const result = await window.axe.run(window.document, OPTIONS);
  const found = Array.from(result.violations).flatMap((v) =>
    Array.from(v.nodes).map((n) => `${v.id} (${v.impact}) : ${Array.from(n.target).join(" ")} — ${n.failureSummary.split("\n").slice(1).join(" ").trim()}`));
  const skipped = Array.from(result.incomplete).filter((r) => r.error).map((r) => `${r.id} sautée : ${r.error.message}`);
  return [...found, ...skipped];
}

const VIDEO = {
  ok: true,
  title: "Une vidéo libre",
  durationS: 125,
  formats: [
    { formatId: "v720", vcodec: "avc1", acodec: "mp4a", height: 720, filesize: 9_000_000 },
    { formatId: "a1", vcodec: null, acodec: "mp4a", filesize: 1_000_000 },
  ],
  subtitles: ["fr", "en"],
};

for (const lang of ["fr", "en"]) {
  test(`fenêtre de l'icône, vidéo décrite (${lang})`, async () => {
    const window = await open("popup.html", ["video.js", "popup.js"], lang, VIDEO);
    assert.equal(window.document.getElementById("choice").hidden, false, "choix montré");
    assert.equal(window.document.documentElement.lang, lang);
    assert.deepEqual(await audit(window), []);
  });

  test(`fenêtre de l'icône, relais absent (${lang})`, async () => {
    const window = await open("popup.html", ["video.js", "popup.js"], lang, { ok: false, code: "RELAY_MISSING" });
    assert.equal(window.document.getElementById("retry").hidden, false, "« Réessayer » montré");
    assert.deepEqual(await audit(window), []);
  });

  test(`options (${lang})`, async () => {
    const window = await open("options.html", ["decide.js", "options.js"], lang, { ok: true, version: "0.1.0" });
    assert.equal(window.document.getElementById("access").hidden, false, "accès aux sites à accorder");
    assert.deepEqual(await audit(window), []);
  });
}

test("l'audit voit un défaut glissé", async () => {
  const window = await open("options.html", ["decide.js", "options.js"], "fr", { ok: true });
  window.document.querySelector("main").insertAdjacentHTML("beforeend", "<button></button>");
  assert.ok((await audit(window)).some((v) => v.startsWith("button-name")));
});

// Contrastes d'options.css (aussi les couleurs de la fenêtre de l'icône).
const css = read("options.css");
function tokens(start) {
  const open = css.indexOf("{", start);
  let depth = 0;
  let end = open;
  for (; end < css.length; end++) {
    if (css[end] === "{") depth++;
    if (css[end] === "}" && --depth === 0) break;
  }
  return Object.fromEntries([...css.slice(open, end).matchAll(/--([a-z-]+):\s*(#[0-9a-f]{6})\s*;/gi)].map((m) => [m[1], m[2]]));
}
function luminance(hex) {
  const [r, g, b] = [1, 3, 5].map((i) => {
    const c = parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}
function ratio(a, b) {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}
const light = tokens(css.indexOf(":root"));
const dark = { ...light, ...tokens(css.indexOf(":root", css.indexOf("@media (prefers-color-scheme: dark)"))) };
const PAIRS = [
  ["text", "bg", 4.5, "texte"],
  ["muted", "bg", 4.5, "aides et états"],
  ["accent", "bg", 4.5, "lien « Options »"],
  ["bg", "accent", 4.5, "bouton « Télécharger »"],
  ["accent", "bg", 3, "contour du focus"],
  ["control", "bg", 3, "bords des champs et boutons"],
];
for (const [theme, colors] of [["clair", light], ["sombre", dark]]) {
  test(`contrastes d'options.css, thème ${theme}`, () => {
    const bad = PAIRS.filter(([fg, bg, min]) => !(colors[fg] && colors[bg]) || ratio(colors[fg], colors[bg]) < min)
      .map(([fg, bg, min, where]) => `${fg} sur ${bg} (${where}) : ${colors[fg] && colors[bg] ? ratio(colors[fg], colors[bg]).toFixed(2) : "absente"} < ${min}`);
    assert.deepEqual(bad, []);
  });
}

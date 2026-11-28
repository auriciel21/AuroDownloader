// Accessibilité de la page vivante (WCAG 2.2 AA, N-09) : la vraie page (main.js) dans jsdom,
// branchée à un faux moteur qui tient des tâches dans tous les états ; axe-core contrôle
// ce que le script dessine (lignes du tableau, arborescence, menu, Propriétés, Options),
// que l'audit du balisage seul (axe.test.js) ne voit pas.
import { test, before } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import axe from "axe-core";
import { JSDOM } from "jsdom";

const html = readFileSync(new URL("../src/index.html", import.meta.url), "utf8");

const OPTIONS = {
  runOnly: { type: "tag", values: ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa", "best-practice"] },
  rules: { "color-contrast": { enabled: false } },
  resultTypes: ["violations", "incomplete"],
};

const NOW = Date.parse("2026-11-18T10:00:00Z");
const MIB = 1024 * 1024;

function task(taskId, state, extra = {}) {
  return {
    taskId, kind: "http", queueId: "default", state, priority: 0,
    displayName: `${taskId}.iso`, destDir: "/home/essai/Téléchargements", fileName: `${taskId}.iso`,
    totalBytes: 100 * MIB, doneBytes: 40 * MIB, maxConnections: 0, speedLimitBps: null,
    pauseReason: null, errorCode: null, errorDetail: null, retryCount: 0,
    createdAt: NOW - 3600_000, finishedAt: null, moving: false, publicOnly: false, checksum: null,
    ...extra,
  };
}

const TASKS = [
  task("actif", "active"),
  task("attente", "queued", { doneBytes: 0 }),
  task("pause", "paused", { pauseReason: "user" }),
  task("echec", "failed", { errorCode: "HOST_KEY_UNKNOWN", errorDetail: "ssh-ed25519 SHA256:abc" }),
  task("fini", "completed", { doneBytes: 100 * MIB, finishedAt: NOW - 60_000 }),
  task("partage", "seeding", { kind: "torrent", doneBytes: 100 * MIB }),
  task("fusion", "post_processing", { kind: "media", doneBytes: 100 * MIB }),
];

const DETAILS = {
  segments: [
    { idx: 0, start: 0, end: 50 * MIB - 1, doneBytes: 30 * MIB, receiving: true, sourceId: "s1" },
    { idx: 1, start: 50 * MIB, end: 100 * MIB - 1, doneBytes: 10 * MIB, receiving: false, sourceId: "s1" },
  ],
  sources: [{ sourceId: "s1", url: "https://exemple.org/actif.iso", acceptsRanges: true, rank: 0, state: "ok", avgBps: 1_000_000, failCount: 0, lastError: null }],
  files: [],
};

/** Réponses du faux moteur ; une méthode absente rend un objet vide. */
const ENGINE = {
  "sync.snapshot": () => ({ rev: 1, tasks: TASKS, queues: [{ queueId: "default", name: "Principale", position: 0, maxActive: 3, speedLimitBps: null, scheduleId: null }] }),
  "task.get": ({ taskId }) => ({ ...TASKS.find((t) => t.taskId === taskId), ...DETAILS }),
  "login.list": () => ({ items: [{ site: "sftp://exemple.org", username: "moi", allowPlain: false, keyPath: "/home/essai/.ssh/id_ed25519" }] }),
  "hostkey.list": () => ({ items: [{ host: "exemple.org", port: 22, algorithm: "ssh-ed25519", fingerprint: "SHA256:abc", acceptedAt: NOW }], pending: [{ host: "exemple.org", port: 22, algorithm: "ssh-ed25519", fingerprint: "SHA256:abc" }] }),
  "rules.list": () => ({ rules: [] }),
  "plugin.list": () => ({ items: [] }),
  "token.list": () => ({ items: [] }),
  "history.list": () => ({ items: [], next: null }),
  "task.add": ({ sources }) => {
    if (!/^https?:\/\//.test(sources?.[0]?.url ?? "")) throw new Error("adresse invalide");
    return { taskId: "nouvelle" };
  },
};

const APP_INFO = { lang: "fr", prefs: { lang: "fr", closeToTray: true, showOnAdd: true }, tray: true, executableExtensions: ["exe"], platform: "linux", autostart: false, version: "0.1.0" };

/** Faux hôte des mises à jour (étape 8b) : installation qui se met à jour, clé présente. */
const FOUND = {
  version: "0.2.0", current: "0.1.0", date: null,
  page: "https://github.com/auriciel21/AuroDownloader/releases/tag/v0.2.0",
  // Notes venues de la publication : du texte, jamais interprété comme du HTML.
  notes: "Corrections.\n<img src=x onerror=\"globalThis.piege = 1\">",
};
const UPDATE = {
  status: { kind: "install", configured: true, enabled: true, lastCheck: null, found: null },
  next: FOUND,
  installs: 0,
  pages: [],
};

let window;
const calls = [];
/** Écouteurs des événements de l'hôte (« engine » : notifications du moteur). */
const listeners = {};
let rev = 1;

function notify(method, params) {
  rev += 1;
  listeners.engine({ payload: { kind: "message", message: { method, params: { rev, ...params } } } });
}

async function settle() {
  for (let i = 0; i < 5; i++) await new Promise((resolve) => setTimeout(resolve, 20));
}

async function audit(context) {
  const result = await window.axe.run(context, OPTIONS);
  const found = Array.from(result.violations).flatMap((v) =>
    Array.from(v.nodes).map((n) => `${v.id} (${v.impact}) : ${Array.from(n.target).join(" ")} — ${n.failureSummary.split("\n").slice(1).join(" ").trim()}`));
  const skipped = Array.from(result.incomplete).filter((r) => r.error).map((r) => `${r.id} sautée : ${r.error.message}`);
  return [...found, ...skipped];
}

before(async () => {
  const dom = new JSDOM(html, { runScripts: "outside-only", pretendToBeVisual: true, url: "http://localhost/" });
  window = dom.window;
  const { document } = window;
  document.elementFromPoint = () => null;
  document.elementsFromPoint = () => [];
  // jsdom n'ouvre pas les boîtes. Comme un navigateur (HTML, « dialog focusing steps ») :
  // le focus va à l'élément `autofocus`, sinon au premier qui le prend.
  const proto = window.HTMLDialogElement.prototype;
  // À la fermeture, le focus revient à l'élément qui l'avait avant l'ouverture.
  const previous = new WeakMap();
  proto.showModal = function () {
    previous.set(this, document.activeElement);
    this.setAttribute("open", "");
    const candidates = [...this.querySelectorAll("button, input, select, textarea, [tabindex]")]
      .filter((el) => el.tabIndex >= 0 && !el.disabled && !el.closest("[hidden], .lacks"));
    (candidates.find((el) => el.hasAttribute("autofocus")) ?? candidates[0])?.focus();
  };
  proto.show = proto.showModal;
  proto.close = function (value) {
    if (value !== undefined) this.returnValue = value;
    this.removeAttribute("open");
    if (this.contains(document.activeElement)) previous.get(this)?.focus();
    this.dispatchEvent(new window.Event("close"));
  };
  window.matchMedia ??= () => ({ matches: false, addEventListener() {}, removeEventListener() {} });
  window.HTMLElement.prototype.scrollIntoView ??= () => {};
  window.__TAURI__ = {
    core: {
      invoke: async (command, args = {}) => {
        calls.push(command === "rpc" ? args.method : command);
        switch (command) {
          case "rpc": return (ENGINE[args.method] ?? (() => ({})))(args.params ?? {});
          case "app_info": return APP_INFO;
          case "save_prefs":
            APP_INFO.prefs = args.prefs;
            return APP_INFO;
          // Mises à jour (étape 8b).
          case "update_status": return { ...UPDATE.status };
          case "update_check": return { checkedAt: 1_790_000_000, found: UPDATE.next };
          case "update_install": UPDATE.installs += 1; return null;
          case "update_open_page": UPDATE.pages.push(args.version); return null;
          case "engine_status": return { state: "up", engine: { engine: { version: "0.1.0" }, capabilities: ["media", "linkscan", "crawl"], scope: "admin" } };
          case "links_status": return { available: false, enabled: false };
          case "take_opened": return [];
          default: return null;
        }
      },
    },
    event: { listen: async (name, fn) => { listeners[name] = fn; return () => {}; } },
  };
  // Fichiers de la page (guide de l'utilisateur, étape 9d), comme les servirait l'hôte.
  globalThis.fetch = async (url) => {
    const path = new URL(url, "http://localhost/").pathname;
    try {
      const text = readFileSync(new URL(`../src${path}`, import.meta.url), "utf8");
      return { ok: true, status: 200, text: async () => text };
    } catch {
      return { ok: false, status: 404, text: async () => "" };
    }
  };
  window.eval(axe.source);
  // La page est un module : chargée par Node, avec la fenêtre de jsdom comme globale.
  for (const key of Object.getOwnPropertyNames(window)) {
    if (!(key in globalThis)) {
      try { globalThis[key] = window[key]; } catch { /* propriété en lecture seule */ }
    }
  }
  // Fonctions héritées de la fenêtre (addEventListener vient d'EventTarget) : depuis
  // Node 21, la liste des propriétés propres de la fenêtre de jsdom ne les donne plus.
  for (let proto = Object.getPrototypeOf(window); proto && proto !== Object.prototype && proto !== window.Object.prototype; proto = Object.getPrototypeOf(proto)) {
    for (const key of Object.getOwnPropertyNames(proto)) {
      if (key === "constructor" || key in globalThis) continue;
      const value = Object.getOwnPropertyDescriptor(proto, key).value;
      if (typeof value === "function") globalThis[key] = value;
    }
  }
  for (const key of ["Option", "Image", "Audio"]) globalThis[key] ??= window[key];
  // Absent de jsdom ; les identifiants de la page n'ont que des lettres, chiffres, - et _.
  globalThis.CSS ??= { escape: (text) => String(text).replace(/[^\w-]/g, (c) => `\\${c}`) };
  globalThis.window = window;
  globalThis.document = document;
  globalThis.navigator ??= window.navigator;
  await import("../src/main.js");
  await settle();
});

test("mises à jour : l'avis du premier lancement, une fois, puis la préférence enregistrée", async () => {
  const document = window.document;
  const notice = document.getElementById("update-notice");
  assert.ok(notice.open, "avis montré au premier lancement");
  assert.ok(notice.contains(document.activeElement), "focus dans l'avis");
  assert.deepEqual(await audit(notice), []);
  notice.close("off");
  await settle();
  assert.equal(APP_INFO.prefs.updateNoticeShown, true);
  assert.equal(APP_INFO.prefs.checkUpdates, false, "« Ne pas rechercher » coupe la recherche");
  assert.equal(document.getElementById("pref-updates").checked, false);
  // Recochée pour la suite.
  const box = document.getElementById("pref-updates");
  box.checked = true;
  box.dispatchEvent(new window.Event("change", { bubbles: true }));
  await settle();
  assert.equal(APP_INFO.prefs.checkUpdates, true);
});

test("la page s'est remplie depuis le faux moteur", () => {
  assert.equal(window.document.getElementById("error").hidden, true, window.document.getElementById("error").textContent);
  assert.equal(window.document.querySelectorAll("#grid tbody tr").length, TASKS.length, calls.join(", "));
});

test("chaque boîte a ses zones de messages dès le chargement, avant tout message", () => {
  // Une zone `aria-live` créée au moment du message n'est pas lue (essai NVDA).
  const missing = [...window.document.querySelectorAll("dialog")]
    .filter((dialog) => !dialog.querySelector('[data-live="status"][aria-live="polite"]') || !dialog.querySelector('[data-live="alert"][role="alert"]'))
    .map((dialog) => dialog.id);
  assert.deepEqual(missing, []);
});

test("fenêtre principale, avec ses lignes et son arborescence", async () => {
  assert.deepEqual(await audit(window.document), []);
});

function row(taskId) {
  return window.document.querySelector(`#grid tbody tr[data-task-id="${taskId}"]`);
}

test("chaque ligne du tableau porte un nom tiré de ses cellules (WebKitGTK ne le calcule pas)", () => {
  const document = window.document;
  for (const { taskId } of TASKS) {
    const r = row(taskId);
    const ids = (r.getAttribute("aria-labelledby") ?? "").split(/\s+/).filter(Boolean);
    const parts = ids.map((id) => document.getElementById(id));
    assert.ok(parts.length >= 4 && parts.every((el) => el && r.contains(el)), `${taskId} : ${ids.join(" ")}`);
    const name = parts.map((el) => el.textContent.trim()).filter(Boolean).join(" ");
    assert.ok(name.startsWith(`${taskId}.iso`), name);
    assert.ok(name.includes(r.querySelector(".state").textContent.trim()), name);
  }
});

function key(target, key, extra = {}) {
  target.dispatchEvent(new window.KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...extra }));
}

async function closeAll() {
  for (const dialog of window.document.querySelectorAll("dialog[open]")) dialog.close();
  const menu = window.document.getElementById("menu");
  if (menu && !menu.hidden) key(menu, "Escape");
  await settle();
}

for (const { taskId, state } of TASKS) {
  test(`Propriétés d'une tâche ${state}`, async () => {
    await closeAll();
    row(taskId).firstElementChild.dispatchEvent(new window.MouseEvent("dblclick", { bubbles: true }));
    await settle();
    const details = window.document.getElementById("details");
    assert.ok(details.open, "Propriétés ouvertes");
    assert.deepEqual(await audit(details), []);
  });
}

test("Propriétés : la clé du serveur SSH d'une tâche arrêtée par elle", async () => {
  await closeAll();
  row("echec").firstElementChild.dispatchEvent(new window.MouseEvent("dblclick", { bubbles: true }));
  await settle();
  assert.equal(window.document.getElementById("d-hostkey").hidden, false);
});

// Audit W7 : « Supprimer » une tâche qui finit sa fusion ou sa commande de fin le dit, au
// lieu de ne rien faire ; rien n'est demandé au moteur pour elle.
test("supprimer une tâche en post_processing : annoncé, rien d'envoyé", async () => {
  await closeAll();
  const { document } = window;
  row("fusion").firstElementChild.dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
  await settle();
  const before = calls.length;
  document.getElementById("tb-remove").click();
  await settle();
  const dialog = document.getElementById("remove-dialog");
  assert.ok(dialog.open, "boîte de confirmation ouverte");
  dialog.close("confirm");
  await settle();
  const sent = calls.slice(before).filter((c) => c === "task.remove" || c === "task.cancel");
  assert.deepEqual(sent, []);
  assert.equal(document.getElementById("error").hidden, false);
  assert.match(document.getElementById("error-text").textContent, /fusion\.iso.*fusion ou sa commande de fin/);
  document.getElementById("error").hidden = true;
});

// Audit W14 (leçon A15) : l'erreur d'un champ lui est liée avant qu'il prenne le focus.
test("Options : une adresse de proxy refusée est liée à son champ, qui prend le focus", async () => {
  await closeAll();
  const { document } = window;
  ENGINE["proxy.set"] = () => { throw new Error("url : adresse de proxy illisible"); };
  try {
    document.getElementById("tb-settings").click();
    await settle();
    document.getElementById("proxy-manual").click();
    await settle();
    const url = document.getElementById("proxy-url");
    url.value = "pas un proxy";
    document.getElementById("proxy-apply").click();
    await settle();
    assert.equal(url.getAttribute("aria-invalid"), "true");
    assert.ok((url.getAttribute("aria-describedby") ?? "").split(/\s+/).includes("proxy-error"));
    assert.match(document.getElementById("proxy-error").textContent, /illisible/);
    assert.equal(document.activeElement?.id, "proxy-url");
  } finally {
    delete ENGINE["proxy.set"];
    await closeAll();
  }
});

// Audit W15 : sans la portée `admin`, les Options ne demandent ni les identifiants ni les
// interfaces réseau (le moteur les refuserait).
test("Options sans droit d'administration : rien de réservé n'est demandé", async () => {
  await closeAll();
  const { document } = window;
  const up = (scope) => listeners.engine({ payload: { kind: "status", state: "up", engine: { engine: { version: "0.1.0" }, capabilities: ["media", "linkscan", "crawl"], scope } } });
  up("control");
  await settle();
  try {
    const before = calls.length;
    document.getElementById("tb-settings").click();
    await settle();
    const asked = calls.slice(before);
    for (const method of ["login.list", "torrent.interfaces", "hostkey.list", "proxy.get"]) {
      assert.ok(!asked.includes(method), `${method} : ${asked.join(" ")}`);
    }
  } finally {
    await closeAll();
    up("admin");
    await settle();
  }
});

test("menu contextuel ouvert au clavier", async () => {
  await closeAll();
  const body = window.document.querySelector("#grid tbody");
  row("actif").focus();
  key(body, "ContextMenu");
  await settle();
  const menu = window.document.getElementById("menu");
  assert.equal(menu.hidden, false, "menu ouvert");
  assert.deepEqual(await audit(menu), []);
  assert.equal(window.document.activeElement?.getAttribute("role"), "menuitem", "le focus entre dans le menu");
});

test("menu « Aide » : guide, raccourcis clavier et À propos, au clavier", async () => {
  await closeAll();
  const document = window.document;
  const button = document.getElementById("tb-help");
  const menu = document.getElementById("help-menu");
  assert.equal(button.getAttribute("aria-haspopup"), "menu");
  // Flèche vers le bas sur le bouton : menu ouvert, focus sur la première entrée.
  button.focus();
  key(button, "ArrowDown");
  await settle();
  assert.equal(menu.hidden, false, "menu ouvert");
  assert.equal(button.getAttribute("aria-expanded"), "true");
  assert.equal(document.activeElement?.dataset.help, "guide");
  assert.deepEqual(await audit(menu), []);
  key(document.activeElement, "ArrowDown");
  assert.equal(document.activeElement?.dataset.help, "keys");
  key(document.activeElement, "End");
  assert.equal(document.activeElement?.dataset.help, "about");
  // Échap : menu fermé, focus rendu au bouton.
  key(document.activeElement, "Escape");
  assert.equal(menu.hidden, true);
  assert.equal(button.getAttribute("aria-expanded"), "false");
  assert.equal(document.activeElement, button);

  // Raccourcis clavier : le guide s'ouvre sur leur section, qui prend le focus.
  button.click();
  await settle();
  menu.querySelector('[data-help="keys"]').click();
  await settle();
  const guide = document.getElementById("guide-dialog");
  assert.ok(guide.open, "guide ouvert");
  assert.equal(document.activeElement?.id, "guide-13-accessibilite-clavier-dans-la-fenêtre");
  assert.equal(menu.hidden, true);
  guide.close();
  await settle();

  // Guide : au début.
  button.click();
  await settle();
  menu.querySelector('[data-help="guide"]').click();
  await settle();
  assert.ok(guide.open);
  assert.equal(document.activeElement, document.getElementById("guide-content"));
  guide.close();
  await settle();

  // À propos : les Options, sur leur rubrique.
  button.click();
  await settle();
  menu.querySelector('[data-help="about"]').click();
  await settle();
  const settings = document.getElementById("settings");
  assert.ok(settings.open, "Options ouvertes");
  assert.equal(document.getElementById("tab-about").getAttribute("aria-selected"), "true");
  assert.equal(document.getElementById("panel-about").hidden, false);
  settings.close();
  await settle();
});

test("guide : recherche sans casse ni accents, résultats suivants et précédents, impression", async () => {
  await closeAll();
  const document = window.document;
  key(document.body, "F1");
  await settle();
  const dialog = document.getElementById("guide-dialog");
  assert.ok(dialog.open);
  const field = document.getElementById("guide-search");
  const count = document.getElementById("guide-count");
  assert.equal(field.value, "");
  assert.equal(count.textContent, "");
  assert.equal(count.getAttribute("role"), "status", "nombre de résultats annoncé");
  assert.deepEqual(await audit(dialog), []);

  // Ctrl+F : le champ de recherche.
  key(document.getElementById("guide-content"), "f", { ctrlKey: true });
  assert.equal(document.activeElement, field);
  // « EMPREINTE » sans accent ni casse ; Entrée lance la recherche sans fermer la boîte.
  field.value = "EMPREINTE";
  key(field, "Enter");
  assert.ok(dialog.open, "Entrée ne ferme pas le guide");
  const hits = document.querySelectorAll("#guide-content mark.guide-hit");
  assert.ok(hits.length >= 5, `${hits.length} résultats`);
  assert.match(count.textContent, new RegExp(`^1 sur ${hits.length}$`));
  assert.equal(document.querySelectorAll("#guide-content mark.current").length, 1);
  assert.match(hits[0].textContent, /^empreinte$/i);
  key(field, "Enter");
  assert.match(count.textContent, /^2 sur /);
  assert.ok(hits[1].classList.contains("current"));
  key(field, "Enter", { shiftKey: true });
  assert.match(count.textContent, /^1 sur /);
  document.getElementById("guide-prev").click();
  assert.match(count.textContent, new RegExp(`^${hits.length} sur `), "en boucle");
  key(dialog, "F3");
  assert.match(count.textContent, /^1 sur /);
  // « deja » trouve « déjà ».
  field.value = "deja";
  key(field, "Enter");
  assert.ok(document.querySelector("#guide-content mark.guide-hit")?.textContent.includes("é"));
  // Rien de trouvé : dit, et les boutons sont inactifs.
  field.value = "zzqqxx";
  key(field, "Enter");
  assert.equal(count.textContent, "Aucun résultat");
  assert.equal(document.querySelectorAll("#guide-content mark").length, 0, "marques retirées");
  assert.ok(document.getElementById("guide-next").disabled);

  // Imprimer : une copie du guide, sans identifiants, par la fenêtre de l'application.
  const before = calls.length;
  document.getElementById("guide-print").click();
  await settle();
  assert.ok(calls.slice(before).includes("print_page"));
  const area = document.getElementById("print-area");
  assert.ok(area.querySelectorAll("section").length >= 10, "tout le guide");
  assert.equal(area.querySelectorAll("[id]").length, 0, "aucun identifiant en double");
  dialog.close();
  await settle();
  assert.equal(area.childNodes.length, 0, "copie retirée à la fermeture");

  // Rouvert : la recherche repart de zéro.
  key(document.body, "F1");
  await settle();
  assert.equal(field.value, "");
  assert.equal(count.textContent, "");
  dialog.close();
});

test("boîtes déplaçables : par le titre, par Alt+flèches, gardées à la réouverture", async () => {
  await closeAll();
  const document = window.document;
  // Chaque boîte a son titre pour poignée.
  for (const dialog of document.querySelectorAll("dialog")) {
    const title = document.getElementById(dialog.getAttribute("aria-labelledby"));
    assert.ok(title?.classList.contains("drag-handle"), dialog.id);
  }
  document.getElementById("tb-settings").click();
  await settle();
  const settings = document.getElementById("settings");
  assert.ok(settings.open);
  assert.equal(settings.dataset.offset, undefined, "au centre");
  // Au clavier, depuis n'importe quel élément de la boîte.
  const tab = document.getElementById("tab-general");
  tab.focus();
  key(tab, "ArrowRight", { altKey: true });
  assert.equal(settings.dataset.offset, "24 0");
  key(tab, "ArrowDown", { altKey: true });
  assert.equal(settings.dataset.offset, "24 24");
  assert.equal(document.activeElement, tab, "le focus ne bouge pas");
  // Alt+flèche vers le bas dans une liste déroulante : la liste, pas la boîte.
  const select = document.getElementById("pref-lang");
  select.focus();
  key(select, "ArrowDown", { altKey: true });
  assert.equal(settings.dataset.offset, "24 24");
  // Rouverte : à la même place.
  settings.close();
  await settle();
  document.getElementById("tb-settings").click();
  await settle();
  assert.equal(settings.dataset.offset, "24 24", "position gardée");
  // Alt+Début : au centre.
  key(document.getElementById("tab-general"), "Home", { altKey: true });
  assert.equal(settings.dataset.offset, undefined);
  // À la souris, par le titre.
  const title = document.getElementById("settings-title");
  const pointer = (type, x, y) => title.dispatchEvent(new window.MouseEvent(type, { clientX: x, clientY: y, button: 0, bubbles: true, cancelable: true }));
  pointer("pointerdown", 100, 100);
  pointer("pointermove", 160, 130);
  assert.equal(settings.dataset.offset, "60 30");
  pointer("pointerup", 160, 130);
  pointer("pointermove", 300, 300);
  assert.equal(settings.dataset.offset, "60 30", "plus rien après le relâchement");
  settings.close();
  await settle();
});

/// Les sections visées par « Raccourcis clavier » existent dans les deux copies du guide.
test("menu « Aide » : la section des raccourcis existe dans chaque langue", () => {
  for (const [langue, id] of [["fr", "guide-13-accessibilite-clavier-dans-la-fenêtre"], ["en", "guide-13-accessibility-keyboard-in-the-window"]]) {
    const html = readFileSync(new URL(`../src/guide/${langue}.html`, import.meta.url), "utf8");
    assert.ok(html.includes(`id="${id}"`), `${langue} : ${id}`);
  }
});

test("Options, chaque rubrique", async () => {
  await closeAll();
  window.document.getElementById("tb-settings").click();
  await settle();
  const settings = window.document.getElementById("settings");
  assert.ok(settings.open, "Options ouvertes");
  const tabs = [...settings.querySelectorAll('[role="tab"]')].filter((tab) => !tab.hidden && !tab.classList.contains("lacks"));
  assert.ok(tabs.length >= 8, tabs.map((tab) => tab.id).join(", "));
  for (const tab of tabs) {
    tab.click();
    await settle();
    assert.equal(tab.getAttribute("aria-selected"), "true", tab.id);
    assert.deepEqual(await audit(settings), [], tab.id);
  }
});

for (const [button, dialog] of [["tb-add", "add-dialog"], ["tb-history", "history-dialog"], ["tb-schedule", "schedule-dialog"]]) {
  test(`boîte ${dialog} ouverte par la barre d'outils`, async () => {
    await closeAll();
    window.document.getElementById(button).click();
    await settle();
    const box = window.document.getElementById(dialog);
    assert.ok(box.open, `${dialog} ouverte`);
    assert.deepEqual(await audit(box), []);
  });
}

// Une boîte modale rend le reste de la page inerte : un message hors d'elle n'est pas lu
// (essai NVDA du 2026-11-18, ajout refusé muet). Il doit être dans la boîte au premier plan.
/** Description d'un champ, telle que la lit un lecteur d'écran (`aria-describedby`). */
function described(field) {
  return (field.getAttribute("aria-describedby") ?? "").split(/\s+/).filter(Boolean)
    .map((id) => window.document.getElementById(id)?.textContent.trim() ?? "").join(" ");
}

function assertAlertIn(dialog, pattern) {
  const document = window.document;
  assert.equal(document.getElementById("error").hidden, true, "bandeau de la fenêtre, inerte derrière la boîte");
  const alerts = [...dialog.querySelectorAll('[role="alert"]')].filter((el) => !el.hidden && el.textContent.trim());
  assert.equal(alerts.length, 1, dialog.innerHTML.slice(-300));
  assert.match(alerts[0].textContent, pattern);
}

test("boîte d'ajout : un ajout refusé est annoncé dans la boîte rouverte, le curseur dans le champ", async () => {
  await closeAll();
  const document = window.document;
  document.getElementById("tb-add").click();
  await settle();
  const box = document.getElementById("add-dialog");
  let closes = 0;
  box.addEventListener("close", () => { closes += 1; }, { once: true });
  const urls = document.getElementById("urls");
  urls.value = "pas-une-adresse";
  // Une alerte seule est coupée par l'annonce du focus (essai NVDA) : le champ porte déjà
  // l'erreur quand il le prend, et le lecteur d'écran la lit avec lui.
  const heard = [];
  const onFocus = () => heard.push(described(urls));
  urls.addEventListener("focus", onFocus);
  const start = document.getElementById("add-start");
  start.focus();
  start.click();
  await settle();
  urls.removeEventListener("focus", onFocus);
  // Refermée puis rouverte, elle était relue en entier, le message en dernier (essai NVDA).
  assert.ok(box.open, "boîte restée ouverte");
  assert.equal(closes, 0, "jamais fermée");
  assertAlertIn(box, /refusé.*adresse invalide/);
  assert.equal(document.activeElement?.id, "urls");
  assert.ok(heard.length > 0 && heard.every((text) => /refusé.*adresse invalide/.test(text)), heard.join(" | "));
  assert.equal(urls.getAttribute("aria-invalid"), "true");
  assert.deepEqual(await audit(box), []);
  // Fermée, elle oublie le message.
  box.close("cancel");
  await settle();
  assert.equal([...box.querySelectorAll('[role="alert"]')].filter((el) => !el.hidden).length, 0);
  assert.equal(urls.hasAttribute("aria-invalid"), false);
  assert.equal(described(urls), "");
  urls.value = "";
});

// Audit W11 : moteur perdu, « Démarrer » garde la boîte ouverte, la saisie et le dit.
test("boîte d'ajout : moteur perdu, la boîte reste ouverte et le dit", async () => {
  await closeAll();
  const document = window.document;
  document.getElementById("tb-add").click();
  await settle();
  const box = document.getElementById("add-dialog");
  const urls = document.getElementById("urls");
  urls.value = "https://exemple.org/garde.iso";
  listeners.engine({ payload: { kind: "status", state: "down", message: "moteur injoignable" } });
  await settle();
  const before = calls.length;
  document.getElementById("add-start").click();
  await settle();
  assert.ok(box.open, "boîte restée ouverte");
  assert.equal(urls.value, "https://exemple.org/garde.iso", "saisie gardée");
  assertAlertIn(box, /moteur ne répond pas/);
  assert.ok(!calls.slice(before).includes("task.add"));
  listeners.engine({ payload: { kind: "status", state: "up", engine: { engine: { version: "0.1.0" }, capabilities: ["media", "linkscan", "crawl"], scope: "admin" } } });
  await settle();
  box.close("cancel");
  await settle();
});

test("boîte d'ajout : un ajout accepté la ferme et rend le focus à la barre d'outils", async () => {
  await closeAll();
  const document = window.document;
  document.getElementById("tb-add").click();
  await settle();
  const box = document.getElementById("add-dialog");
  document.getElementById("urls").value = "https://exemple.org/essai.iso";
  const before = calls.filter((c) => c === "task.add").length;
  document.getElementById("add-start").click();
  await settle();
  assert.equal(calls.filter((c) => c === "task.add").length, before + 1);
  assert.equal(box.open, false, "boîte fermée");
  assert.equal(document.activeElement?.id, "tb-add");
  assert.equal(document.getElementById("error").hidden, true);
});

test("Propriétés : erreur et annonce dans la boîte", async () => {
  await closeAll();
  const document = window.document;
  row("pause").firstElementChild.dispatchEvent(new window.MouseEvent("dblclick", { bubbles: true }));
  await settle();
  const details = document.getElementById("details");
  const input = document.getElementById("d-checksum-input");
  const note = described(input);
  input.value = "pas-une-empreinte";
  document.getElementById("d-checksum-apply").focus();
  document.getElementById("d-checksum-apply").click();
  await settle();
  assertAlertIn(details, /Empreinte illisible/);
  assert.equal(document.activeElement?.id, "d-checksum-input");
  assert.match(described(input), /Empreinte illisible/, "erreur lue avec le champ");
  assert.equal(input.getAttribute("aria-invalid"), "true");
  input.value = "";
  document.getElementById("d-checksum-apply").click();
  await settle();
  assert.equal([...details.querySelectorAll('[role="alert"]')].filter((el) => !el.hidden).length, 0, "erreur effacée");
  assert.equal(input.hasAttribute("aria-invalid"), false);
  assert.equal(described(input), note, "description d'origine rendue");
  const status = details.querySelector('[aria-live="polite"][data-live]');
  assert.match(status?.textContent ?? "", /Empreinte retirée/);
  assert.doesNotMatch(document.getElementById("announce").textContent, /Empreinte/, "annonce de la fenêtre, inerte derrière la boîte");
});

test("mises à jour : « Rechercher maintenant », la version proposée, notes en texte, installation", async () => {
  await closeAll();
  const document = window.document;
  document.getElementById("tb-settings").click();
  await settle();
  const section = document.getElementById("updates-section");
  assert.equal(section.classList.contains("lacks"), false, "rubrique visible : cette installation se met à jour");
  document.getElementById("update-check-now").click();
  await settle();
  const dialog = document.getElementById("update-dialog");
  assert.ok(dialog.open, "version proposée par-dessus les Options");
  assert.match(document.getElementById("update-text").textContent, /0\.2\.0.*0\.1\.0/);
  const notes = document.getElementById("update-notes");
  assert.match(notes.textContent, /<img src=x/, "notes montrées telles quelles");
  assert.equal(dialog.querySelector("img"), null, "rien d'interprété comme du HTML");
  assert.equal(globalThis.piege, undefined);
  assert.equal(document.getElementById("update-install").hidden, false);
  assert.equal(document.activeElement?.id, "update-install");
  assert.deepEqual(await audit(dialog), []);
  document.getElementById("update-install").click();
  await settle();
  assert.equal(UPDATE.installs, 1, "installation demandée à l'hôte");
  assert.equal(document.getElementById("update-progress-row").hidden, false);
  listeners["update-progress"]({ payload: { done: 512, total: 1024 } });
  assert.equal(document.getElementById("update-progress").value, 512);
  dialog.close("later");
  await settle();
  assert.ok(document.getElementById("settings").open, "les Options restent ouvertes");
});

test("mises à jour : à jour, l'état le dit dans la rubrique", async () => {
  await closeAll();
  const document = window.document;
  UPDATE.next = null;
  try {
    document.getElementById("tb-settings").click();
    await settle();
    document.getElementById("update-check-now").click();
    await settle();
    assert.equal(document.getElementById("update-dialog").open, false);
    assert.match(document.getElementById("update-state").textContent, /à jour \(version 0\.1\.0\)/);
  } finally {
    UPDATE.next = FOUND;
  }
});

test("mises à jour : paquet .deb, la version signalée avec sa page, sans installation", async () => {
  await closeAll();
  const document = window.document;
  UPDATE.status.kind = "notify";
  try {
    // Trouvée par la recherche de fond, aucune boîte ouverte.
    listeners["update-available"]({ payload: FOUND });
    await settle();
    const dialog = document.getElementById("update-dialog");
    assert.ok(dialog.open);
    assert.equal(document.getElementById("update-install").hidden, true, "pas d'installation pour un .deb");
    assert.equal(document.getElementById("update-page-note").hidden, false);
    assert.equal(document.activeElement?.id, "update-page");
    document.getElementById("update-page").click();
    await settle();
    assert.deepEqual(UPDATE.pages, ["0.2.0"]);
    dialog.close("later");
  } finally {
    UPDATE.status.kind = "install";
  }
});

test("mises à jour : trouvée pendant qu'une autre boîte est ouverte, proposée à sa fermeture", async () => {
  await closeAll();
  const document = window.document;
  row("pause").firstElementChild.dispatchEvent(new window.MouseEvent("dblclick", { bubbles: true }));
  await settle();
  listeners["update-available"]({ payload: FOUND });
  await settle();
  const dialog = document.getElementById("update-dialog");
  assert.equal(dialog.open, false, "pas par-dessus les Propriétés");
  document.getElementById("details").close();
  await settle();
  assert.ok(dialog.open, "proposée à la fermeture");
  dialog.close("later");
});

test("guide de l'utilisateur : F1, le fragment de la langue, liens internes qui donnent le focus", async () => {
  await closeAll();
  const document = window.document;
  key(document.body, "F1");
  await settle();
  const dialog = document.getElementById("guide-dialog");
  assert.ok(dialog.open, "guide ouvert par F1");
  const content = document.getElementById("guide-content");
  assert.equal(content.dataset.lang, "fr");
  assert.equal(document.activeElement, content, "focus dans le guide, au début");
  assert.ok(content.querySelectorAll("section").length >= 10);
  assert.equal(content.querySelector("script"), null);
  // Le sommaire (première section) mène aux autres : le titre visé prend le focus.
  const link = content.querySelector('a[href="#guide-07-organisation"]');
  assert.ok(link, "lien du sommaire vers Organisation");
  link.click();
  assert.equal(document.activeElement?.id, "guide-07-organisation");
  const anchor = content.querySelector('a[href="#guide-07-organisation-règles-de-rangement"]');
  anchor.click();
  assert.equal(document.activeElement?.id, "guide-07-organisation-règles-de-rangement");
  assert.equal(window.location.hash, "", "l'adresse de la page ne change pas");
  assert.deepEqual(await audit(dialog), []);
  dialog.close();
  await settle();
  // Aussi depuis Options, À propos.
  document.getElementById("tb-settings").click();
  await settle();
  document.getElementById("guide-open").click();
  await settle();
  assert.ok(dialog.open, "guide ouvert depuis les Options");
  dialog.close();
});

test("action de fin annoncée pendant qu'une boîte est ouverte : dite dans la boîte", async () => {
  await closeAll();
  const document = window.document;
  document.getElementById("tb-add").click();
  await settle();
  notify("engine.endAction", { pending: { action: "sleep", queueId: "default", at: NOW + 60_000 } });
  try {
    await settle();
    assert.equal(document.getElementById("end-action").hidden, false, "bandeau de la fenêtre");
    const status = document.getElementById("add-dialog").querySelector('[aria-live="polite"][data-live]');
    assert.match(status?.textContent ?? "", /Mise en veille/);
  } finally {
    // Sinon le décompte du bandeau tient Node en vie.
    notify("engine.endAction", { pending: null });
    await settle();
  }
  assert.equal(document.getElementById("end-action").hidden, true);
});

test("clavier : Entrée ouvre les Propriétés, le focus y entre puis revient à la ligne", async () => {
  await closeAll();
  const body = window.document.querySelector("#grid tbody");
  row("pause").focus();
  key(body, "Home");
  key(body, "ArrowDown");
  key(body, "ArrowDown");
  await settle();
  const third = body.rows[2].dataset.taskId;
  assert.equal(window.document.activeElement?.dataset.taskId, third, "flèches : la ligne suivante prend le focus");
  key(body, "Enter");
  await settle();
  const details = window.document.getElementById("details");
  assert.ok(details.open);
  assert.ok(details.contains(window.document.activeElement), `focus dans les Propriétés : ${window.document.activeElement?.outerHTML.slice(0, 80)}`);
  details.close();
  await settle();
  assert.equal(window.document.activeElement?.dataset.taskId, third, "focus rendu à la ligne");
});

test("clavier : Échap ferme le menu et rend le focus à la ligne", async () => {
  await closeAll();
  const body = window.document.querySelector("#grid tbody");
  row("fini").focus();
  key(body, "F10", { shiftKey: true });
  await settle();
  const menu = window.document.getElementById("menu");
  assert.equal(menu.hidden, false);
  key(window.document.activeElement, "ArrowDown");
  assert.equal(window.document.activeElement?.getAttribute("role"), "menuitem");
  key(window.document.activeElement, "Escape");
  await settle();
  assert.equal(menu.hidden, true, "menu fermé");
  assert.equal(window.document.activeElement?.dataset.taskId, "fini", "focus rendu à la ligne");
});

test("chaque élément qui prend le focus a un nom", () => {
  const focusable = [...window.document.querySelectorAll("button, input, select, textarea, [tabindex]")]
    .filter((el) => el.tabIndex >= 0 && !el.closest("[hidden], .lacks, dialog:not([open])") && !el.disabled);
  assert.ok(focusable.length > 10);
  const nameless = focusable.filter((el) => {
    const labelled = el.getAttribute("aria-labelledby")?.split(/\s+/).map((id) => window.document.getElementById(id)?.textContent.trim()).join("");
    return !(el.getAttribute("aria-label") || labelled || el.labels?.length || el.textContent.trim() || el.title);
  });
  assert.deepEqual(nameless.map((el) => el.outerHTML.slice(0, 100)), []);
});

test("exploration d'un site : modèle, avancement, colonnes, arborescence, arrêt", async () => {
  await closeAll();
  const document = window.document;
  const crawls = [];
  const cancels = [];
  const batches = [];
  let ending = "done";
  ENGINE["linkscan.crawl"] = (params) => {
    crawls.push(params);
    return { crawlId: `crawl-${crawls.length}` };
  };
  ENGINE["linkscan.crawlResult"] = ({ crawlId }) => ({
    crawlId, state: ending, stop: ending === "done" ? "maxPages" : undefined, pagesRead: 3, pagesQueued: 0,
    bytesRead: 10, pageUrl: "https://exemple.org/",
    links: [
      { url: "https://exemple.org/docs/a.pdf", name: "a.pdf", ext: "pdf", kind: "file", depth: 2, foundOn: "https://exemple.org/docs/", sitePath: "exemple.org/docs" },
      { url: "https://exemple.org/p.html", name: "p.html", ext: "html", kind: "page", depth: 1, foundOn: "https://exemple.org/", sitePath: "exemple.org", visit: "robots" },
    ],
  });
  ENGINE["linkscan.cancel"] = ({ crawlId }) => {
    cancels.push(crawlId);
    return { crawlId, state: "cancelled", pagesRead: 1, pagesQueued: 0, links: 2, bytesRead: 0 };
  };
  ENGINE["linkscan.probe"] = ({ urls }) => ({ items: urls.map((url) => ({ url, size: 1000, name: "x", kind: null })) });
  ENGINE["task.addBatch"] = ({ items }) => {
    batches.push(items);
    return { tasks: items.map((_, index) => ({ index, taskId: `t${index}` })), errors: [] };
  };
  const progress = (crawlId, params) => listeners.engine({ payload: { kind: "message", message: {
    method: "linkscan.progress", params: { rev, crawlId, pagesQueued: 0, bytesRead: 0, ...params } } } });
  const $ = (id) => document.getElementById(id);
  try {
    $("tb-links").click();
    await settle();
    assert.ok($("links-dialog").open);
    assert.equal($("l-crawl-box").hidden, false);
    assert.deepEqual(await audit($("links-dialog")), []);

    // Un modèle : exploration cochée, profondeur et extensions remplies ; changé à la main,
    // le modèle redevient « Personnalisé ».
    $("l-template").value = "documents";
    $("l-template").dispatchEvent(new window.Event("change"));
    assert.equal($("l-crawl").checked, true);
    assert.equal($("l-depth").value, "1");
    assert.match($("l-ext").value, /\bpdf\b/);
    $("l-depth").value = "3";
    $("l-depth").dispatchEvent(new window.Event("input"));
    assert.equal($("l-template").value, "");
    $("l-depth").value = "1";
    $("l-depth").dispatchEvent(new window.Event("input"));
    assert.equal($("l-template").value, "documents");

    // Un texte : l'exploration demande une page.
    $("l-source").value = "voir https://exemple.org/a.zip et https://exemple.org/b.zip";
    $("l-scan").click();
    await settle();
    assert.equal(crawls.length, 0);
    assert.match($("l-error").textContent, /adresse d'une page/);
    // Une profondeur hors des bornes : refusée, liée au champ.
    $("l-source").value = "https://exemple.org/";
    $("l-depth").value = "9";
    $("l-scan").click();
    await settle();
    assert.equal(crawls.length, 0);
    assert.equal(document.activeElement, $("l-depth"));
    assert.equal($("l-depth").getAttribute("aria-invalid"), "true");
    $("l-depth").value = "1";

    // Une page : lancée avec les réglages ; l'avancement s'affiche, la fin remplit le tableau.
    $("l-scan").click();
    await settle();
    assert.deepEqual(crawls[0], {
      pageUrl: "https://exemple.org/", depth: 1, maxPages: 200,
      exclude: "(?i)logout|signout|deconnexion|déconnexion|[?&;]C=[NMSD][;&]O=[AD]", sameFolder: false, ignoreRobots: false,
    });
    assert.equal($("l-crawling").hidden, false);
    assert.equal($("l-scan").disabled, true);
    progress("autre", { state: "running", pagesRead: 9, links: 9 });
    progress("crawl-1", { state: "running", pagesRead: 2, links: 5, pagesQueued: 3 });
    assert.equal($("l-progress").textContent, "2 page(s) lue(s), 3 en attente, 5 lien(s) trouvé(s)");
    progress("crawl-1", { state: "done", pagesRead: 3, links: 2, stop: "maxPages" });
    await settle();
    assert.equal($("l-crawling").hidden, true);
    assert.equal($("l-scan").disabled, false);
    assert.match($("l-status").textContent, /^2 lien\(s\) trouvé\(s\) en explorant https:\/\/exemple\.org\/ \(3 page\(s\) lue\(s\)\) Limite de pages/);
    const table = $("l-links");
    assert.ok(table.classList.contains("crawled"));
    let rows = [...table.tBodies[0].rows];
    assert.equal(rows.length, 1, "filtre des documents : la page est masquée");
    assert.equal(rows[0].cells[4].textContent, "2");
    assert.equal(rows[0].cells[5].textContent, "https://exemple.org/docs/");
    // Les pages montrées disent ce qu'il en est advenu.
    $("l-ext").value = "";
    $("l-ext").dispatchEvent(new window.Event("input"));
    $("l-all").checked = true;
    $("l-all").dispatchEvent(new window.Event("change"));
    rows = [...table.tBodies[0].rows];
    assert.equal(rows[1].cells[4].textContent, "1 (non lue : robots.txt)");
    $("l-all").checked = false;
    $("l-all").dispatchEvent(new window.Event("change"));

    // Arborescence du site : le sous-dossier de chaque lien.
    assert.equal($("l-tree-box").hidden, false);
    $("l-tree").checked = true;
    $("l-start").click();
    await settle();
    assert.equal(batches.length, 1, calls.slice(-5).join(", "));
    assert.equal(batches[0][0].subDir, "exemple.org/docs");
    assert.equal($("links-dialog").open, false);

    // « Arrêter l'exploration » : ce qui a été trouvé est gardé.
    ending = "cancelled";
    $("tb-links").click();
    await settle();
    $("l-source").value = "https://exemple.org/";
    $("l-crawl").checked = true;
    $("l-scan").click();
    await settle();
    $("l-stop").focus();
    $("l-stop").click();
    await settle();
    assert.deepEqual(cancels, ["crawl-2"]);
    assert.match($("l-status").textContent, /^Exploration arrêtée : 2 lien\(s\)/);
    assert.equal($("l-crawling").hidden, true);
    assert.equal(document.activeElement, $("l-scan"), "focus rendu à « Analyser »");

    // Fermer la boîte pendant une exploration l'arrête.
    $("l-scan").click();
    await settle();
    $("links-dialog").close();
    await settle();
    assert.deepEqual(cancels, ["crawl-2", "crawl-3"]);
    assert.equal($("l-crawling").hidden, true);
  } finally {
    for (const m of ["linkscan.crawl", "linkscan.crawlResult", "linkscan.cancel", "linkscan.probe", "task.addBatch"]) delete ENGINE[m];
    $("l-crawl").checked = false;
    await closeAll();
  }
});

test("copie d'un site : case de la boîte « Liens », ligne, Propriétés, Ouvrir", async () => {
  await closeAll();
  const document = window.document;
  const $ = (id) => document.getElementById(id);
  // Un moteur qui sait copier un site (capacité `site`).
  listeners.engine({ payload: { kind: "status", state: "up", engine: { engine: { version: "0.1.0" }, capabilities: ["media", "linkscan", "crawl", "site"], scope: "admin" } } });
  await settle();
  // La page a relu l'instantané du faux moteur, à la révision 1.
  rev = 1;
  const adds = [];
  const plain = { add: ENGINE["task.add"], get: ENGINE["task.get"] };
  ENGINE["linkscan.crawl"] = () => ({ crawlId: "crawl-site" });
  ENGINE["linkscan.crawlResult"] = ({ crawlId }) => ({
    crawlId, state: "done", stop: "finished", pagesRead: 2, pagesQueued: 0, bytesRead: 10, pageUrl: "https://exemple.org/",
    links: [
      { url: "https://exemple.org/docs/a.pdf", name: "a.pdf", ext: "pdf", kind: "file", depth: 1, foundOn: "https://exemple.org/", sitePath: "exemple.org/docs" },
      { url: "https://exemple.org/p.html", name: "p.html", ext: "html", kind: "page", depth: 1, foundOn: "https://exemple.org/", sitePath: "exemple.org" },
      { url: "magnet:?xt=urn:btih:abc", name: "abc", ext: null, kind: "magnet", depth: 1, foundOn: "https://exemple.org/", sitePath: "" },
    ],
  });
  ENGINE["linkscan.probe"] = ({ urls }) => ({ items: urls.map((url) => ({ url, size: 1000, name: "x", kind: null })) });
  ENGINE["task.add"] = (params) => {
    adds.push(params);
    return { taskId: "copie" };
  };
  try {
    $("tb-links").click();
    await settle();
    $("l-source").value = "https://exemple.org/";
    $("l-crawl").checked = true;
    $("l-depth").value = "2";
    $("l-scan").click();
    await settle();
    listeners.engine({ payload: { kind: "message", message: { method: "linkscan.progress", params: {
      rev, crawlId: "crawl-site", state: "done", pagesRead: 2, pagesQueued: 0, links: 3, bytesRead: 10, stop: "finished" } } } });
    await settle();
    assert.equal($("l-site-box").hidden, false);
    assert.equal($("l-tree").disabled, false);
    $("l-site").checked = true;
    $("l-site").dispatchEvent(new window.Event("change"));
    assert.equal($("l-tree").disabled, true, "la copie garde toujours l'arborescence");
    assert.deepEqual(await audit($("links-dialog")), []);
    // Sans fichier coché, la copie se fait quand même.
    $("l-all").checked = true;
    $("l-all").dispatchEvent(new window.Event("change"));
    $("l-check-none").click();
    assert.equal($("l-start").disabled, false);
    $("l-check-all").click();
    // Les réglages de l'exploration lancée, même changés depuis ; ni page ni lien magnet.
    $("l-depth").value = "5";
    $("l-later").click();
    await settle();
    assert.deepEqual(adds, [{
      site: {
        pageUrl: "https://exemple.org/", depth: 2, maxPages: 200,
        exclude: "(?i)logout|signout|deconnexion|déconnexion|[?&;]C=[NMSD][;&]O=[AD]", sameFolder: false, ignoreRobots: false,
        files: ["https://exemple.org/docs/a.pdf"],
      },
      startPaused: true,
    }]);
    assert.equal($("links-dialog").open, false);
    assert.equal($("l-site").checked, false);
    assert.equal($("l-tree").disabled, false);

    // La ligne : « Site : … », ses éléments au lieu d'un pourcentage.
    const copy = task("copie", "active", {
      kind: "site", displayName: "exemple.org", fileName: "exemple.org", totalBytes: null, doneBytes: 3000,
    });
    notify("task.added", { task: copy });
    await settle();
    listeners.engine({ payload: { kind: "message", message: { method: "task.progress", params: { rev, items: [{
      taskId: "copie", doneBytes: 4000, totalBytes: null, bps: 100, connections: 0, etaS: null,
      site: { pages: 3, resources: 7, files: 0, failed: 1, ignored: 5, queued: 2 } }] } } } });
    await settle();
    assert.equal(row("copie").querySelector(".name .text").textContent, "Site : exemple.org");
    assert.equal(row("copie").querySelector(".pct").textContent, "10 téléchargée(s), 1 échec(s), 5 ignorée(s)");
    assert.equal(row("copie").querySelector(".pct").title, "10 téléchargée(s), 1 échec(s), 5 ignorée(s)");

    // Finie : Propriétés de la copie, éléments en échec, « Ouvrir » vers la page de départ.
    const site = {
      pageUrl: "https://exemple.org/", startPath: "exemple.org/index.html", depth: 2, maxPages: 200,
      maxBytes: 1024 * MIB, exclude: "", sameFolder: true, ignoreRobots: false, stop: "maxPages",
      pages: 3, resources: 7, files: 1, failed: 1, ignored: 6, skipped: 4, queued: 0,
      items: [
        { url: "https://exemple.org/", kind: "page", state: "done", path: "exemple.org/index.html", size: 100, error: null },
        { url: "https://exemple.org/manque.png", kind: "resource", state: "failed", path: null, size: null, error: "HTTP_STATUS" },
        { url: "https://exemple.org/LISEZMOI", kind: "page", state: "ignored", path: null, size: null, error: "NOT_HTML" },
        { url: "https://ailleurs.org/", kind: "page", state: "ignored", path: null, size: null, error: "OFFSITE" },
        { url: "https://exemple.org/p.html", kind: "page", state: "skipped", path: null, size: null, error: null },
      ],
    };
    ENGINE["task.get"] = ({ taskId }) => taskId === "copie"
      ? { ...copy, state: "completed", finishedAt: NOW, doneBytes: 5000, totalBytes: 5000, site }
      : plain.get({ taskId });
    notify("task.stateChanged", { taskId: "copie", from: "active", to: "completed" });
    await settle();
    row("copie").firstElementChild.dispatchEvent(new window.MouseEvent("dblclick", { bubbles: true }));
    await settle();
    assert.ok($("details").open);
    assert.equal($("d-site-section").hidden, false);
    assert.equal($("d-name").textContent, "exemple.org", "le dossier de la copie");
    assert.notEqual($("d-size").textContent, "inconnue");
    assert.equal($("d-site-page").textContent, "https://exemple.org/");
    assert.equal($("d-site-start").textContent, "exemple.org/index.html");
    assert.match($("d-site-settings").textContent, /^profondeur 2, 200 pages au plus, .* au plus, sous le dossier de départ$/);
    assert.equal($("d-site-counts").textContent, "11 (3 page(s), 7 ressource(s), 1 fichier(s))");
    assert.equal($("d-site-failed-count").textContent, "1");
    assert.equal($("d-site-ignored-count").textContent, "6");
    assert.equal($("d-site-stop").textContent, "arrêtée : limite de pages atteinte");
    const failed = [...$("d-site-failed").tBodies[0].rows].map((r) => [...r.cells].map((c) => c.textContent));
    assert.deepEqual(failed, [["https://exemple.org/manque.png", "refusé par le serveur"]]);
    // Laissés en ligne par choix : à part des échecs techniques, avec leur raison.
    assert.equal($("d-site-ignored-box").hidden, false);
    const ignored = [...$("d-site-ignored").tBodies[0].rows].map((r) => [...r.cells].map((c) => c.textContent));
    assert.deepEqual(ignored, [
      ["https://exemple.org/LISEZMOI", "pas une page web"],
      ["https://ailleurs.org/", "autre site"],
      ["https://exemple.org/p.html", "limite atteinte"],
    ]);
    assert.equal($("d-open").hidden, false);
    assert.deepEqual(await audit($("details")), []);
    const before = calls.length;
    $("d-open").click();
    await settle();
    assert.ok(calls.slice(before).includes("open_site"), calls.slice(before).join(", "));
    assert.ok(!calls.slice(before).includes("open_file"));
  } finally {
    for (const m of ["linkscan.crawl", "linkscan.crawlResult", "linkscan.probe"]) delete ENGINE[m];
    ENGINE["task.add"] = plain.add;
    ENGINE["task.get"] = plain.get;
    $("l-crawl").checked = false;
    $("l-all").checked = false;
    $("l-depth").value = "1";
    await closeAll();
    notify("task.removed", { taskId: "copie" });
    await settle();
  }
});

test("projets : boîte « Projets », « Enregistrer comme projet… », « Modifier », ligne « Fichiers : … »", async () => {
  await closeAll();
  const document = window.document;
  const $ = (id) => document.getElementById(id);
  listeners.engine({ payload: { kind: "status", state: "up", engine: { engine: { version: "0.1.0" }, capabilities: ["media", "linkscan", "crawl", "site", "projects"], scope: "admin" } } });
  await settle();
  rev = 1;
  const sent = [];
  for (const method of ["project.set", "project.run", "project.delete"]) {
    ENGINE[method] = (params) => {
      sent.push([method, params]);
      if (method === "project.set") return { projectId: params.projectId ?? "neuf" };
      return method === "project.run" ? { taskId: "projtache" } : {};
    };
  }
  const docs = {
    projectId: "p1", name: "Docs", pageUrl: "https://exemple.org/docs/", depth: 2, maxPages: 200, maxBytes: 1024 * MIB,
    exclude: null, sameFolder: false, ignoreRobots: false,
    filters: { extensions: ["pdf"], regex: null, minSize: MIB, maxSize: null },
    copySite: false, keepTree: true, destDir: "/home/essai/Docs", queueId: null,
    schedule: { time: "03:30", days: [1, 2, 3, 4, 5] }, nextRunAt: NOW + 3600_000,
    taskId: "projtache", lastRunAt: NOW - 60_000, lastSkippedAt: null, publicOnly: false,
  };
  try {
    assert.equal($("tb-projects").hidden, false);
    notify("project.updated", { projectId: "p1", project: docs });
    notify("task.added", { task: task("projtache", "completed", { kind: "site", displayName: "exemple.org", fileName: "exemple.org", totalBytes: null }) });
    await settle();
    // Fichiers seuls d'un projet : le nom de la ligne le dit, avant tout avancement.
    assert.equal(row("projtache").querySelector(".name .text").textContent, "Fichiers : exemple.org");

    $("tb-projects").click();
    await settle();
    assert.ok($("projects-dialog").open);
    const cells = [...$("pj-table").tBodies[0].rows[0].cells].map((c) => c.textContent);
    assert.equal(cells[0], "Docs");
    assert.equal(cells[1], "https://exemple.org/docs/");
    assert.match(cells[2], / à 03:30$/);
    assert.match(cells[4], /Terminé/);
    assert.deepEqual(await audit($("projects-dialog")), []);

    // « Lancer » : la copie du projet.
    $("pj-table").querySelector('button[data-action="run"]').click();
    await settle();
    assert.deepEqual(sent.pop(), ["project.run", { projectId: "p1" }]);

    // « Modifier » : la boîte « Liens » remplie, en mode projet ; « Projets » se ferme.
    $("pj-table").querySelector('button[data-action="edit"]').click();
    await settle();
    assert.equal($("projects-dialog").open, false);
    assert.ok($("links-dialog").open);
    assert.equal(document.activeElement, $("l-p-name"));
    assert.equal($("l-p-name").value, "Docs");
    assert.equal($("l-source").value, "https://exemple.org/docs/");
    assert.equal($("l-depth").value, "2");
    assert.equal($("l-ext").value, "pdf");
    assert.equal($("l-min").value, "1M");
    assert.equal($("l-filters").hidden, false, "les filtres, sans liens analysés");
    assert.equal($("l-site").checked, false);
    assert.equal($("l-tree").checked, true);
    assert.equal($("l-p-scheduled").checked, true);
    assert.equal($("l-p-time").value, "03:30");
    assert.deepEqual([...$("l-p-days").querySelectorAll("input:checked")].map((b) => b.value), ["1", "2", "3", "4", "5"]);
    assert.equal($("l-p-run").checked, false);
    assert.equal($("l-p-back").hidden, true, "ouverte depuis « Projets »");
    assert.equal($("l-start").hidden, true);
    assert.equal($("l-dest").textContent, "/home/essai/Docs");
    assert.deepEqual(await audit($("links-dialog")), []);

    // Un nom vide : refusé, lié au champ, qui prend le focus ; rien d'envoyé.
    $("l-p-name").value = " ";
    $("l-p-save").click();
    await settle();
    assert.equal(sent.length, 0);
    assert.equal(document.activeElement, $("l-p-name"));
    assert.equal($("l-p-name").getAttribute("aria-invalid"), "true");
    assertAlertIn($("links-dialog"), /Nom du projet : de 1 à 100 caractères/);

    // Enregistré : ce que la boîte montre, plus ce qu'elle ne montre pas (dossier, volume).
    $("l-p-name").value = "Docs 2";
    $("l-p-save").click();
    await settle();
    assert.deepEqual(sent.pop(), ["project.set", {
      projectId: "p1", name: "Docs 2", pageUrl: "https://exemple.org/docs/", depth: 2, maxPages: 200, exclude: "",
      sameFolder: false, ignoreRobots: false,
      filters: { extensions: ["pdf"], regex: null, minSize: MIB, maxSize: null },
      copySite: false, keepTree: true, schedule: { time: "03:30", days: [1, 2, 3, 4, 5] },
      destDir: "/home/essai/Docs", maxBytes: 1024 * MIB,
    }]);
    assert.equal(sent.length, 0, "pas de lancement : « Lancer maintenant » décoché");
    assert.equal($("links-dialog").open, false);
    assert.ok($("projects-dialog").open, "« Projets » rouverte");

    // « Supprimer » : envoyé ; la notification retire la ligne.
    $("pj-table").querySelector('button[data-action="delete"]').click();
    await settle();
    assert.deepEqual(sent.pop(), ["project.delete", { projectId: "p1" }]);
    notify("project.updated", { projectId: "p1", project: null });
    await settle();
    assert.equal($("pj-table").tBodies[0].rows.length, 0);
    assert.match($("pj-status").textContent, /^Aucun projet/);
    // Sans son projet, la copie reprend le nom d'une copie, jusqu'à son avancement.
    assert.equal(row("projtache").querySelector(".name .text").textContent, "Site : exemple.org");
    listeners.engine({ payload: { kind: "message", message: { method: "task.progress", params: { rev, items: [{
      taskId: "projtache", doneBytes: 10, totalBytes: null, bps: 0, connections: 0, etaS: null,
      site: { pages: 3, resources: 0, files: 2, failed: 0, ignored: 1, unchanged: 4, queued: 0, mode: "files" } }] } } } });
    await settle();
    assert.equal(row("projtache").querySelector(".name .text").textContent, "Fichiers : exemple.org");
    assert.equal(row("projtache").querySelector(".pct").textContent, "2 téléchargée(s), 4 inchangée(s), 1 ignorée(s)");

    // Depuis la boîte « Liens » : la page et ses réglages, le nom de son hôte, lancé.
    await closeAll();
    $("tb-links").click();
    await settle();
    $("l-source").value = "https://exemple.org/docs/";
    $("l-depth").value = "1";
    $("l-ext").value = "";
    $("l-min").value = "";
    $("l-tree").checked = false;
    assert.equal($("l-project").hidden, false);
    $("l-project").click();
    await settle();
    assert.equal($("l-project-box").hidden, false);
    assert.equal($("l-p-name").value, "exemple.org");
    assert.equal($("l-p-run").checked, true);
    assert.equal($("l-p-back").hidden, false);
    assert.equal($("l-p-time").disabled, true, "programme décoché");
    $("l-p-scheduled").checked = true;
    $("l-p-scheduled").dispatchEvent(new window.Event("change"));
    assert.equal($("l-p-time").disabled, false);
    $("l-p-days").querySelectorAll("input").forEach((b) => { b.checked = false; });
    $("l-p-save").click();
    await settle();
    assert.deepEqual(sent.shift(), ["project.set", {
      name: "exemple.org", pageUrl: "https://exemple.org/docs/", depth: 1, maxPages: 200,
      exclude: "(?i)logout|signout|deconnexion|déconnexion|[?&;]C=[NMSD][;&]O=[AD]", sameFolder: false, ignoreRobots: false,
      filters: { extensions: [], regex: null, minSize: null, maxSize: null },
      copySite: false, keepTree: false, schedule: { time: "03:00", days: [] },
    }]);
    assert.deepEqual(sent.shift(), ["project.run", { projectId: "neuf" }]);
    assert.equal($("links-dialog").open, false);
    assert.equal(document.activeElement, $("tb-links"));
    assert.equal($("announce").textContent, "Projet « exemple.org » enregistré et lancé");
  } finally {
    for (const m of ["project.set", "project.run", "project.delete"]) delete ENGINE[m];
    $("l-crawl").checked = false;
    $("l-depth").value = "1";
    await closeAll();
    notify("task.removed", { taskId: "projtache" });
    await settle();
  }
});

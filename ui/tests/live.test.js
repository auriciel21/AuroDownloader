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
          case "engine_status": return { state: "up", engine: { engine: { version: "0.1.0" }, capabilities: ["media", "linkscan"], scope: "admin" } };
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
  const up = (scope) => listeners.engine({ payload: { kind: "status", state: "up", engine: { engine: { version: "0.1.0" }, capabilities: ["media", "linkscan"], scope } } });
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
  listeners.engine({ payload: { kind: "status", state: "up", engine: { engine: { version: "0.1.0" }, capabilities: ["media", "linkscan"], scope: "admin" } } });
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

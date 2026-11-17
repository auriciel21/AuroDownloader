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
};

let window;
const calls = [];

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
  proto.showModal = function () {
    this.setAttribute("open", "");
    const candidates = [...this.querySelectorAll("button, input, select, textarea, [tabindex]")]
      .filter((el) => el.tabIndex >= 0 && !el.disabled && !el.closest("[hidden], .lacks"));
    (candidates.find((el) => el.hasAttribute("autofocus")) ?? candidates[0])?.focus();
  };
  proto.show = proto.showModal;
  proto.close = function (value) {
    if (value !== undefined) this.returnValue = value;
    this.removeAttribute("open");
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
          case "app_info": return { lang: "fr", prefs: { lang: "fr", closeToTray: true, showOnAdd: true }, tray: true, executableExtensions: ["exe"], platform: "linux", autostart: false, version: "0.1.0" };
          case "engine_status": return { state: "up", engine: { engine: { version: "0.1.0" }, capabilities: ["media", "linkscan"], scope: "admin" } };
          case "links_status": return { available: false, enabled: false };
          case "take_opened": return [];
          default: return null;
        }
      },
    },
    event: { listen: async () => () => {} },
  };
  window.eval(axe.source);
  // La page est un module : chargée par Node, avec la fenêtre de jsdom comme globale.
  for (const key of Object.getOwnPropertyNames(window)) {
    if (!(key in globalThis)) {
      try { globalThis[key] = window[key]; } catch { /* propriété en lecture seule */ }
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

test("la page s'est remplie depuis le faux moteur", () => {
  assert.equal(window.document.getElementById("error").hidden, true, window.document.getElementById("error").textContent);
  assert.equal(window.document.querySelectorAll("#grid tbody tr").length, TASKS.length, calls.join(", "));
});

test("fenêtre principale, avec ses lignes et son arborescence", async () => {
  assert.deepEqual(await audit(window.document), []);
});

function row(taskId) {
  return window.document.querySelector(`#grid tbody tr[data-task-id="${taskId}"]`);
}

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

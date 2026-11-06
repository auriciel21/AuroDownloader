// Page du prototype Tauri : parle le protocole du moteur (SPEC §6) par la commande `rpc`
// et reçoit ses notifications par l'événement `engine`.
"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const EN = {
  "Adresse à télécharger": "Address to download",
  "Ajouter": "Add",
  "Ajouter le téléchargement": "Add the download",
  "Téléchargements": "Downloads",
  "En attente": "Queued", "En cours": "Downloading", "En pause": "Paused",
  "Nouvel essai bientôt": "Retrying soon", "Terminé": "Completed", "Échec": "Failed",
  "Annulé": "Cancelled",
  "Reprendre": "Resume", "Pause": "Pause", "Annuler": "Cancel",
  "Reprendre {0}": "Resume {0}", "Mettre en pause {0}": "Pause {0}", "Annuler {0}": "Cancel {0}",
  "Avancement de {0}": "Progress of {0}",
  "{0} tâche(s)": "{0} task(s)",
  "Moteur non connecté": "Engine not connected",
  "o": "B", "Kio": "KiB", "Mio": "MiB", "Gio": "GiB", "Tio": "TiB",
};
let lang = "fr";
const t = (text, ...args) => {
  const s = lang === "en" ? (EN[text] ?? text) : text;
  return s.replace(/\{(\d)\}/g, (_, i) => args[i]);
};

const STATES = {
  queued: "En attente", active: "En cours", paused: "En pause", retry_wait: "Nouvel essai bientôt",
  completed: "Terminé", failed: "Échec", cancelled: "Annulé",
};
const stateName = (s) => (STATES[s] ? t(STATES[s]) : s);

function bytes(n) {
  if (n == null || n < 0) return "?";
  const units = ["o", "Kio", "Mio", "Gio", "Tio"];
  let i = 0;
  while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
  const number = n.toLocaleString(lang, { maximumFractionDigits: i === 0 ? 0 : 1, minimumFractionDigits: i === 0 ? 0 : 1 });
  return `${number} ${t(units[i])}`;
}
function duration(s) {
  if (s == null || s < 0) return "";
  if (s < 60) return `${s} s`;
  if (s < 3600) return `${Math.floor(s / 60)} min ${s % 60} s`;
  return `${Math.floor(s / 3600)} h ${Math.floor((s % 3600) / 60)} min`;
}

const tasks = new Map(); // taskId -> {task, row}
const list = document.getElementById("tasks");
const template = document.getElementById("row");
const errorBox = document.getElementById("error");
const statusBar = document.getElementById("status");
let connected = false;
let dest = "";

function showError(text) {
  errorBox.textContent = text;
  errorBox.hidden = !text;
}

const resumable = (s) => s === "paused" || s === "failed";

function render(entry) {
  const { task, row } = entry;
  const name = task.displayName || task.fileName || "";
  row.querySelector(".name").textContent = name;
  const bar = row.querySelector("progress");
  if (task.totalBytes > 0) {
    bar.max = task.totalBytes;
    bar.value = task.doneBytes;
  } else if (task.state === "active") {
    bar.removeAttribute("value");
  } else {
    bar.max = 1;
    bar.value = 0;
  }
  bar.setAttribute("aria-label", t("Avancement de {0}", name));
  const parts = [stateName(task.state), `${bytes(task.doneBytes)} / ${bytes(task.totalBytes ?? -1)}`];
  if (task.state === "active") parts.push(`${bytes(task.bps ?? 0)}/s`);
  const eta = duration(task.etaS);
  if (eta) parts.push(eta);
  row.querySelector(".detail").textContent = parts.join(" · ");
  const toggle = row.querySelector(".toggle");
  toggle.textContent = resumable(task.state) ? t("Reprendre") : t("Pause");
  toggle.setAttribute("aria-label", resumable(task.state) ? t("Reprendre {0}", name) : t("Mettre en pause {0}", name));
  toggle.disabled = !["queued", "active", "retry_wait", "paused", "failed"].includes(task.state);
  const cancel = row.querySelector(".cancel");
  cancel.textContent = t("Annuler");
  cancel.setAttribute("aria-label", t("Annuler {0}", name));
  cancel.disabled = ["completed", "cancelled"].includes(task.state);
}

function upsert(task) {
  let entry = tasks.get(task.taskId);
  if (!entry) {
    const row = template.content.firstElementChild.cloneNode(true);
    entry = { task, row };
    row.querySelector(".toggle").addEventListener("click", () => {
      const method = resumable(entry.task.state) ? "task.resume" : "task.pause";
      call(method, { taskIds: [entry.task.taskId] });
    });
    row.querySelector(".cancel").addEventListener("click", () =>
      call("task.cancel", { taskIds: [entry.task.taskId], deleteFiles: false }));
    tasks.set(task.taskId, entry);
    list.append(row);
  } else {
    entry.task = { ...entry.task, ...task };
  }
  render(entry);
}

function updateStatus() {
  statusBar.textContent = connected ? t("{0} tâche(s)", tasks.size) : t("Moteur non connecté");
}

async function call(method, params) {
  try {
    return await invoke("rpc", { method, params });
  } catch (e) {
    showError(String(e));
    throw e;
  }
}

function onEngine(message) {
  const p = message.params || {};
  switch (message.method) {
    case "task.progress":
      for (const item of p.items) {
        const entry = tasks.get(item.taskId);
        if (!entry) continue;
        Object.assign(entry.task, item);
        // Fenêtre repliée : l'état suit, l'affichage attend son retour.
        if (!document.hidden) render(entry);
      }
      break;
    case "task.stateChanged": {
      const entry = tasks.get(p.taskId);
      if (!entry) break;
      entry.task.state = p.to;
      if (p.to !== "active") { entry.task.bps = 0; entry.task.etaS = null; }
      if (p.to === "completed" && entry.task.totalBytes > 0) entry.task.doneBytes = entry.task.totalBytes;
      render(entry);
      break;
    }
    case "task.added":
      upsert(p.task);
      updateStatus();
      break;
  }
}

function translatePage() {
  document.documentElement.lang = lang;
  for (const el of document.querySelectorAll("[data-i18n]")) el.textContent = t(el.dataset.i18n);
  for (const el of document.querySelectorAll("[data-i18n-placeholder]")) el.placeholder = t(el.dataset.i18nPlaceholder);
  for (const el of document.querySelectorAll("[data-i18n-label]")) el.setAttribute("aria-label", t(el.dataset.i18nLabel));
}

async function start() {
  const config = await invoke("config");
  dest = config.dest;
  lang = config.lang || (navigator.language.startsWith("fr") ? "fr" : "en");
  translatePage();
  invoke("set_lang", { lang });
  updateStatus();

  document.addEventListener("keydown", (e) => {
    if (e.ctrlKey && e.key.toLowerCase() === "l") { e.preventDefault(); document.getElementById("url").focus(); }
  });
  document.getElementById("add-form").addEventListener("submit", (e) => {
    e.preventDefault();
    const input = document.getElementById("url");
    const url = input.value.trim();
    if (!url) return;
    call("task.add", { sources: [{ url }], destDir: dest }).then(() => { input.value = ""; }, () => {});
  });

  document.addEventListener("visibilitychange", () => {
    if (!document.hidden) for (const entry of tasks.values()) render(entry);
  });
  // Fenêtre rouverte : l'avancement reçu pendant le repli manque, l'état complet le remplace.
  await listen("resync", async () => {
    const snapshot = await call("sync.snapshot", {});
    for (const task of snapshot.tasks) upsert(task);
    updateStatus();
  });
  await listen("engine", (event) => onEngine(event.payload));
  await listen("engine-error", (event) => { connected = false; showError(event.payload); updateStatus(); });

  await call("engine.hello", { client: { name: "auro-tauri", version: "0.1" }, protocol: { min: "1.0", max: "1.0" } });
  connected = true;
  await call("events.subscribe", { topics: ["tasks", "progress"], progressIntervalMs: 500 });
  const snapshot = await call("sync.snapshot", {});
  for (const task of snapshot.tasks) upsert(task);
  updateStatus();
  document.getElementById("url").focus();
  // Première image affichée avec les tâches : mesure du démarrage.
  requestAnimationFrame(() => requestAnimationFrame(() => invoke("ready", { epochMs: Date.now() })));
}

start().catch((e) => showError(String(e)));

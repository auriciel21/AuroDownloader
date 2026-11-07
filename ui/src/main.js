// Page de l'application : liste des téléchargements, ajout, actions, filtres, limite
// globale, préférences. Toute action passe par le protocole du moteur (SPEC §6) ; la page
// ne garde que ce qu'elle affiche.
import { translator, stateText, errorText, bytes, duration } from "./i18n.js";
import { TaskStore, FILTERS, canPause, canResume, canCancel, canRemove } from "./model.js";
import { tauriHost } from "./host.js";

const host = tauriHost();
const store = new TaskStore();
let t = translator("fr");
let lang = "fr";
let info = null;
let filter = "all";
let connected = false;
let engineVersion = "";
let executables = new Set();

const $ = (id) => document.getElementById(id);
const list = $("tasks");
const template = $("row");
/// Lignes affichées ou prêtes à l'être, par tâche.
const rows = new Map();

// ---------- Messages ----------

function showError(text) {
  $("error-text").textContent = text;
  $("error").hidden = !text;
}

function refused(e) {
  return t("Action refusée : {0}", e?.message ?? String(e));
}

function announce(text) {
  const box = $("announce");
  box.textContent = "";
  // Un changement de contenu après coup : lu par les lecteurs d'écran.
  requestAnimationFrame(() => { box.textContent = text; });
}

// ---------- Rendu ----------

const FILTER_LABELS = {
  all: "Toutes ({0})",
  active: "En cours ({0})",
  stopped: "Arrêtées ({0})",
  completed: "Terminées ({0})",
  cancelled: "Annulées ({0})",
};

function renderFilters() {
  const nav = $("filters");
  const counts = store.counts();
  if (!nav.childElementCount) {
    for (const f of FILTERS) {
      const button = document.createElement("button");
      button.type = "button";
      button.dataset.filter = f;
      button.addEventListener("click", () => {
        filter = f;
        renderFilters();
        renderList();
      });
      nav.append(button);
    }
  }
  for (const button of nav.children) {
    const f = button.dataset.filter;
    button.textContent = t(FILTER_LABELS[f], counts[f]);
    button.setAttribute("aria-pressed", String(f === filter));
  }
  const clear = $("clear-finished");
  clear.hidden = counts.completed + counts.cancelled === 0;
  clear.disabled = !connected;
}

function taskName(task) {
  return task.fileName || task.displayName || t("(sans nom)");
}

function detailText(task) {
  const parts = [stateText(task, t)];
  const total = task.totalBytes;
  if (task.state === "completed") {
    parts.push(bytes(total ?? task.doneBytes, lang, t));
  } else {
    parts.push(t("{0} sur {1}", bytes(task.doneBytes, lang, t), bytes(total ?? -1, lang, t)));
  }
  if (task.state === "active") {
    parts.push(t("{0}/s", bytes(task.bps ?? 0, lang, t)));
    const eta = duration(task.etaS, t);
    if (eta) parts.push(t("reste {0}", eta));
    if (task.connections > 0) parts.push(t("{0} connexion(s)", task.connections));
  }
  if (task.speedLimitBps && task.state !== "completed" && task.state !== "cancelled") {
    parts.push(t("limite {0}", t("{0}/s", bytes(task.speedLimitBps, lang, t))));
  }
  return parts.join(" · ");
}

function makeRow(taskId) {
  const row = template.content.firstElementChild.cloneNode(true);
  row.dataset.taskId = taskId;
  const task = () => store.tasks.get(taskId);
  row.querySelector(".toggle").addEventListener("click", () => {
    const state = task()?.state;
    const method = canResume(state) ? "task.resume" : "task.pause";
    act(method, { taskIds: [taskId] });
  });
  // Une tâche annulée ne reprend jamais : son fichier partiel ne servirait plus.
  row.querySelector(".cancel").addEventListener("click", () =>
    act("task.cancel", { taskIds: [taskId], deleteFiles: true }));
  row.querySelector(".remove").addEventListener("click", () => removeTasks([taskId]));
  row.querySelector(".rate").addEventListener("click", () => openTaskLimit(taskId));
  row.querySelector(".open").addEventListener("click", () => withFile(taskId, host.openFile));
  row.querySelector(".reveal").addEventListener("click", () => withFile(taskId, host.revealFile));
  return row;
}

/// Ouvre ou montre le fichier d'une tâche, sous le nom que le moteur a enregistré : relu
/// au moment du clic, pour ne jamais viser le fichier d'une autre tâche.
async function withFile(taskId, action) {
  try {
    const task = await host.call("task.get", { taskId });
    await action(task.destDir, task.fileName);
  } catch (e) {
    showError(refused(e));
  }
}

function renderRow(taskId) {
  const task = store.tasks.get(taskId);
  if (!task) return;
  let row = rows.get(taskId);
  if (!row) {
    row = makeRow(taskId);
    rows.set(taskId, row);
  }
  const name = taskName(task);
  const detail = detailText(task);
  row.dataset.state = task.state;
  row.querySelector(".name").textContent = name;
  row.querySelector(".name").title = name;
  row.querySelector(".detail").textContent = detail;
  // Nom de la ligne pour les lecteurs d'écran : nom du fichier puis état, sans répéter
  // la barre d'avancement.
  row.setAttribute("aria-label", t("{0}, {1}", name, detail));

  const bar = row.querySelector("progress");
  bar.setAttribute("aria-label", t("Avancement de {0}", name));
  if (task.totalBytes > 0) {
    bar.max = task.totalBytes;
    bar.value = Math.min(task.doneBytes ?? 0, task.totalBytes);
  } else if (task.state === "active") {
    bar.removeAttribute("value");
  } else {
    bar.max = 1;
    bar.value = task.state === "completed" ? 1 : 0;
  }

  const toggle = row.querySelector(".toggle");
  const resume = canResume(task.state);
  const retry = task.state === "failed";
  toggle.textContent = retry ? t("Relancer") : resume ? t("Reprendre") : t("Pause");
  toggle.setAttribute("aria-label",
    retry ? t("Relancer {0}", name) : resume ? t("Reprendre {0}", name) : t("Mettre en pause {0}", name));
  toggle.hidden = !(resume || canPause(task.state));
  toggle.disabled = !connected;

  const cancel = row.querySelector(".cancel");
  cancel.textContent = t("Annuler");
  cancel.setAttribute("aria-label", t("Annuler {0}", name));
  cancel.hidden = !canCancel(task.state);
  cancel.disabled = !connected;

  const done = task.state === "completed" && task.fileName;
  const open = row.querySelector(".open");
  open.textContent = t("Ouvrir");
  open.setAttribute("aria-label", t("Ouvrir {0}", name));
  // Programmes et scripts : jamais ouverts depuis l'application (SPEC §8.1).
  open.hidden = !done || isExecutable(task.fileName);
  const reveal = row.querySelector(".reveal");
  reveal.textContent = t("Afficher dans le dossier");
  reveal.setAttribute("aria-label", t("Afficher {0} dans son dossier", name));
  reveal.hidden = !done;

  // Limite propre à la tâche : tant qu'elle peut encore transférer.
  const rate = row.querySelector(".rate");
  rate.textContent = t("Débit…");
  rate.setAttribute("aria-label", t("Limiter le débit de {0}", name));
  rate.hidden = !(canPause(task.state) || canResume(task.state));
  rate.disabled = !connected;

  const remove = row.querySelector(".remove");
  remove.textContent = t("Retirer");
  remove.setAttribute("aria-label", t("Retirer {0} de la liste", name));
  remove.hidden = !canRemove(task.state);
  remove.disabled = !connected;
}

/// Retire des tâches finies : un fichier téléchargé reste, un fichier partiel (échec,
/// annulation) part avec sa tâche.
async function removeTasks(ids) {
  const done = ids.filter((id) => store.tasks.get(id)?.state === "completed");
  const others = ids.filter((id) => !done.includes(id));
  for (const [taskIds, deleteFiles] of [[done, false], [others, true]]) {
    if (taskIds.length) await act("task.remove", { taskIds, deleteFiles });
  }
}

function isExecutable(name) {
  const dot = name.lastIndexOf(".");
  return dot > 0 && executables.has(name.slice(dot + 1).toLowerCase());
}

/// Met la liste dans l'ordre du filtre ; ne déplace que les lignes mal placées, pour
/// garder le focus de celle qui l'a.
function renderList() {
  const wanted = store.list(filter).map((task) => {
    if (!rows.has(task.taskId)) renderRow(task.taskId);
    return rows.get(task.taskId);
  });
  const keep = new Set(wanted);
  // Une ligne qui s'en va avec le focus le confie à celle qui prend sa place.
  const children = [...list.children];
  const focused = children.findIndex((row) => row.contains(document.activeElement));
  const losing = focused >= 0 && !keep.has(children[focused]);
  for (const child of children) if (!keep.has(child)) child.remove();
  wanted.forEach((row, i) => {
    if (list.children[i] !== row) list.insertBefore(row, list.children[i] ?? null);
  });
  for (const id of rows.keys()) if (!store.tasks.has(id)) rows.delete(id);
  $("empty").hidden = wanted.length > 0;
  renderFilters();
  if (losing) {
    const next = wanted[Math.min(focused, wanted.length - 1)];
    const target = next?.querySelector(".buttons button:not([hidden]):not(:disabled)");
    (target ?? $("filters").querySelector('[aria-pressed="true"]'))?.focus();
  }
}

function renderSummary() {
  const { active, bps } = store.activity();
  $("summary").textContent = active
    ? t("{0} téléchargement(s) en cours, {1}", active, t("{0}/s", bytes(bps, lang, t)))
    : t("Aucun téléchargement en cours");
}

function renderAll() {
  for (const id of store.tasks.keys()) renderRow(id);
  renderList();
  renderSummary();
}

// Changements regroupés et dessinés une fois par image.
const pending = { rows: new Set(), list: false };
let frame = 0;

function schedule(result) {
  for (const id of result.changed) pending.rows.add(id);
  pending.list ||= result.list;
  if (frame) return;
  frame = requestAnimationFrame(() => {
    frame = 0;
    for (const id of pending.rows) renderRow(id);
    if (pending.list) renderList();
    else renderFilters();
    renderSummary();
    pending.rows.clear();
    pending.list = false;
  });
}

// ---------- Moteur ----------

let syncing = 0;

/// Recharge l'état complet (SPEC §6.10).
async function resync() {
  const mine = ++syncing;
  store.reset();
  let snapshot;
  try {
    snapshot = await host.call("sync.snapshot", {});
  } catch {
    return; // Connexion perdue : l'état de la connexion suivra.
  }
  if (mine !== syncing) return;
  const result = store.load(snapshot);
  renderAll();
  if (store.limits) renderLimit();
  else loadLimit();
  if (result.resync) resync();
}

function onStatus(status) {
  const banner = $("banner");
  connected = status.state === "up";
  document.body.dataset.engine = status.state;
  $("add").disabled = !connected;
  if (status.state === "up") {
    banner.hidden = true;
    engineVersion = status.engine?.engine?.version ?? "";
    resync();
  } else {
    banner.hidden = false;
    banner.textContent = status.state === "connecting"
      ? t("Connexion au moteur…")
      : status.message
        ? t("Moteur injoignable : {0}. Nouvel essai automatique.", status.message)
        : t("Moteur injoignable.");
    for (const id of rows.keys()) renderRow(id);
  }
  renderAbout();
}

function onEngine(event) {
  switch (event.kind) {
    case "status":
      onStatus(event);
      break;
    case "resync":
      resync();
      break;
    case "message": {
      const { method, params } = event.message;
      const result = store.apply(method, params ?? {});
      if (result.resync) {
        resync();
        return;
      }
      for (const { taskId, to } of result.finished) {
        const task = store.tasks.get(taskId);
        if (!task) continue;
        announce(to === "completed"
          ? t("{0} terminé", taskName(task))
          : t("{0} en échec : {1}", taskName(task), errorText(task.errorCode, t)));
      }
      schedule(result);
      if (result.limits) renderLimit();
      break;
    }
  }
}

async function act(method, params) {
  try {
    const result = await host.call(method, params);
    const errors = result?.errors ?? [];
    if (errors.length) showError(refused(errors[0]));
  } catch (e) {
    showError(refused(e));
  }
}

// ---------- Ajout ----------

async function addUrls(event) {
  event.preventDefault();
  const field = $("urls");
  const urls = field.value.split(/\s+/).filter(Boolean);
  if (!urls.length || !connected) return;
  const failed = [];
  let firstError = null;
  for (const url of urls) {
    try {
      await host.call("task.add", {
        sources: [{ url }],
        destDir: info.destDir,
        startPaused: $("paused").checked,
      });
    } catch (e) {
      failed.push(url);
      firstError ??= e;
    }
  }
  field.value = failed.join("\n");
  if (failed.length) {
    showError(t("{0} adresse(s) refusée(s) : {1}", failed.length, firstError?.message ?? ""));
  } else {
    showError("");
    filter = filter === "cancelled" || filter === "completed" ? "all" : filter;
  }
  field.focus();
}

// ---------- Limites de débit ----------

const LIMITS = [0, 128 << 10, 512 << 10, 1 << 20, 2 << 20, 5 << 20, 10 << 20, 25 << 20, 50 << 20, 100 << 20];

/// Remplit une liste de limites : « Aucune limite », les paliers, et la valeur courante
/// si elle n'en est pas un.
function fillLimits(select, current) {
  const values = new Set(LIMITS);
  if (current) values.add(current);
  select.replaceChildren(...[...values].sort((a, b) => a - b).map((v) => {
    const option = document.createElement("option");
    option.value = String(v);
    option.textContent = v ? t("{0}/s", bytes(v, lang, t)) : t("Aucune limite");
    return option;
  }));
  select.value = String(current ?? 0);
}

/// Limite globale : suivie par l'instantané et `limits.updated`.
function renderLimit() {
  fillLimits($("limit"), store.limits?.downloadBps ?? null);
  $("limit").disabled = !connected;
}

/// Moteur antérieur au 2026-11-07, sans limites dans l'instantané : les demander.
async function loadLimit() {
  try {
    store.limits = await host.call("limits.get", {});
  } catch {
    return;
  }
  renderLimit();
}

async function changeLimit() {
  const value = Number($("limit").value);
  try {
    store.limits = await host.call("limits.set", { downloadBps: value > 0 ? value : null });
  } catch (e) {
    showError(refused(e));
  }
  renderLimit();
}

/// Tâche dont le dialogue de limite est ouvert.
let limitTask = null;

function openTaskLimit(taskId) {
  const task = store.tasks.get(taskId);
  if (!task) return;
  limitTask = taskId;
  $("task-limit-title").textContent = t("Débit de {0}", taskName(task));
  fillLimits($("task-limit-value"), task.speedLimitBps ?? null);
  const dialog = $("task-limit");
  dialog.returnValue = "";
  dialog.showModal();
}

async function closeTaskLimit() {
  const id = limitTask;
  limitTask = null;
  if ($("task-limit").returnValue === "apply" && id) {
    const value = Number($("task-limit-value").value);
    try {
      await host.call("task.update", { taskId: id, speedLimitBps: value > 0 ? value : null });
    } catch (e) {
      showError(refused(e));
    }
  }
  // Le focus revient au bouton qui a ouvert le dialogue.
  rows.get(id)?.querySelector(".rate:not([hidden])")?.focus();
}

// ---------- Préférences ----------

function applyInfo(next) {
  info = next;
  lang = info.lang;
  t = translator(lang);
  executables = new Set(info.executableExtensions);
  translatePage();
  $("dest").textContent = info.destDir;
  $("pref-dest").textContent = info.destDir;
  $("pref-lang").value = info.prefs.lang;
  $("pref-tray").checked = info.prefs.closeToTray && info.tray;
  $("pref-tray").disabled = !info.tray;
  $("pref-no-tray").hidden = info.tray;
  $("pref-autostart").checked = info.autostart;
  renderAbout();
  renderLimit();
  renderAll();
}

function renderAbout() {
  if (info) $("about").textContent = t("AuroDownloader {0}, moteur {1}", info.version, engineVersion || "?");
}

function translatePage() {
  document.documentElement.lang = lang;
  document.title = t("AuroDownloader");
  for (const el of document.querySelectorAll("[data-i18n]")) el.textContent = t(el.dataset.i18n);
  for (const el of document.querySelectorAll("[data-i18n-placeholder]")) el.placeholder = t(el.dataset.i18nPlaceholder);
  for (const el of document.querySelectorAll("[data-i18n-label]")) el.setAttribute("aria-label", t(el.dataset.i18nLabel));
}

async function savePrefs(change) {
  try {
    applyInfo(await host.savePrefs({ ...info.prefs, ...change }));
  } catch (e) {
    showError(refused(e));
  }
}

async function chooseFolder(save) {
  const folder = await host.pickFolder(info.destDir).catch(() => null);
  if (folder) await savePrefs({ destDir: folder });
  if (save) $(save).focus();
}

// ---------- Démarrage ----------

async function start() {
  applyInfo(await host.info());
  renderFilters();

  $("add-form").addEventListener("submit", addUrls);
  $("urls").addEventListener("keydown", (e) => {
    // Entrée ajoute ; Maj+Entrée passe à la ligne.
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      $("add-form").requestSubmit();
    }
  });
  $("dest-change").addEventListener("click", () => chooseFolder("dest-change"));
  $("pref-dest-change").addEventListener("click", () => chooseFolder("pref-dest-change"));
  $("error-close").addEventListener("click", () => showError(""));
  $("clear-finished").addEventListener("click", () => {
    const finished = [...store.tasks.values()]
      .filter((task) => task.state === "completed" || task.state === "cancelled")
      .map((task) => task.taskId);
    removeTasks(finished);
  });
  $("limit").addEventListener("change", changeLimit);
  $("task-limit").addEventListener("close", closeTaskLimit);
  $("settings-open").addEventListener("click", () => $("settings").showModal());
  $("pref-lang").addEventListener("change", (e) => savePrefs({ lang: e.target.value }));
  $("pref-tray").addEventListener("change", (e) => savePrefs({ closeToTray: e.target.checked }));
  $("pref-autostart").addEventListener("change", async (e) => {
    try {
      e.target.checked = await host.setAutostart(e.target.checked);
    } catch (err) {
      e.target.checked = !e.target.checked;
      showError(refused(err));
    }
  });
  $("stop-engine").addEventListener("click", () => host.quit(true));
  document.addEventListener("keydown", (e) => {
    if (e.ctrlKey && !e.altKey && e.key.toLowerCase() === "l") {
      e.preventDefault();
      $("urls").focus();
    }
  });

  // S'abonner d'abord, puis demander l'état de la connexion : rien n'est manqué.
  await host.listen(onEngine);
  onStatus(await host.status());
  $("urls").focus();
}

start().catch((e) => showError(String(e?.message ?? e)));

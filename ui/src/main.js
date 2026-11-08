// Page de l'application : barre d'outils, arborescence des catégories, tableau des
// téléchargements, menu contextuel, propriétés d'un téléchargement, options. Toute action
// passe par le protocole du moteur (SPEC §6) ; la page ne garde que ce qu'elle affiche.
import { translator, stateText, errorText, bytes, duration } from "./i18n.js";
import {
  TaskStore, GROUPS, CATEGORIES, COLUMNS, LIVE_COLUMNS, category, taskFileName,
  canPause, canResume, canCancel, canRemove, canRestart, canRelocate, canSetChecksum,
  parseChecksum, ALGO_LABELS,
} from "./model.js";
import { tauriHost } from "./host.js";

const host = tauriHost();
const store = new TaskStore();
let t = translator("fr");
let lang = "fr";
let info = null;
let connected = false;
let engineVersion = "";
let executables = new Set();

const $ = (id) => document.getElementById(id);
const body = $("tasks");
const template = $("row");
/// Lignes affichées ou prêtes à l'être, par tâche.
const rows = new Map();

// ---------- Réglages d'affichage de ce poste ----------

// Vue, tri et branches repliées : de simples commodités, perdues sans dommage si le
// stockage du navigateur manque.
const LOCAL_KEY = "auro.display";

function loadLocal() {
  try {
    return JSON.parse(localStorage.getItem(LOCAL_KEY)) ?? {};
  } catch {
    return {};
  }
}

const local = loadLocal();
let view = typeof local.view === "string" ? local.view : "all";
const sort = {
  column: COLUMNS.includes(local.sort?.column) ? local.sort.column : "added",
  descending: local.sort?.descending ?? true,
};
const collapsed = new Set(Array.isArray(local.collapsed) ? local.collapsed : ["incomplete", "completed"]);

function saveLocal() {
  try {
    localStorage.setItem(LOCAL_KEY, JSON.stringify({ view, sort, collapsed: [...collapsed] }));
  } catch {
    // Stockage refusé : les réglages valent pour cette fois.
  }
}

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

// ---------- Textes ----------

const GROUP_LABELS = {
  all: "Tous les téléchargements",
  incomplete: "Incomplets",
  completed: "Terminés",
};

const CATEGORY_LABELS = {
  compressed: "Archives",
  documents: "Documents",
  images: "Images",
  music: "Musique",
  programs: "Programmes",
  video: "Vidéos",
  other: "Autres",
};

const COLUMN_LABELS = {
  name: "Nom du fichier",
  size: "Taille",
  progress: "Avancement",
  state: "Statut",
  eta: "Temps restant",
  rate: "Débit",
  added: "Ajouté le",
};

const QUEUE_LABELS = {
  default: "File principale",
};

const ICONS = { all: "i-all", incomplete: "i-incomplete", completed: "i-completed" };

const taskName = (task) => taskFileName(task) || t("(sans nom)");
const queueName = (queue) => (QUEUE_LABELS[queue.name] ? t(QUEUE_LABELS[queue.name]) : queue.name);

function viewLabel(key) {
  if (key.startsWith("queue:")) {
    const queue = store.queues.get(key.slice(6));
    return queue ? queueName(queue) : t(GROUP_LABELS.all);
  }
  const [group, cat] = key.split("/");
  return cat ? t("{0} — {1}", t(GROUP_LABELS[group]), t(CATEGORY_LABELS[cat])) : t(GROUP_LABELS[group]);
}

let dateFormat = new Intl.DateTimeFormat("fr", { dateStyle: "short", timeStyle: "short" });
/// Info-bulle : la date en toutes lettres.
let longDateFormat = new Intl.DateTimeFormat("fr", { dateStyle: "full", timeStyle: "short" });

/// Date et heure, toujours les deux : « 07/11/2026 13:10 ».
function dateText(ms, format = dateFormat) {
  return ms ? format.format(new Date(ms)) : "";
}

function percent(task) {
  if (task.state === "completed") return 100;
  if (!(task.totalBytes > 0)) return null;
  return Math.min(100, (100 * (task.doneBytes ?? 0)) / task.totalBytes);
}

function percentText(task) {
  const p = percent(task);
  if (p === null) return "";
  return t("{0} %", p.toLocaleString(lang, { minimumFractionDigits: 1, maximumFractionDigits: 1 }));
}

const rate = (bps) => t("{0}/s", bytes(bps, lang, t));

/// État affiché d'une tâche : une tâche en file dont la file est hors de ses plages
/// horaires le dit.
function taskStateText(task) {
  if (task.state === "queued" && store.waitsForWindow(task.queueId)) {
    return t("En attente de la plage horaire");
  }
  return stateText(task, t);
}

// ---------- Arborescence ----------

let treeSignature = "";

function viewButton(key, icon, label) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "view";
  button.dataset.view = key;
  button.innerHTML = '<svg class="icon" aria-hidden="true"><use/></svg><span class="label"></span><span class="count"></span>';
  button.querySelector("use").setAttribute("href", `#${icon}`);
  button.querySelector(".label").textContent = label;
  button.addEventListener("click", () => chooseView(key));
  return button;
}

function branch(id, button, children) {
  const item = document.createElement("li");
  item.className = "branch";
  const line = document.createElement("div");
  line.className = "tree-line";
  const twisty = document.createElement("button");
  twisty.type = "button";
  twisty.className = "twisty";
  twisty.dataset.branch = id;
  twisty.innerHTML = '<svg aria-hidden="true"><use href="#i-chevron"/></svg>';
  twisty.addEventListener("click", () => {
    if (collapsed.has(id)) collapsed.delete(id);
    else collapsed.add(id);
    saveLocal();
    renderTree();
  });
  line.append(twisty, button);
  const list = document.createElement("ul");
  list.append(...children.map((child) => {
    const li = document.createElement("li");
    li.append(child);
    return li;
  }));
  item.append(line, list);
  return item;
}

/// Construit l'arborescence quand ses branches changent (langue, files), puis met à jour
/// les nombres et la vue choisie.
function renderTree() {
  const queues = [...store.queues.values()].sort((a, b) => (a.position ?? 0) - (b.position ?? 0));
  const signature = JSON.stringify([lang, queues.map((q) => [q.queueId, q.name])]);
  const root = $("tree-items");
  if (signature !== treeSignature) {
    const focused = document.activeElement?.closest?.("#tree")
      ? document.activeElement.dataset.view ?? document.activeElement.dataset.branch
      : null;
    treeSignature = signature;
    const items = GROUPS.map((group) => branch(group,
      viewButton(group, ICONS[group], t(GROUP_LABELS[group])),
      CATEGORIES.map((cat) => viewButton(`${group}/${cat}`, `i-${cat}`, t(CATEGORY_LABELS[cat])))));
    if (queues.length) {
      const title = document.createElement("span");
      title.className = "view heading";
      title.textContent = t("Files d'attente");
      items.push(branch("queues", title,
        queues.map((q) => viewButton(`queue:${q.queueId}`, "i-queue", queueName(q)))));
    }
    root.replaceChildren(...items);
    if (focused) {
      const key = CSS.escape(focused);
      root.querySelector(`[data-view="${key}"], [data-branch="${key}"]`)?.focus();
    }
  }
  // Vue disparue (file supprimée) : retour à tout, une fois l'état du moteur chargé.
  if (store.rev !== null && !root.querySelector(`[data-view="${CSS.escape(view)}"]`)) view = "all";
  const counts = store.counts();
  for (const button of root.querySelectorAll("button.view")) {
    const n = counts[button.dataset.view] ?? 0;
    button.querySelector(".count").textContent = n ? t("({0})", n) : "";
    if (button.dataset.view === view) button.setAttribute("aria-current", "true");
    else button.removeAttribute("aria-current");
  }
  for (const twisty of root.querySelectorAll(".twisty")) {
    const id = twisty.dataset.branch;
    const open = !collapsed.has(id) || view.startsWith(`${id}/`) || (id === "queues" && view.startsWith("queue:"));
    twisty.setAttribute("aria-expanded", String(open));
    const label = id === "queues" ? t("Files d'attente") : t(GROUP_LABELS[id]);
    twisty.setAttribute("aria-label", open ? t("Replier {0}", label) : t("Déplier {0}", label));
    twisty.closest(".branch").querySelector("ul").hidden = !open;
  }
}

function chooseView(key) {
  view = key;
  saveLocal();
  renderTree();
  renderList();
  $("grid").setAttribute("aria-label", t("Téléchargements : {0}", viewLabel(view)));
}

// ---------- Tableau ----------

function renderHead() {
  const head = $("grid-head");
  if (!head.childElementCount) {
    for (const column of COLUMNS) {
      const th = document.createElement("th");
      th.setAttribute("role", "columnheader");
      th.className = column;
      const button = document.createElement("button");
      button.type = "button";
      button.addEventListener("click", () => {
        sort.descending = sort.column === column ? !sort.descending : column !== "name";
        sort.column = column;
        saveLocal();
        renderHead();
        renderList();
      });
      th.append(button);
      head.append(th);
    }
  }
  for (const th of head.children) {
    const column = th.className;
    const label = t(COLUMN_LABELS[column]);
    const button = th.firstElementChild;
    button.textContent = label;
    if (column === sort.column) {
      th.setAttribute("aria-sort", sort.descending ? "descending" : "ascending");
      button.dataset.arrow = sort.descending ? "▼" : "▲";
    } else {
      th.removeAttribute("aria-sort");
      delete button.dataset.arrow;
    }
    button.setAttribute("aria-label", t("Trier par {0}", label));
  }
}

function renderRow(taskId) {
  const task = store.tasks.get(taskId);
  if (!task) return;
  let row = rows.get(taskId);
  if (!row) {
    row = template.content.firstElementChild.cloneNode(true);
    row.dataset.taskId = taskId;
    rows.set(taskId, row);
  }
  const name = taskName(task);
  const cell = (c) => row.querySelector(`.${c}`);
  row.dataset.state = task.state;
  cell("name").querySelector("use").setAttribute("href", `#i-${category(taskFileName(task))}`);
  cell("name").querySelector(".text").textContent = name;
  cell("name").title = name;
  const total = task.state === "completed" ? task.totalBytes ?? task.doneBytes : task.totalBytes;
  cell("size").textContent = total != null ? bytes(total, lang, t) : "";

  const bar = cell("progress").querySelector("progress");
  bar.setAttribute("aria-hidden", "true");
  const p = percent(task);
  if (p !== null) {
    bar.max = 100;
    bar.value = p;
  } else if (task.state === "active") {
    bar.removeAttribute("value");
  } else {
    bar.max = 1;
    bar.value = 0;
  }
  cell("pct").textContent = percentText(task);
  cell("state").textContent = taskStateText(task);
  cell("state").title = cell("state").textContent;
  const active = task.state === "active";
  cell("eta").textContent = active ? duration(task.etaS, t) : "";
  cell("rate").textContent = active ? rate(task.bps ?? 0) : "";
  cell("added").textContent = dateText(task.createdAt);
  cell("added").title = dateText(task.createdAt, longDateFormat);
}

/// Met le tableau dans l'ordre de la vue ; ne déplace que les lignes mal placées, pour
/// garder le focus de celle qui l'a.
function renderList() {
  const wanted = store.list(view, sort.column, sort.descending).map((task) => {
    if (!rows.has(task.taskId)) renderRow(task.taskId);
    return rows.get(task.taskId);
  });
  const keep = new Set(wanted);
  const children = [...body.children];
  const focused = children.findIndex((row) => row.contains(document.activeElement));
  const losing = focused >= 0 && !keep.has(children[focused]);
  for (const child of children) if (!keep.has(child)) child.remove();
  wanted.forEach((row, i) => {
    if (body.children[i] !== row) body.insertBefore(row, body.children[i] ?? null);
  });
  for (const id of rows.keys()) if (!store.tasks.has(id)) rows.delete(id);
  // La sélection ne garde que les lignes affichées.
  for (const id of selected) if (!rows.get(id)?.isConnected) selected.delete(id);
  if (cursor && !rows.get(cursor)?.isConnected) {
    const next = wanted[Math.min(Math.max(focused, 0), wanted.length - 1)];
    cursor = next?.dataset.taskId ?? null;
    if (losing && cursor) {
      selected.add(cursor);
      anchor = cursor;
    }
  }
  $("empty").hidden = wanted.length > 0;
  renderSelection();
  renderTree();
  if (losing) (cursor ? rows.get(cursor) : $("tree").querySelector('[aria-current="true"]'))?.focus();
}

// ---------- Sélection ----------

/// Tâches choisies ; `anchor` : départ d'une sélection à la touche Maj ; `cursor` : ligne
/// active, qui reçoit le focus.
const selected = new Set();
let anchor = null;
let cursor = null;

const visibleIds = () => [...body.children].map((row) => row.dataset.taskId);
const selectedTasks = () => [...selected].map((id) => store.tasks.get(id)).filter(Boolean);

function renderSelection() {
  const ids = visibleIds();
  if (!cursor || !ids.includes(cursor)) cursor = ids[0] ?? null;
  for (const [id, row] of rows) {
    row.setAttribute("aria-selected", String(selected.has(id)));
    row.tabIndex = id === cursor ? 0 : -1;
  }
  $("selection-count").textContent = selected.size > 1 ? t("{0} sélectionnés", selected.size) : "";
  renderToolbar();
}

function moveTo(id, { extend = false, keep = false } = {}) {
  if (!id) return;
  if (extend && anchor) {
    const ids = visibleIds();
    const [from, to] = [ids.indexOf(anchor), ids.indexOf(id)].sort((a, b) => a - b);
    if (!keep) selected.clear();
    for (const other of ids.slice(from, to + 1)) selected.add(other);
  } else if (!keep) {
    selected.clear();
    selected.add(id);
    anchor = id;
  }
  cursor = id;
  renderSelection();
  rows.get(id)?.focus();
  rows.get(id)?.scrollIntoView({ block: "nearest" });
}

function toggle(id) {
  if (selected.has(id)) selected.delete(id);
  else selected.add(id);
  anchor = id;
  cursor = id;
  renderSelection();
  rows.get(id)?.focus();
}

function rowOf(event) {
  return event.target.closest?.("tr[data-task-id]");
}

function onGridClick(event) {
  const row = rowOf(event);
  if (!row) return;
  const id = row.dataset.taskId;
  if (event.shiftKey) moveTo(id, { extend: true, keep: event.ctrlKey || event.metaKey });
  else if (event.ctrlKey || event.metaKey) toggle(id);
  else moveTo(id);
}

function onGridKey(event) {
  const ids = visibleIds();
  if (!ids.length) return;
  const at = Math.max(0, ids.indexOf(cursor));
  const go = (index) => {
    // Ctrl seul déplace la ligne active sans toucher à la sélection.
    const id = ids[Math.max(0, Math.min(ids.length - 1, index))];
    moveTo(id, { extend: event.shiftKey, keep: event.ctrlKey || event.metaKey });
  };
  const page = Math.max(1, Math.floor(body.parentElement.parentElement.clientHeight / 28) - 1);
  switch (event.key) {
    case "ArrowDown": go(at + 1); break;
    case "ArrowUp": go(at - 1); break;
    case "PageDown": go(at + page); break;
    case "PageUp": go(at - page); break;
    case "Home": go(0); break;
    case "End": go(ids.length - 1); break;
    case " ":
      if (event.ctrlKey || event.metaKey) toggle(cursor);
      else moveTo(cursor, { extend: event.shiftKey });
      break;
    case "a":
    case "A":
      if (!(event.ctrlKey || event.metaKey)) return;
      for (const id of ids) selected.add(id);
      renderSelection();
      break;
    case "Enter":
      if (cursor) openDetails(cursor);
      break;
    case "Delete":
      openRemove();
      break;
    case "ContextMenu":
      openMenuAtRow();
      break;
    case "F10":
      if (!event.shiftKey) return;
      openMenuAtRow();
      break;
    default:
      return;
  }
  event.preventDefault();
}

// ---------- Barre d'outils ----------

function renderToolbar() {
  const chosen = selectedTasks();
  const all = [...store.tasks.values()];
  const off = !connected;
  $("tb-add").disabled = off;
  $("tb-resume").disabled = off || !chosen.some((task) => canResume(task.state));
  $("tb-pause").disabled = off || !chosen.some((task) => canPause(task.state));
  $("tb-resume-all").disabled = off || !all.some((task) => canResume(task.state));
  $("tb-pause-all").disabled = off || !all.some((task) => canPause(task.state));
  $("tb-remove").disabled = off || !chosen.length;
  $("tb-clear").disabled = off || !all.some((task) => task.state === "completed" || task.state === "cancelled");
  $("tb-properties").disabled = !cursor;
}

function resume(tasks) {
  const taskIds = tasks.filter((task) => canResume(task.state)).map((task) => task.taskId);
  if (taskIds.length) act("task.resume", { taskIds });
}

function pause(tasks) {
  const taskIds = tasks.filter((task) => canPause(task.state)).map((task) => task.taskId);
  if (taskIds.length) act("task.pause", { taskIds });
}

// ---------- Menu contextuel ----------

let menuReturn = null;

function openMenu(x, y) {
  const menu = $("menu");
  const chosen = selectedTasks();
  const one = chosen.length === 1 ? chosen[0] : null;
  const done = one?.state === "completed" && one.fileName;
  const enable = {
    open: done && !isExecutable(one.fileName),
    reveal: done,
    restart: chosen.some((task) => canRestart(task.state)),
    move: one && canRelocate(one),
    refresh: one && canRelocate(one),
    resume: chosen.some((task) => canResume(task.state)),
    pause: chosen.some((task) => canPause(task.state)),
    limit: one && (canPause(one.state) || canResume(one.state)),
    remove: chosen.length > 0,
    properties: !!one,
  };
  for (const item of menu.querySelectorAll("[data-command]")) {
    item.disabled = (!connected && item.dataset.command !== "properties") || !enable[item.dataset.command];
  }
  menuReturn = document.activeElement;
  menu.hidden = false;
  const { width, height } = menu.getBoundingClientRect();
  menu.style.left = `${Math.max(4, Math.min(x, innerWidth - width - 4))}px`;
  menu.style.top = `${Math.max(4, Math.min(y, innerHeight - height - 4))}px`;
  (menu.querySelector("[data-command]:not(:disabled)") ?? menu).focus();
}

function openMenuAtRow() {
  const row = cursor && rows.get(cursor);
  if (!row) return;
  if (!selected.has(cursor)) moveTo(cursor);
  const box = row.getBoundingClientRect();
  openMenu(box.left + 40, box.bottom);
}

function closeMenu(restore = true) {
  const menu = $("menu");
  if (menu.hidden) return;
  menu.hidden = true;
  if (restore) menuReturn?.focus();
  menuReturn = null;
}

function onMenuKey(event) {
  const items = [...$("menu").querySelectorAll("[data-command]:not(:disabled)")];
  const at = items.indexOf(document.activeElement);
  switch (event.key) {
    case "ArrowDown": items[(at + 1) % items.length]?.focus(); break;
    case "ArrowUp": items[(at - 1 + items.length) % items.length]?.focus(); break;
    case "Home": items[0]?.focus(); break;
    case "End": items[items.length - 1]?.focus(); break;
    case "Escape": closeMenu(); break;
    case "Tab": closeMenu(); break;
    default: return;
  }
  event.preventDefault();
}

function runCommand(command) {
  const chosen = selectedTasks();
  const one = chosen.length === 1 ? chosen[0] : null;
  closeMenu(!["properties", "limit", "remove", "restart", "move", "refresh"].includes(command));
  switch (command) {
    case "open": if (one) withFile(one.taskId, host.openFile); break;
    case "reveal": if (one) withFile(one.taskId, host.revealFile); break;
    case "resume": resume(chosen); break;
    case "pause": pause(chosen); break;
    case "limit": if (one) openDetails(one.taskId, { focusLimit: true }); break;
    case "remove": openRemove(); break;
    case "restart": openRestart(); break;
    case "move": openMove(); break;
    case "refresh": openRefresh(); break;
    case "properties": if (one) openDetails(one.taskId); break;
  }
}

// ---------- Fichiers ----------

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

function isExecutable(name) {
  const dot = name.lastIndexOf(".");
  return dot > 0 && executables.has(name.slice(dot + 1).toLowerCase());
}

// ---------- Suppression ----------

/// Tâches annulées pour être supprimées : retirées dès que le moteur annonce l'annulation.
const removeWhenCancelled = new Set();
let removing = [];

function openRemove() {
  const chosen = selectedTasks();
  if (!chosen.length || !connected) return;
  removing = chosen.map((task) => task.taskId);
  const completed = chosen.some((task) => task.state === "completed");
  $("remove-text").textContent = chosen.length === 1
    ? t("Supprimer « {0} » de la liste ?", taskName(chosen[0]))
    : t("Supprimer {0} téléchargements de la liste ?", chosen.length);
  $("remove-files").checked = false;
  $("remove-files-label").hidden = !completed;
  $("remove-note").hidden = chosen.every((task) => task.state === "completed");
  const dialog = $("remove-dialog");
  dialog.returnValue = "";
  dialog.showModal();
}

async function closeRemove() {
  const ids = removing;
  removing = [];
  if ($("remove-dialog").returnValue === "confirm") await deleteTasks(ids, $("remove-files").checked);
  (cursor ? rows.get(cursor) : null)?.focus();
}

/// Supprime des tâches de la liste. Un fichier terminé ne part que si on le demande ; un
/// fichier partiel part toujours, il ne servirait plus. Les tâches inachevées s'annulent
/// d'abord, puis se retirent quand le moteur annonce l'annulation.
async function deleteTasks(ids, deleteCompleted) {
  const tasks = ids.map((id) => store.tasks.get(id)).filter(Boolean);
  const done = tasks.filter((task) => task.state === "completed").map((task) => task.taskId);
  const finished = tasks.filter((task) => canRemove(task.state) && task.state !== "completed")
    .map((task) => task.taskId);
  const running = tasks.filter((task) => canCancel(task.state)).map((task) => task.taskId);
  if (done.length) await act("task.remove", { taskIds: done, deleteFiles: deleteCompleted });
  if (finished.length) await act("task.remove", { taskIds: finished, deleteFiles: true });
  if (running.length) {
    for (const id of running) removeWhenCancelled.add(id);
    const result = await act("task.cancel", { taskIds: running, deleteFiles: true });
    for (const error of result?.errors ?? []) removeWhenCancelled.delete(error.taskId);
    if (!result) for (const id of running) removeWhenCancelled.delete(id);
    removeCancelled();
  }
}

function removeCancelled() {
  const ready = [];
  for (const id of removeWhenCancelled) {
    const state = store.tasks.get(id)?.state;
    if (state === "cancelled" || state === undefined) {
      removeWhenCancelled.delete(id);
      if (state) ready.push(id);
    }
  }
  if (ready.length) act("task.remove", { taskIds: ready, deleteFiles: true });
}

/// « Retirer les finis » : un fichier téléchargé reste, un fichier partiel (annulation)
/// part avec sa tâche.
function clearFinished() {
  const all = [...store.tasks.values()];
  const done = all.filter((task) => task.state === "completed").map((task) => task.taskId);
  const cancelled = all.filter((task) => task.state === "cancelled").map((task) => task.taskId);
  if (done.length) act("task.remove", { taskIds: done, deleteFiles: false });
  if (cancelled.length) act("task.remove", { taskIds: cancelled, deleteFiles: true });
}

// ---------- Retéléchargement ----------

let restarting = [];

/// « Retélécharger… » : confirmation ; pour un fichier terminé, une case « Remplacer le
/// fichier déjà téléchargé », cochée par défaut (décision du 2026-11-07).
function openRestart() {
  const chosen = selectedTasks().filter((task) => canRestart(task.state));
  if (!chosen.length || !connected) return;
  restarting = chosen.map((task) => task.taskId);
  const completed = chosen.some((task) => task.state === "completed");
  $("restart-text").textContent = chosen.length === 1
    ? t("Retélécharger « {0} » depuis le début ?", taskName(chosen[0]))
    : t("Retélécharger {0} téléchargements depuis le début ?", chosen.length);
  $("restart-files").checked = true;
  $("restart-files-label").hidden = !completed;
  $("restart-keep-note").hidden = !completed;
  $("restart-part-note").hidden = chosen.every((task) => task.state === "completed");
  const dialog = $("restart-dialog");
  dialog.returnValue = "";
  dialog.showModal();
}

async function closeRestart() {
  const taskIds = restarting;
  restarting = [];
  if ($("restart-dialog").returnValue === "confirm") {
    await act("task.restart", { taskIds, deleteFiles: $("restart-files").checked });
  }
  (cursor ? rows.get(cursor) : null)?.focus();
}

// ---------- Déplacer/Renommer ----------

/// Tâche dont on change l'emplacement, et le dossier choisi : `{taskId, dir}`.
let moving = null;

/// « Déplacer/Renommer… » (SPEC §7.12) : le moteur déplace le fichier final d'une tâche
/// terminée, le fichier partiel des autres ; une tâche pas encore commencée prendra ce nom
/// et ce dossier.
function openMove() {
  const [task] = selectedTasks();
  if (!task || !canRelocate(task) || !connected) return;
  moving = { taskId: task.taskId, dir: task.destDir };
  const input = $("move-name");
  input.value = taskFileName(task);
  $("move-dir").textContent = task.destDir;
  $("move-part-note").hidden = !task.fileName || task.state === "completed";
  $("move-later-note").hidden = !!task.fileName;
  $("move-error").hidden = true;
  $("move-dir-change").hidden = typeof host.pickFolder !== "function";
  const dialog = $("move-dialog");
  dialog.returnValue = "";
  dialog.showModal();
  input.focus();
  // Le nom sans son extension, prêt à être remplacé, comme dans l'explorateur.
  const dot = input.value.lastIndexOf(".");
  input.setSelectionRange(0, dot > 0 ? dot : input.value.length);
}

async function chooseMoveDir() {
  if (!moving) return;
  const folder = await host.pickFolder(moving.dir).catch(() => null);
  if (folder && moving) {
    moving.dir = folder;
    $("move-dir").textContent = folder;
  }
  $("move-dir-change").focus();
}

async function saveMove() {
  if (!moving) return;
  const task = store.tasks.get(moving.taskId);
  const params = { taskId: moving.taskId };
  const name = $("move-name").value.trim();
  if (name && name !== (task ? taskFileName(task) : "")) params.fileName = name;
  if (moving.dir !== task?.destDir) params.destDir = moving.dir;
  if (!params.fileName && !params.destDir) {
    $("move-dialog").close();
    return;
  }
  $("move-save").disabled = true;
  try {
    const result = await host.call("task.update", params);
    // Vers un autre disque, la copie continue ; sa fin est annoncée par le moteur.
    if (result.moving) announce(t("Copie de « {0} » vers l'autre disque en cours", taskName(result)));
    $("move-dialog").close();
  } catch (e) {
    $("move-error").textContent = e?.message ?? String(e);
    $("move-error").hidden = false;
  } finally {
    $("move-save").disabled = false;
  }
}

// ---------- Actualiser l'adresse ----------

/// Tâche dont on remplace l'adresse.
let refreshing = null;

/// « Actualiser l'adresse… » (SPEC §7.13) : un lien expiré remplacé par un lien régénéré
/// sur le site d'origine ; le moteur vérifie qu'il donne le même fichier.
async function openRefresh() {
  const [task] = selectedTasks();
  if (!task || !canRelocate(task) || !connected) return;
  refreshing = task.taskId;
  $("refresh-text").textContent = t(
    "Nouvelle adresse de « {0} », par exemple un lien régénéré sur le site d'origine.",
    taskName(task),
  );
  $("refresh-old").textContent = "";
  $("refresh-url").value = "";
  $("refresh-note").hidden = !task.totalBytes || ["completed", "cancelled"].includes(task.state);
  $("refresh-error").hidden = true;
  const dialog = $("refresh-dialog");
  dialog.returnValue = "";
  dialog.showModal();
  $("refresh-url").focus();
  try {
    const full = await host.call("task.get", { taskId: task.taskId, include: ["sources"] });
    if (refreshing === task.taskId) $("refresh-old").textContent = full.sources?.[0]?.url ?? "";
  } catch {
    // L'adresse actuelle n'est qu'un rappel.
  }
}

async function saveRefresh() {
  const taskId = refreshing;
  if (!taskId) return;
  $("refresh-save").disabled = true;
  try {
    const task = await host.call("task.update", { taskId, url: $("refresh-url").value.trim() });
    $("refresh-dialog").close();
    // Un téléchargement en échec (lien expiré) repart aussitôt.
    if (task.state === "failed") await act("task.resume", { taskIds: [taskId] });
  } catch (e) {
    $("refresh-error").textContent = e?.message ?? String(e);
    $("refresh-error").hidden = false;
  } finally {
    $("refresh-save").disabled = false;
  }
}

// ---------- Propriétés d'un téléchargement ----------

let detailsTask = null;
/// Dernière réponse de `task.get` avec segments et sources, pour la tâche affichée.
let detailsFull = null;
let detailsTimer = 0;

/// Segments et sources : redemandés chaque seconde tant que les propriétés sont ouvertes
/// (SPEC §6.5, détail d'une tâche).
async function refreshDetails() {
  const id = detailsTask;
  if (!id || !connected) return;
  // Une tâche finie ne change plus : une seule lecture suffit.
  if (detailsFull?.taskId === id && ["completed", "failed", "cancelled"].includes(store.tasks.get(id)?.state)) return;
  let full;
  try {
    full = await host.call("task.get", { taskId: id, include: ["segments", "sources"] });
  } catch {
    return; // Tâche retirée ou moteur perdu : la notification suivra.
  }
  if (detailsTask !== id) return;
  detailsFull = full;
  renderDetails();
}

function openDetails(taskId, { focusLimit = false } = {}) {
  const task = store.tasks.get(taskId);
  if (!task) return;
  detailsTask = taskId;
  detailsFull = null;
  clearInterval(detailsTimer);
  detailsTimer = setInterval(refreshDetails, 1000);
  refreshDetails();
  fillLimits($("d-limit"), task.speedLimitBps ?? null);
  // Rempli à l'ouverture seulement : l'affichage se refait chaque seconde.
  $("d-checksum-input").value = task.checksum ? `${task.checksum.algo}:${task.checksum.value}` : "";
  renderDetails();
  const dialog = $("details");
  dialog.returnValue = "";
  dialog.showModal();
  if (focusLimit && !$("d-limit-field").hidden) $("d-limit").focus();
}

function renderDetails() {
  const task = store.tasks.get(detailsTask);
  if (!task) return;
  const name = taskName(task);
  const pct = percentText(task);
  $("details-title").textContent = pct ? t("{0} — {1}", pct, name) : name;
  const set = (key, text) => {
    $(`d-${key}`).textContent = text;
    $(`f-${key}`).hidden = !text;
  };
  const active = task.state === "active";
  const source = detailsFull?.sources?.[0];
  set("name", name);
  set("dir", task.destDir ?? "");
  set("url", source?.url ?? "");
  set("state", taskStateText(task));
  set("size", task.totalBytes != null ? bytes(task.totalBytes, lang, t) : t("inconnue"));
  set("done", pct ? t("{0} ({1})", bytes(task.doneBytes, lang, t), pct) : bytes(task.doneBytes, lang, t));
  set("rate", active ? rate(task.bps ?? 0) : "");
  set("eta", active ? duration(task.etaS, t) : "");
  set("connections", active && task.connections > 0 ? String(task.connections) : "");
  // F-04 : un serveur sans reprise le dit clairement.
  set("resume", source?.acceptsRanges === true
    ? t("Oui")
    : source?.acceptsRanges === false
      ? t("Non : ce serveur ne permet pas de reprendre ; un arrêt ou une coupure fait repartir de zéro")
      : "");
  $("d-resume").classList.toggle("warn", source?.acceptsRanges === false);
  // F-11 : empreinte attendue ; celle d'un fichier terminé a été vérifiée.
  const sum = task.checksum;
  set("checksum", !sum
    ? ""
    : task.state === "completed"
      ? t("{0} {1} (vérifiée)", ALGO_LABELS[sum.algo] ?? sum.algo, sum.value)
      : t("{0} {1}", ALGO_LABELS[sum.algo] ?? sum.algo, sum.value));
  $("d-checksum").classList.toggle("warn", task.errorCode === "CHECKSUM_MISMATCH");
  $("d-checksum-field").hidden = !canSetChecksum(task);
  $("d-checksum-apply").disabled = !connected;
  set("added", dateText(task.createdAt));
  set("finished", dateText(task.finishedAt));
  $("d-progress").setAttribute("aria-label", t("Avancement de {0}", name));
  const p = percent(task);
  if (p !== null) {
    $("d-progress").max = 100;
    $("d-progress").value = p;
  } else if (active) {
    $("d-progress").removeAttribute("value");
  } else {
    $("d-progress").value = 0;
  }

  const toggleButton = $("d-toggle");
  const retry = task.state === "failed";
  const resumable = canResume(task.state);
  toggleButton.textContent = retry ? t("Relancer") : resumable ? t("Reprendre") : t("Pause");
  toggleButton.hidden = !(resumable || canPause(task.state));
  toggleButton.disabled = !connected;
  $("d-cancel").hidden = !canCancel(task.state);
  $("d-cancel").disabled = !connected;
  const done = task.state === "completed" && task.fileName;
  // Programmes et scripts : jamais ouverts depuis l'application (SPEC §8.1).
  $("d-open").hidden = !done || isExecutable(task.fileName);
  $("d-reveal").hidden = !done;
  $("d-limit-field").hidden = !(canPause(task.state) || canResume(task.state));
  $("d-limit-apply").disabled = !connected;
  renderSegments(task);
}

/// Carte des segments (chacun à sa place dans le fichier, rempli de ce qu'il a reçu) et
/// tableau des segments, comme les connexions d'un gestionnaire classique.
function renderSegments(task) {
  const segments = detailsFull?.segments ?? [];
  const section = $("d-map-section");
  const size = (s) => s.end - s.start + 1;
  const total = task.totalBytes ?? segments.reduce((sum, s) => Math.max(sum, s.end + 1), 0);
  section.hidden = !segments.length || !(total > 0);
  if (section.hidden) return;
  const map = $("d-map");
  const pct = (n) => `${(100 * n) / total}%`;
  map.replaceChildren(...segments.map((s) => {
    const part = document.createElement("div");
    part.className = s.receiving ? "part receiving" : "part";
    part.style.left = pct(s.start);
    part.style.width = pct(size(s));
    const fill = document.createElement("div");
    fill.className = "fill";
    fill.style.width = `${Math.min(100, (100 * s.doneBytes) / size(s))}%`;
    part.append(fill);
    return part;
  }));
  const receiving = segments.filter((s) => s.receiving).length;
  map.setAttribute("aria-label", t("{0} segments, {1} en cours de réception", segments.length, receiving));
  $("d-segments").tBodies[0].replaceChildren(...segments.map((s, i) => {
    const row = document.createElement("tr");
    const finished = s.doneBytes >= size(s);
    const status = s.receiving ? t("Réception des données") : finished ? t("Terminé") : t("En attente");
    row.className = s.receiving ? "receiving" : finished ? "finished" : "";
    for (const text of [String(i + 1), t("{0} sur {1}", bytes(s.doneBytes, lang, t), bytes(size(s), lang, t)), status]) {
      const cell = document.createElement("td");
      cell.textContent = text;
      row.append(cell);
    }
    return row;
  }));
}

function closeDetails() {
  const id = detailsTask;
  detailsTask = null;
  detailsFull = null;
  clearInterval(detailsTimer);
  (id && rows.get(id)?.isConnected ? rows.get(id) : null)?.focus();
}

/// Message d'une empreinte mal écrite.
function checksumError() {
  return t("Empreinte illisible : 64 (SHA-256), 40 (SHA-1) ou 32 (MD5) chiffres hexadécimaux, ou « sha256:… »");
}

/// Empreinte attendue d'un téléchargement (F-11) : donnée, changée, ou retirée (champ vide).
async function applyChecksum() {
  const { checksum, error } = parseChecksum($("d-checksum-input").value);
  if (error) {
    showError(checksumError());
    $("d-checksum-input").focus();
    return;
  }
  try {
    await host.call("task.update", { taskId: detailsTask, checksum });
    showError("");
    announce(checksum ? t("Empreinte enregistrée") : t("Empreinte retirée"));
  } catch (e) {
    showError(refused(e));
  }
}

async function applyTaskLimit() {
  const value = Number($("d-limit").value);
  try {
    await host.call("task.update", { taskId: detailsTask, speedLimitBps: value > 0 ? value : null });
    announce(value > 0 ? t("Limite de {0} appliquée", rate(value)) : t("Limite retirée"));
  } catch (e) {
    showError(refused(e));
  }
}

// ---------- Planification (F-16) ----------

/// Lundi 2026-11-02 à midi, heure locale : pour nommer les jours dans la langue choisie.
const A_MONDAY = new Date(2026, 10, 2, 12);

function dayName(day, style) {
  const date = new Date(A_MONDAY);
  date.setDate(A_MONDAY.getDate() + day - 1);
  return new Intl.DateTimeFormat(lang, { weekday: style }).format(date);
}

/// Moment d'un changement de plage : l'heure seule aujourd'hui, avec le jour sinon.
function whenText(ms) {
  if (!ms) return "";
  const date = new Date(ms);
  const sameDay = date.toDateString() === new Date().toDateString();
  return new Intl.DateTimeFormat(lang, sameDay
    ? { timeStyle: "short" }
    : { weekday: "long", hour: "2-digit", minute: "2-digit" }).format(date);
}

/// État d'une planification, en une phrase ; vide sans planification active.
function planningText(schedule) {
  if (!schedule?.enabled) return "";
  if (schedule.open) {
    return schedule.nextChangeAt
      ? t("ouverte jusqu'à {0}", whenText(schedule.nextChangeAt))
      : t("ouverte");
  }
  return schedule.nextChangeAt
    ? t("fermée jusqu'à {0}", whenText(schedule.nextChangeAt))
    : t("fermée");
}

const END_ACTIONS = {
  sleep: "Mise en veille de l'ordinateur dans une minute : la file « {0} » n'a plus rien à télécharger.",
  shutdown: "Extinction de l'ordinateur dans une minute : la file « {0} » n'a plus rien à télécharger.",
};

let endActionTimer = 0;

/// Bandeau de l'action de fin annoncée, avec son compte à rebours.
function renderEndAction() {
  const pending = store.endAction;
  const box = $("end-action");
  clearInterval(endActionTimer);
  if (!pending) {
    box.hidden = true;
    return;
  }
  const queue = store.queues.get(pending.queueId);
  const text = t(END_ACTIONS[pending.action] ?? END_ACTIONS.shutdown, queue ? queueName(queue) : "?");
  // Lu une fois par les lecteurs d'écran ; le décompte, lui, ne l'est pas.
  if ($("end-action-text").textContent !== text) $("end-action-text").textContent = text;
  const tick = () => {
    const left = Math.max(0, Math.ceil((pending.at - Date.now()) / 1000));
    $("end-action-count").textContent = t("({0} s)", left);
  };
  tick();
  endActionTimer = setInterval(tick, 1000);
  $("end-action-cancel").disabled = !connected;
  box.hidden = false;
}

/// Barre d'état : les files planifiées et leur ouverture.
function renderPlanningStatus() {
  const parts = [];
  for (const queue of store.queues.values()) {
    const text = planningText(store.scheduleOf(queue.queueId));
    if (text) parts.push(t("{0} : {1}", queueName(queue), text));
  }
  $("planning-status").textContent = parts.join(" · ");
}

function renderPlanning() {
  renderEndAction();
  renderPlanningStatus();
  $("tb-schedule").disabled = !connected || store.queues.size === 0;
}

/// File dont la boîte de planification est ouverte.
let scheduleQueue = null;

function windowRow(window) {
  const item = document.createElement("li");
  item.className = "window";
  const days = document.createElement("fieldset");
  days.className = "days";
  const legend = document.createElement("legend");
  legend.className = "visually-hidden";
  legend.textContent = t("Jours");
  days.append(legend);
  for (let day = 1; day <= 7; day++) {
    const label = document.createElement("label");
    label.className = "day";
    const box = document.createElement("input");
    box.type = "checkbox";
    box.value = String(day);
    box.checked = window.days.includes(day);
    box.setAttribute("aria-label", dayName(day, "long"));
    const short = document.createElement("span");
    short.setAttribute("aria-hidden", "true");
    short.textContent = dayName(day, "short");
    label.append(box, short);
    days.append(label);
  }
  const time = (cls, label, value) => {
    const wrap = document.createElement("label");
    wrap.className = "time";
    const text = document.createElement("span");
    text.textContent = label;
    const input = document.createElement("input");
    input.type = "time";
    input.className = cls;
    input.required = true;
    input.value = value;
    wrap.append(text, input);
    return wrap;
  };
  const remove = document.createElement("button");
  remove.type = "button";
  remove.className = "remove-window";
  remove.textContent = t("Retirer");
  remove.setAttribute("aria-label", t("Retirer cette plage"));
  remove.addEventListener("click", () => {
    const list = $("sch-windows");
    const next = item.nextElementSibling ?? item.previousElementSibling;
    item.remove();
    (next?.querySelector("input") ?? $("sch-add-window")).focus();
    if (!list.childElementCount) list.append(windowRow(NEW_WINDOW));
  });
  item.append(days, time("start", t("de"), window.start), time("end", t("à"), window.end), remove);
  return item;
}

/// Plage proposée à une file qui n'en a pas : la nuit, tous les jours.
const NEW_WINDOW = { days: [1, 2, 3, 4, 5, 6, 7], start: "23:00", end: "07:00" };

function fillSchedule(queueId) {
  scheduleQueue = queueId;
  const schedule = store.scheduleOf(queueId);
  $("sch-enabled").checked = schedule ? schedule.enabled : true;
  const windows = schedule?.rules?.windows?.length ? schedule.rules.windows : [NEW_WINDOW];
  $("sch-windows").replaceChildren(...windows.map(windowRow));
  $("sch-end").value = schedule?.rules?.onDone ?? "none";
  const state = planningText(schedule);
  $("sch-state").textContent = state ? t("En ce moment : {0}.", state) : "";
  $("sch-error").hidden = true;
  syncScheduleForm();
}

/// Les plages ne servent que si la planification est active.
function syncScheduleForm() {
  const on = $("sch-enabled").checked;
  $("sch-windows-box").disabled = !on;
  $("sch-end").disabled = !on;
}

function openSchedule() {
  if (!connected || !store.queues.size) return;
  const select = $("sch-queue");
  const queues = [...store.queues.values()].sort((a, b) => (a.position ?? 0) - (b.position ?? 0));
  select.replaceChildren(...queues.map((q) => {
    const option = document.createElement("option");
    option.value = q.queueId;
    option.textContent = queueName(q);
    return option;
  }));
  // La file de la vue choisie, sinon la première.
  const fromView = view.startsWith("queue:") ? view.slice(6) : null;
  select.value = fromView && store.queues.has(fromView) ? fromView : queues[0].queueId;
  $("sch-queue-field").hidden = queues.length < 2;
  fillSchedule(select.value);
  $("schedule-dialog").showModal();
}

/// Règles saisies, ou un message d'erreur.
function readSchedule() {
  const windows = [];
  for (const item of $("sch-windows").children) {
    const days = [...item.querySelectorAll(".days input:checked")].map((box) => Number(box.value));
    const start = item.querySelector("input.start").value;
    const end = item.querySelector("input.end").value;
    if (!days.length) return { error: t("Chaque plage a besoin d'au moins un jour.") };
    if (!start || !end) return { error: t("Chaque plage a besoin d'une heure de début et de fin.") };
    windows.push({ days, start: start.slice(0, 5), end: end.slice(0, 5) });
  }
  return { rules: { windows, onDone: $("sch-end").value } };
}

async function saveSchedule() {
  const queue = store.queues.get(scheduleQueue);
  if (!queue) return;
  const enabled = $("sch-enabled").checked;
  const existing = store.scheduleOf(scheduleQueue);
  const fail = (text) => {
    $("sch-error").textContent = text;
    $("sch-error").hidden = false;
  };
  // Rien à enregistrer : pas de planification, et aucune demandée.
  if (!enabled && !existing) {
    $("schedule-dialog").close();
    return;
  }
  // Désactivée, les plages ne se modifient pas (champs grisés) : celles enregistrées
  // restent si la saisie est incomplète, sinon on ne pourrait plus la désactiver.
  const read = readSchedule();
  const rules = read.error && !enabled ? existing.rules : read.rules;
  if (!rules) return fail(read.error);
  try {
    const params = { name: queueName(queue), enabled, rules };
    if (existing) params.scheduleId = existing.scheduleId;
    const saved = await host.call("schedule.set", params);
    if (queue.scheduleId !== saved.scheduleId) {
      await host.call("queue.update", { queueId: queue.queueId, scheduleId: saved.scheduleId });
    }
  } catch (e) {
    return fail(refused(e));
  }
  $("schedule-dialog").close();
  announce(enabled ? t("Planification enregistrée") : t("Planification désactivée"));
}

async function cancelEndAction() {
  try {
    await host.call("engine.cancelEndAction", {});
    announce(t("Action de fin annulée"));
  } catch (e) {
    showError(refused(e));
  }
}

// ---------- Rendu groupé ----------

function renderSummary() {
  const { active, bps } = store.activity();
  $("summary").textContent = active
    ? t("{0} téléchargement(s) en cours, {1}", active, rate(bps))
    : t("Aucun téléchargement en cours");
}

function renderAll() {
  for (const id of store.tasks.keys()) renderRow(id);
  renderHead();
  renderList();
  renderSummary();
  if (detailsTask) renderDetails();
}

// Changements regroupés et dessinés une fois par image.
const pending = { rows: new Set(), list: false };
let frame = 0;

function schedule(result) {
  for (const id of result.changed) pending.rows.add(id);
  // Un tri sur une valeur qui bouge suit l'avancement.
  pending.list ||= result.list || (result.changed.size > 0 && LIVE_COLUMNS.has(sort.column));
  if (frame) return;
  frame = requestAnimationFrame(() => {
    frame = 0;
    for (const id of pending.rows) renderRow(id);
    if (pending.list) renderList();
    else renderToolbar();
    renderSummary();
    if (detailsTask) {
      if (!store.tasks.has(detailsTask)) $("details").close();
      else if (pending.rows.has(detailsTask)) renderDetails();
    }
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
  renderLimit();
  renderDest();
  renderPlanning();
  removeCancelled();
  if (result.resync) resync();
}

function onStatus(status) {
  const banner = $("banner");
  connected = status.state === "up";
  document.body.dataset.engine = status.state;
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
  }
  renderToolbar();
  renderLimit();
  renderDest();
  renderPlanning();
  if (detailsTask) renderDetails();
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
      if (removeWhenCancelled.size) removeCancelled();
      schedule(result);
      if (result.limits) renderLimit();
      if (result.settings) renderDest();
      if (result.planning) renderPlanning();
      for (const { taskId, error } of result.moveErrors) {
        const task = store.tasks.get(taskId);
        showError(t("Déplacement de « {0} » impossible : {1}", task ? taskName(task) : taskId, error));
      }
      if (result.endActionError) {
        showError(t("Action de fin refusée par le système : {0}", result.endActionError));
      }
      break;
    }
  }
}

/// Appelle le moteur ; montre un refus. Rend le résultat, ou `null` en cas d'erreur.
async function act(method, params) {
  try {
    const result = await host.call(method, params);
    const errors = result?.errors ?? [];
    if (errors.length) showError(refused(errors[0]));
    return result ?? {};
  } catch (e) {
    showError(refused(e));
    return null;
  }
}

// ---------- Ajout ----------

/// Dossier des téléchargements du moteur (`downloads.dir`), dans l'ajout et les options.
function renderDest() {
  const dir = store.settings["downloads.dir"] ?? "";
  for (const id of ["dest", "pref-dest"]) $(id).textContent = dir || t("(aucun)");
  for (const id of ["dest-change", "pref-dest-change"]) $(id).disabled = !connected;
  syncLoginButtons();
}

function openAdd() {
  if (!connected || document.querySelector("dialog[open]")) return;
  renderDest();
  const dialog = $("add-dialog");
  dialog.returnValue = "";
  dialog.showModal();
  $("urls").focus();
}

async function closeAdd() {
  const choice = $("add-dialog").returnValue;
  if (choice !== "start" && choice !== "later") {
    $("tb-add").focus();
    return;
  }
  const field = $("urls");
  const urls = field.value.split(/\s+/).filter(Boolean);
  if (!urls.length || !connected) return;
  // F-11 : l'empreinte est celle d'un seul fichier.
  const { checksum, error } = parseChecksum($("add-checksum").value);
  if (error || (checksum && urls.length > 1)) {
    showError(error ? checksumError() : t("Une empreinte ne vaut que pour une seule adresse."));
    openAdd();
    $("add-checksum").focus();
    return;
  }
  const failed = [];
  let firstError = null;
  for (const url of urls) {
    try {
      // Sans destDir : le dossier des téléchargements du moteur.
      const params = { sources: [{ url }], startPaused: choice === "later" };
      if (checksum) params.checksum = checksum;
      await host.call("task.add", params);
    } catch (e) {
      failed.push(url);
      firstError ??= e;
    }
  }
  field.value = failed.join("\n");
  if (!failed.length) $("add-checksum").value = "";
  if (failed.length) {
    showError(t("{0} adresse(s) refusée(s) : {1}", failed.length, firstError?.message ?? ""));
    openAdd();
    return;
  }
  showError("");
  // Les nouvelles tâches doivent se voir.
  if (view.startsWith("completed")) chooseView("all");
  $("tb-add").focus();
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
    option.textContent = v ? rate(v) : t("Aucune limite");
    return option;
  }));
  select.value = String(current ?? 0);
}

/// Limite globale : suivie par l'instantané et `limits.updated`, dans la barre d'état et
/// dans les options.
function renderLimit() {
  for (const id of ["limit", "pref-limit"]) {
    fillLimits($(id), store.limits?.downloadBps ?? null);
    $(id).disabled = !connected;
  }
}

async function changeLimit(event) {
  const value = Number(event.target.value);
  try {
    store.limits = await host.call("limits.set", { downloadBps: value > 0 ? value : null });
  } catch (e) {
    showError(refused(e));
  }
  renderLimit();
}

// ---------- Identifiants de sites ----------

/// Identifiants enregistrés (`login.list`, SPEC §7.14) : site et nom, jamais le mot de passe.
let logins = [];
/// Identifiants en cours de modification ; `null` : nouveaux.
let editingLogin = null;

async function loadLogins() {
  $("logins-error").hidden = true;
  try {
    logins = (await host.call("login.list")).items ?? [];
  } catch (e) {
    logins = [];
    $("logins-error").textContent = refused(e);
    $("logins-error").hidden = false;
  }
  renderLogins();
}

function renderLogins(select) {
  const list = $("logins");
  const keep = select ?? list.value;
  list.replaceChildren(...logins.map((login) => {
    const option = document.createElement("option");
    option.value = login.loginId;
    option.textContent = `${login.site} — ${login.username}`;
    return option;
  }));
  if (logins.some((login) => login.loginId === keep)) list.value = keep;
  list.hidden = !logins.length;
  $("logins-empty").hidden = logins.length > 0;
  syncLoginButtons();
}

function syncLoginButtons() {
  const chosen = !!$("logins").value;
  $("login-new").disabled = !connected;
  $("login-edit").disabled = !connected || !chosen;
  $("login-delete").disabled = !connected || !chosen;
}

function openLogin(login) {
  editingLogin = login?.loginId ?? null;
  $("login-site").value = login?.site ?? "";
  $("login-user").value = login?.username ?? "";
  $("login-password").value = "";
  // Nouveaux identifiants : un mot de passe ; sinon, vide garde celui enregistré.
  $("login-password").required = !login;
  $("login-keep-note").hidden = !login;
  $("login-error").hidden = true;
  const dialog = $("login-dialog");
  dialog.returnValue = "";
  dialog.showModal();
  (login ? $("login-password") : $("login-site")).focus();
}

async function saveLogin() {
  const params = { site: $("login-site").value, username: $("login-user").value };
  if (editingLogin) params.loginId = editingLogin;
  const password = $("login-password").value;
  if (password) params.password = password;
  $("login-save").disabled = true;
  try {
    const saved = await host.call("login.set", params);
    $("login-dialog").close();
    await loadLogins();
    renderLogins(saved.loginId);
    $("logins").focus();
  } catch (e) {
    $("login-error").textContent = e?.message ?? String(e);
    $("login-error").hidden = false;
  } finally {
    $("login-save").disabled = false;
  }
}

async function deleteLogin() {
  const login = logins.find((l) => l.loginId === $("logins").value);
  if (!login) return;
  try {
    await host.call("login.delete", { loginId: login.loginId });
    announce(t("Identifiants de {0} supprimés", login.site));
  } catch (e) {
    $("logins-error").textContent = refused(e);
    $("logins-error").hidden = false;
  }
  await loadLogins();
  (logins.length ? $("logins") : $("login-new")).focus();
}

// ---------- Options ----------

function selectTab(tab, focus = false) {
  for (const other of document.querySelectorAll('#settings [role="tab"]')) {
    const on = other === tab;
    other.setAttribute("aria-selected", String(on));
    other.tabIndex = on ? 0 : -1;
    $(other.getAttribute("aria-controls")).hidden = !on;
  }
  if (focus) tab.focus();
}

function onTabKey(event) {
  const tabs = [...document.querySelectorAll('#settings [role="tab"]')];
  const at = tabs.indexOf(event.target);
  const go = { ArrowRight: at + 1, ArrowLeft: at - 1, Home: 0, End: tabs.length - 1 }[event.key];
  if (go === undefined) return;
  event.preventDefault();
  selectTab(tabs[(go + tabs.length) % tabs.length], true);
}

function applyInfo(next) {
  info = next;
  lang = info.lang;
  t = translator(lang);
  dateFormat = new Intl.DateTimeFormat(lang, { dateStyle: "short", timeStyle: "short" });
  longDateFormat = new Intl.DateTimeFormat(lang, { dateStyle: "full", timeStyle: "short" });
  executables = new Set(info.executableExtensions);
  translatePage();
  renderDest();
  $("pref-lang").value = info.prefs.lang;
  $("pref-tray").checked = info.prefs.closeToTray && info.tray;
  $("pref-tray").disabled = !info.tray;
  $("pref-no-tray").hidden = info.tray;
  $("pref-autostart").checked = info.autostart;
  $("grid").setAttribute("aria-label", t("Téléchargements : {0}", viewLabel(view)));
  renderAbout();
  renderLimit();
  renderPlanning();
  renderAll();
}

function renderAbout() {
  if (info) $("about").textContent = t("AuroDownloader {0}, moteur {1}", info.version, engineVersion || "?");
}

function translatePage() {
  document.documentElement.lang = lang;
  document.title = t("AuroDownloader");
  for (const el of document.querySelectorAll("[data-i18n]")) el.textContent = t(el.dataset.i18n);
  for (const el of document.querySelectorAll("[data-i18n-label]")) el.setAttribute("aria-label", t(el.dataset.i18nLabel));
  for (const el of document.querySelectorAll("[data-i18n-title]")) el.title = t(el.dataset.i18nTitle);
}

async function savePrefs(change) {
  try {
    applyInfo(await host.savePrefs({ ...info.prefs, ...change }));
  } catch (e) {
    showError(refused(e));
  }
}

/// Change le dossier des téléchargements du moteur ; le moteur l'annonce
/// (`settings.updated`), la réponse le donne aussitôt.
async function chooseFolder(save) {
  const current = store.settings["downloads.dir"] ?? "";
  const folder = await host.pickFolder(current).catch(() => null);
  if (folder) {
    try {
      const result = await host.call("settings.set", { values: { "downloads.dir": folder } });
      Object.assign(store.settings, result.values);
      renderDest();
    } catch (e) {
      showError(refused(e));
    }
  }
  if (save) $(save).focus();
}

// ---------- Démarrage ----------

function wire() {
  $("tb-add").addEventListener("click", openAdd);
  $("tb-resume").addEventListener("click", () => resume(selectedTasks()));
  $("tb-pause").addEventListener("click", () => pause(selectedTasks()));
  $("tb-resume-all").addEventListener("click", () => resume([...store.tasks.values()]));
  $("tb-pause-all").addEventListener("click", () => pause([...store.tasks.values()]));
  $("tb-remove").addEventListener("click", openRemove);
  $("tb-clear").addEventListener("click", clearFinished);
  $("tb-properties").addEventListener("click", () => cursor && openDetails(cursor));
  $("tb-schedule").addEventListener("click", openSchedule);
  $("sch-queue").addEventListener("change", (e) => fillSchedule(e.target.value));
  $("sch-enabled").addEventListener("change", syncScheduleForm);
  $("sch-add-window").addEventListener("click", () => {
    const item = windowRow(NEW_WINDOW);
    $("sch-windows").append(item);
    item.querySelector("input").focus();
  });
  $("schedule-form").addEventListener("submit", (event) => {
    if (event.submitter?.value !== "save") return;
    // Enregistrée d'abord ; la boîte ne se ferme qu'en cas de succès.
    event.preventDefault();
    saveSchedule();
  });
  $("schedule-dialog").addEventListener("close", () => {
    scheduleQueue = null;
    $("tb-schedule").focus();
  });
  $("end-action-cancel").addEventListener("click", cancelEndAction);
  $("tb-settings").addEventListener("click", () => {
    selectTab($("tab-general"));
    $("settings").showModal();
    if (connected) loadLogins();
  });
  $("logins").addEventListener("change", syncLoginButtons);
  $("logins").addEventListener("dblclick", () => {
    const login = logins.find((l) => l.loginId === $("logins").value);
    if (login) openLogin(login);
  });
  $("login-new").addEventListener("click", () => openLogin(null));
  $("login-edit").addEventListener("click", () => {
    const login = logins.find((l) => l.loginId === $("logins").value);
    if (login) openLogin(login);
  });
  $("login-delete").addEventListener("click", deleteLogin);
  $("login-form").addEventListener("submit", (event) => {
    if (event.submitter?.value !== "save") return;
    // Enregistrés d'abord ; la boîte ne se ferme qu'en cas de succès.
    event.preventDefault();
    saveLogin();
  });
  $("login-dialog").addEventListener("close", () => {
    // Le mot de passe ne reste pas dans la page.
    $("login-password").value = "";
    editingLogin = null;
  });

  body.addEventListener("click", onGridClick);
  body.addEventListener("dblclick", (event) => {
    const row = rowOf(event);
    if (row) openDetails(row.dataset.taskId);
  });
  body.addEventListener("keydown", onGridKey);
  body.addEventListener("contextmenu", (event) => {
    const row = rowOf(event);
    if (!row) return;
    event.preventDefault();
    const id = row.dataset.taskId;
    if (!selected.has(id)) moveTo(id);
    else {
      cursor = id;
      renderSelection();
      row.focus();
    }
    openMenu(event.clientX, event.clientY);
  });
  // Un clic dans le vide du tableau désélectionne.
  document.querySelector("main").addEventListener("click", (event) => {
    if (event.target.closest("tr, thead")) return;
    selected.clear();
    renderSelection();
  });
  // Le menu du navigateur n'a rien à faire ailleurs dans la page.
  document.addEventListener("contextmenu", (event) => {
    if (!event.target.closest("textarea, input")) event.preventDefault();
  });

  const menu = $("menu");
  menu.addEventListener("keydown", onMenuKey);
  menu.addEventListener("click", (event) => {
    const item = event.target.closest("[data-command]");
    if (item && !item.disabled) runCommand(item.dataset.command);
  });
  document.addEventListener("pointerdown", (event) => {
    if (!menu.hidden && !menu.contains(event.target)) closeMenu(false);
  });
  addEventListener("blur", () => closeMenu(false));
  addEventListener("resize", () => closeMenu(false));

  $("add-dialog").addEventListener("close", closeAdd);
  $("urls").addEventListener("keydown", (event) => {
    // Entrée démarre ; Maj+Entrée passe à la ligne.
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      $("add-dialog").close("start");
    }
  });
  // Sans cela, Entrée dans un champ actionnerait le premier bouton du formulaire.
  $("add-checksum").addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      $("add-dialog").close("start");
    }
  });
  $("d-checksum-input").addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      applyChecksum();
    }
  });
  $("dest-change").addEventListener("click", () => chooseFolder("dest-change"));
  $("remove-dialog").addEventListener("close", closeRemove);
  $("restart-dialog").addEventListener("close", closeRestart);
  $("move-form").addEventListener("submit", (event) => {
    if (event.submitter?.value !== "save") return;
    // Enregistré d'abord ; la boîte ne se ferme qu'en cas de succès.
    event.preventDefault();
    saveMove();
  });
  $("move-dir-change").addEventListener("click", chooseMoveDir);
  $("move-dialog").addEventListener("close", () => {
    moving = null;
    (cursor ? rows.get(cursor) : null)?.focus();
  });
  $("refresh-form").addEventListener("submit", (event) => {
    if (event.submitter?.value !== "save") return;
    event.preventDefault();
    saveRefresh();
  });
  $("refresh-dialog").addEventListener("close", () => {
    refreshing = null;
    (cursor ? rows.get(cursor) : null)?.focus();
  });

  $("details").addEventListener("close", closeDetails);
  $("d-toggle").addEventListener("click", () => {
    const task = store.tasks.get(detailsTask);
    if (!task) return;
    if (canResume(task.state)) resume([task]);
    else pause([task]);
  });
  $("d-cancel").addEventListener("click", () => {
    // Une tâche annulée ne reprend jamais : son fichier partiel ne servirait plus.
    if (detailsTask) act("task.cancel", { taskIds: [detailsTask], deleteFiles: true });
  });
  $("d-open").addEventListener("click", () => detailsTask && withFile(detailsTask, host.openFile));
  $("d-reveal").addEventListener("click", () => detailsTask && withFile(detailsTask, host.revealFile));
  $("d-limit-apply").addEventListener("click", applyTaskLimit);
  $("d-checksum-apply").addEventListener("click", applyChecksum);

  for (const tab of document.querySelectorAll('#settings [role="tab"]')) {
    tab.addEventListener("click", () => selectTab(tab));
    tab.addEventListener("keydown", onTabKey);
  }
  $("pref-dest-change").addEventListener("click", () => chooseFolder("pref-dest-change"));
  $("error-close").addEventListener("click", () => showError(""));
  $("limit").addEventListener("change", changeLimit);
  $("pref-limit").addEventListener("change", changeLimit);
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

  document.addEventListener("keydown", (event) => {
    const key = event.key.toLowerCase();
    if ((event.ctrlKey || event.metaKey) && !event.altKey && (key === "n" || key === "l")) {
      event.preventDefault();
      openAdd();
    }
  });
}

async function start() {
  wire();
  applyInfo(await host.info());
  // S'abonner d'abord, puis demander l'état de la connexion : rien n'est manqué.
  await host.listen(onEngine);
  onStatus(await host.status());
  (cursor ? rows.get(cursor) : $("tb-add"))?.focus();
}

start().catch((e) => showError(String(e?.message ?? e)));

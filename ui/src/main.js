// Page de l'application : barre d'outils, arborescence des catégories, tableau des
// téléchargements, menu contextuel, propriétés d'un téléchargement, options. Toute action
// passe par le protocole du moteur (SPEC §6) ; la page ne garde que ce qu'elle affiche.
import { translator, stateText, errorText, bytes, duration } from "./i18n.js";
import {
  TaskStore, GROUPS, CATEGORIES, COLUMNS, LIVE_COLUMNS, category, taskFileName,
  canPause, canResume, canCancel, canRemove,
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
let timeFormat = new Intl.DateTimeFormat("fr", { timeStyle: "short" });

/// Date courte : l'heure seule pour aujourd'hui.
function shortDate(ms) {
  if (!ms) return "";
  const date = new Date(ms);
  return date.toDateString() === new Date().toDateString() ? timeFormat.format(date) : dateFormat.format(date);
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
    if (focused) root.querySelector(`[data-view="${focused}"], [data-branch="${focused}"]`)?.focus();
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
  cell("state").textContent = stateText(task, t);
  cell("state").title = cell("state").textContent;
  const active = task.state === "active";
  cell("eta").textContent = active ? duration(task.etaS, t) : "";
  cell("rate").textContent = active ? rate(task.bps ?? 0) : "";
  cell("added").textContent = shortDate(task.createdAt);
  cell("added").title = task.createdAt ? dateFormat.format(new Date(task.createdAt)) : "";
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
  closeMenu(command !== "properties" && command !== "limit" && command !== "remove");
  switch (command) {
    case "open": if (one) withFile(one.taskId, host.openFile); break;
    case "reveal": if (one) withFile(one.taskId, host.revealFile); break;
    case "resume": resume(chosen); break;
    case "pause": pause(chosen); break;
    case "limit": if (one) openDetails(one.taskId, { focusLimit: true }); break;
    case "remove": openRemove(); break;
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

// ---------- Propriétés d'un téléchargement ----------

let detailsTask = null;

function openDetails(taskId, { focusLimit = false } = {}) {
  const task = store.tasks.get(taskId);
  if (!task) return;
  detailsTask = taskId;
  fillLimits($("d-limit"), task.speedLimitBps ?? null);
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
  set("name", name);
  set("dir", task.destDir ?? "");
  set("state", stateText(task, t));
  set("size", task.totalBytes != null ? bytes(task.totalBytes, lang, t) : t("inconnue"));
  set("done", pct ? t("{0} ({1})", bytes(task.doneBytes, lang, t), pct) : bytes(task.doneBytes, lang, t));
  set("rate", active ? rate(task.bps ?? 0) : "");
  set("eta", active ? duration(task.etaS, t) : "");
  set("connections", active && task.connections > 0 ? String(task.connections) : "");
  set("added", task.createdAt ? dateFormat.format(new Date(task.createdAt)) : "");
  set("finished", task.finishedAt ? dateFormat.format(new Date(task.finishedAt)) : "");
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
}

function closeDetails() {
  const id = detailsTask;
  detailsTask = null;
  (id && rows.get(id)?.isConnected ? rows.get(id) : null)?.focus();
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
  if (store.limits) renderLimit();
  else loadLimit();
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

function openAdd() {
  if (!connected || document.querySelector("dialog[open]")) return;
  $("dest").textContent = info.destDir;
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
  const failed = [];
  let firstError = null;
  for (const url of urls) {
    try {
      await host.call("task.add", {
        sources: [{ url }],
        destDir: info.destDir,
        startPaused: choice === "later",
      });
    } catch (e) {
      failed.push(url);
      firstError ??= e;
    }
  }
  field.value = failed.join("\n");
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

/// Moteur antérieur au 2026-11-07, sans limites dans l'instantané : les demander.
async function loadLimit() {
  try {
    store.limits = await host.call("limits.get", {});
  } catch {
    return;
  }
  renderLimit();
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
  timeFormat = new Intl.DateTimeFormat(lang, { timeStyle: "short" });
  executables = new Set(info.executableExtensions);
  translatePage();
  $("dest").textContent = info.destDir;
  $("pref-dest").textContent = info.destDir;
  $("pref-lang").value = info.prefs.lang;
  $("pref-tray").checked = info.prefs.closeToTray && info.tray;
  $("pref-tray").disabled = !info.tray;
  $("pref-no-tray").hidden = info.tray;
  $("pref-autostart").checked = info.autostart;
  $("grid").setAttribute("aria-label", t("Téléchargements : {0}", viewLabel(view)));
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

async function chooseFolder(save) {
  const folder = await host.pickFolder(info.destDir).catch(() => null);
  if (folder) await savePrefs({ destDir: folder });
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
  $("tb-settings").addEventListener("click", () => {
    selectTab($("tab-general"));
    $("settings").showModal();
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
  $("dest-change").addEventListener("click", () => chooseFolder("dest-change"));
  $("remove-dialog").addEventListener("close", closeRemove);

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

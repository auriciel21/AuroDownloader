// Page de l'application : barre d'outils, arborescence des catégories, tableau des
// téléchargements, menu contextuel, propriétés d'un téléchargement, options. Toute action
// passe par le protocole du moteur (SPEC §6) ; la page ne garde que ce qu'elle affiche.
import { translator, stateText, errorText, bytes, duration } from "./i18n.js";
import {
  TaskStore, GROUPS, CATEGORIES, COLUMNS, LIVE_COLUMNS, category, taskFileName,
  canPause, canResume, canCancel, canRemove, canRestart, canRelocate, canSetChecksum, detailsOutdated,
  parseChecksum, ALGO_LABELS, historyCsv, localStamp, canChooseFiles, isFinishedDownload, isMagnet,
  shareRatio, qualityChoices, exactFormat, formatContent, parseMirrors, canAddMirror, sourceName,
  SOURCE_STATES, MAX_SOURCES, isMetalinkName, isMetalinkUrl, scanRequest, linkFilter, reclassifyLink,
  subDirParts, scopeAllows, pastedToken, isToken, pluginCandidate, pluginLinkName,
  splitListen, joinListen, pageLink, isLoopbackUrl, qrUrls, qrPath, remoteSettled, plannedUrls,
  ruleFromFields, ruleForSet, ruleSummary, ruleError, moveRule,
  pendingKeyFor, blockedByHostKey,
} from "./model.js";
import { tauriHost } from "./host.js";
import { webHost } from "./web.js";

// L'application de bureau (Tauri), ou la page servie par le moteur (interface distante,
// SPEC §7.19).
const host = window.__TAURI__ ? tauriHost() : webHost();
const store = new TaskStore();
let t = translator("fr");
let lang = "fr";
let info = null;
let connected = false;
let engineVersion = "";
let executables = new Set();

const $ = (id) => document.getElementById(id);

// ---------- Capacités (interface distante, étape 3c) ----------

/// Ce que cette page peut faire : services de l'hôte (application de bureau, ou page
/// distante), et portée de la connexion au moteur (SPEC §6.9 ; `admin` au bureau). Un élément
/// marqué `data-needs="…"` n'apparaît que si toutes ses capacités sont là.
const caps = {
  app: !host.remote,
  remote: Boolean(host.remote),
  files: typeof host.openFile === "function",
  folders: typeof host.pickFolder === "function",
  read: true,
  add: true,
  control: true,
  admin: true,
  // Accès à distance (étape 3d) : seulement avec un hôte qui sait faire un QR code.
  qr: typeof host.qrCode === "function",
  // Rubrique « Plugins » (étape 4c) : seulement avec un hôte qui sait lire un fichier choisi.
  plugins: typeof host.readPlugin === "function",
  // Export du diagnostic (N-12) : seulement avec un hôte qui sait enregistrer l'archive.
  diagnostic: typeof host.saveDiagnostic === "function",
  // Mises à jour de l'application (étape 8b) : vrai quand l'hôte dit que cette installation
  // se met à jour (`refreshUpdateStatus`).
  updates: false,
};

function applyScope(scope) {
  for (const need of ["read", "add", "control", "admin"]) caps[need] = scopeAllows(scope ?? "admin", need);
  markNeeds(document);
  for (const row of rows.values()) markNeeds(row);
  $("empty-add-only").hidden = caps.read || !caps.add;
}

/// Cache (classe `lacks`) les éléments de `root` dont une capacité manque.
function markNeeds(root) {
  for (const el of root.querySelectorAll("[data-needs]")) {
    el.classList.toggle("lacks", !el.dataset.needs.split(" ").every((need) => caps[need]));
  }
}

/// Sous-dossier saisi dans la page distante (champ `id`) : paramètres à ajouter à un ajout
/// (`subDir`), ou null s'il est refusé (`.` ou `..`).
function subDirParam(id) {
  if (!caps.remote) return {};
  const parts = subDirParts($(id).value);
  if (parts === null) return null;
  return parts ? { subDir: parts } : {};
}

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

// Une boîte modale rend le reste de la page inerte, hors de portée des lecteurs d'écran :
// bandeau d'erreur et annonces de la fenêtre s'y taisent (essai NVDA du 2026-11-18). Les
// messages vont donc dans la boîte au premier plan, s'il y en a une.

/// Boîte modale au premier plan : celle du focus, sinon la dernière ouverte.
function topDialog() {
  return document.activeElement?.closest?.("dialog[open]")
    ?? [...document.querySelectorAll("dialog[open]")].pop()
    ?? null;
}

/// Zone de messages d'une boîte, créée au premier besoin, avant ses boutons : `alert` pour
/// les erreurs (visibles), `status` pour les annonces (cachées à l'œil).
function dialogLive(dialog, kind) {
  let box = dialog.querySelector(`[data-live="${kind}"]`);
  if (!box) {
    box = document.createElement(kind === "alert" ? "p" : "div");
    box.dataset.live = kind;
    if (kind === "alert") {
      box.className = "form-error";
      box.setAttribute("role", "alert");
      box.hidden = true;
    } else {
      box.className = "visually-hidden";
      box.setAttribute("aria-live", "polite");
    }
    const buttons = dialog.querySelector(".buttons");
    if (buttons) buttons.before(box);
    else dialog.append(box);
  }
  return box;
}

// Créées dès le chargement : une zone `aria-live` doit exister avant que son texte change,
// sinon le lecteur d'écran prend le premier message pour du contenu initial et le tait
// (essai NVDA du 2026-11-18 : « Empreinte retirée » muet).
for (const dialog of document.querySelectorAll("dialog")) {
  dialogLive(dialog, "status");
  dialogLive(dialog, "alert");
}

// Une boîte fermée oublie ses messages.
document.addEventListener("close", (event) => {
  if (event.target?.tagName !== "DIALOG") return;
  clearFieldErrors(event.target);
  for (const box of event.target.querySelectorAll("[data-live]")) {
    box.textContent = "";
    if (box.dataset.live === "alert") box.hidden = true;
  }
}, true);

/// Erreur d'un champ d'une boîte : écrite dans la boîte et liée au champ (`aria-invalid`,
/// `aria-describedby`), lue avec lui quand il prend le focus. L'appelant le lui donne ensuite.
/// Une alerte seule est coupée par l'annonce du focus qui la suit (essai NVDA du 2026-11-18 :
/// ajout refusé affiché, mais pas lu).
function fieldError(field, text, box = null) {
  const dialog = field.closest("dialog");
  $("error").hidden = true;
  clearFieldErrors(dialog);
  box ??= dialogLive(dialog, "alert");
  box.id ||= `${dialog.id}-live-error`;
  box.dataset.fieldError = "";
  liveSerial.set(box, (liveSerial.get(box) ?? 0) + 1);
  box.textContent = text;
  box.hidden = false;
  field.setAttribute("aria-invalid", "true");
  field.dataset.errorLinked = "";
  const ids = (field.getAttribute("aria-describedby") ?? "").split(/\s+/).filter(Boolean);
  field.setAttribute("aria-describedby", [box.id, ...ids].join(" "));
  // Déjà au focus : rien ne le relira, l'alerte le dit.
  if (document.activeElement === field) speak(box, text);
}

function clearFieldErrors(dialog) {
  const boxes = new Set([...dialog.querySelectorAll("[data-field-error]")].map((box) => box.id));
  for (const field of dialog.querySelectorAll("[data-error-linked]")) {
    delete field.dataset.errorLinked;
    field.removeAttribute("aria-invalid");
    const ids = (field.getAttribute("aria-describedby") ?? "").split(/\s+/).filter((id) => id && !boxes.has(id));
    if (ids.length) field.setAttribute("aria-describedby", ids.join(" "));
    else field.removeAttribute("aria-describedby");
  }
}

/// Écrit un message après coup (image suivante) : un changement de contenu, lu par les
/// lecteurs d'écran même s'il redit le message d'avant. Un message plus récent l'emporte.
const liveSerial = new WeakMap();
function speak(box, text) {
  const serial = (liveSerial.get(box) ?? 0) + 1;
  liveSerial.set(box, serial);
  box.textContent = "";
  if (text) requestAnimationFrame(() => { if (liveSerial.get(box) === serial) box.textContent = text; });
}

function showError(text) {
  const dialog = topDialog();
  if (!dialog) {
    $("error").hidden = !text;
    speak($("error-text"), text);
    return;
  }
  $("error").hidden = true;
  clearFieldErrors(dialog);
  const box = dialogLive(dialog, "alert");
  box.hidden = !text;
  speak(box, text);
}

function refused(e) {
  return t("Action refusée : {0}", e?.message ?? String(e));
}

function announce(text) {
  const dialog = topDialog();
  speak(dialog ? dialogLive(dialog, "status") : $("announce"), text);
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

/// Barre d'avancement en relief : la `<progress>` qu'elle contient garde l'avancement
/// pour les lecteurs d'écran, invisible ; la barre dessinée prend sa longueur (`--p`, de 0
/// à 1) et sa couleur de l'état, et s'anime pendant le téléchargement.
function paintMeter(meter, task) {
  const p = percent(task);
  const unknown = p === null && task.state === "active";
  meter.dataset.state = task.state;
  meter.classList.toggle("unknown", unknown);
  meter.style.setProperty("--p", unknown ? "1" : String((p ?? 0) / 100));
}

/// État affiché d'une tâche : une tâche en file dont la file est hors de ses plages
/// horaires le dit.
function taskStateText(task) {
  if (task.state === "queued" && store.waitsForWindow(task.queueId)) {
    return t("En attente de la plage horaire");
  }
  // Torrent qui attend son interface réseau (VPN, SPEC §7.16).
  if (task.kind === "torrent" && task.state === "queued" && !store.torrentInterfaceUp) {
    return t("En attente de l'interface réseau");
  }
  const ratio = task.state === "seeding" ? shareRatio(task) : null;
  if (ratio !== null) return t("Partage (ratio {0})", ratioText(ratio));
  return stateText(task, t);
}

/// Ratio de partage, deux décimales.
const ratioText = (ratio) =>
  ratio.toLocaleString(lang, { minimumFractionDigits: 2, maximumFractionDigits: 2 });

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
    // Nom de la ligne : ses cellules, dans l'ordre. Chromium le tire du contenu, WebKitGTK non,
    // et Orca ne lisait rien d'une ligne qui prend le focus (essai Orca du 2026-11-18).
    const parts = ["name .text", "size", "pct", "state", "eta", "rate", "added"].map((sel) => {
      const part = row.querySelector(`.${sel}`);
      part.id = `row-${taskId}-${sel.split(" ")[0]}`;
      return part.id;
    });
    row.setAttribute("aria-labelledby", parts.join(" "));
    markNeeds(row);
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
  paintMeter(cell("progress").querySelector(".meter"), task);
  cell("pct").textContent = percentText(task);
  cell("state").textContent = taskStateText(task);
  cell("state").title = cell("state").textContent;
  const active = task.state === "active";
  cell("eta").textContent = active ? duration(task.etaS, t) : "";
  // Un torrent qui partage : son débit d'envoi.
  cell("rate").textContent = active
    ? rate(task.bps ?? 0)
    : task.state === "seeding" ? t("↑ {0}", rate(task.uploadBps ?? 0)) : "";
  cell("added").textContent = dateText(task.createdAt);
  cell("added").title = dateText(task.createdAt, longDateFormat);
  // Bouton de la carte (écran étroit, page distante) : arrêter ou reprendre cette tâche.
  const action = cell("action").querySelector("button");
  const verb = canPause(task.state) ? "pause" : canResume(task.state) ? "resume" : "";
  action.hidden = !verb;
  action.dataset.verb = verb;
  action.className = `row-action ${verb}`;
  action.querySelector("use").setAttribute("href", `#i-${verb || "pause"}`);
  action.setAttribute("aria-label", verb === "pause" ? t("Arrêter {0}", name) : t("Reprendre {0}", name));
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
  const action = event.target.closest(".row-action");
  if (action) {
    const task = store.tasks.get(row.dataset.taskId);
    if (task && connected) (action.dataset.verb === "pause" ? pause : resume)([task]);
    return;
  }
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
      if (caps.control) openRemove();
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
  $("tb-video").disabled = off;
  $("tb-links").disabled = off;
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
    item.disabled = (!connected && item.dataset.command !== "properties") || !enable[item.dataset.command]
      || item.classList.contains("lacks");
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
  // Un torrent qui partage a ses fichiers complets : comme un téléchargement terminé.
  const completed = chosen.some(isFinishedDownload);
  $("remove-text").textContent = chosen.length === 1
    ? t("Supprimer « {0} » de la liste ?", taskName(chosen[0]))
    : t("Supprimer {0} téléchargements de la liste ?", chosen.length);
  $("remove-files").checked = false;
  $("remove-files-label").hidden = !completed;
  $("remove-note").hidden = chosen.every(isFinishedDownload);
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
/// d'abord, puis se retirent quand le moteur annonce l'annulation. Un torrent qui partage a
/// ses fichiers complets : annulé, il ne les perd que si on le demande (son retrait, ensuite,
/// n'efface que des fichiers partiels, qu'il n'a plus).
async function deleteTasks(ids, deleteCompleted) {
  const tasks = ids.map((id) => store.tasks.get(id)).filter(Boolean);
  const done = tasks.filter((task) => task.state === "completed").map((task) => task.taskId);
  const finished = tasks.filter((task) => canRemove(task.state) && task.state !== "completed")
    .map((task) => task.taskId);
  const sharing = tasks.filter((task) => task.state === "seeding").map((task) => task.taskId);
  const running = tasks.filter((task) => canCancel(task.state) && task.state !== "seeding")
    .map((task) => task.taskId);
  if (done.length) await act("task.remove", { taskIds: done, deleteFiles: deleteCompleted });
  if (finished.length) await act("task.remove", { taskIds: finished, deleteFiles: true });
  for (const [list, deleteFiles] of [[sharing, deleteCompleted], [running, true]]) {
    if (!list.length) continue;
    for (const id of list) removeWhenCancelled.add(id);
    const result = await act("task.cancel", { taskIds: list, deleteFiles });
    for (const error of result?.errors ?? []) removeWhenCancelled.delete(error.taskId);
    if (!result) for (const id of list) removeWhenCancelled.delete(id);
  }
  removeCancelled();
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
  // Une tâche finie ne change plus : une seule lecture suffit, faite après la fin — sauf
  // si son état a changé depuis (relancée, retéléchargée).
  const stateRev = store.tasks.get(id)?.stateRev ?? null;
  if (!detailsOutdated(detailsFull, id, stateRev)) return;
  let full;
  // Un torrent : ses fichiers (SPEC §6.5) ; il n'a pas de segments.
  const include = store.tasks.get(id)?.kind === "torrent" ? ["sources", "files"] : ["segments", "sources"];
  try {
    full = await host.call("task.get", { taskId: id, include });
  } catch {
    return; // Tâche retirée ou moteur perdu : la notification suivra.
  }
  if (detailsTask !== id) return;
  // Révision lue avant la demande : un changement pendant celle-ci fera relire.
  full.stateRev = stateRev;
  detailsFull = full;
  renderDetails();
}

function openDetails(taskId, { focusLimit = false } = {}) {
  const task = store.tasks.get(taskId);
  if (!task) return;
  detailsTask = taskId;
  detailsFull = null;
  fileChoice.clear();
  $("d-files").tBodies[0].replaceChildren();
  clearInterval(detailsTimer);
  detailsTimer = setInterval(refreshDetails, 1000);
  refreshDetails();
  fillLimits($("d-limit"), task.speedLimitBps ?? null);
  // Rempli à l'ouverture seulement : l'affichage se refait chaque seconde.
  $("d-checksum-input").value = task.checksum ? `${task.checksum.algo}:${task.checksum.value}` : "";
  $("d-mirror-input").value = "";
  // Une autre tâche : sa carte se dessine à neuf.
  $("d-map").replaceChildren();
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
  const seeding = task.state === "seeding";
  const torrent = task.kind === "torrent";
  const source = detailsFull?.sources?.[0];
  set("name", name);
  set("dir", task.destDir ?? "");
  set("url", source?.url ?? "");
  // Tâche venue d'un plugin (SPEC §7.20) : sa page, et le plugin qui l'a lue.
  const origin = detailsFull?.plugin;
  set("plugin", origin
    ? t("{0} (plugin {1})", origin.page, origin.name ?? t("retiré"))
    : "");
  set("state", taskStateText(task));
  set("size", task.totalBytes != null ? bytes(task.totalBytes, lang, t) : t("inconnue"));
  set("done", pct ? t("{0} ({1})", bytes(task.doneBytes, lang, t), pct) : bytes(task.doneBytes, lang, t));
  set("rate", active ? rate(task.bps ?? 0) : seeding ? t("Envoi : {0}", rate(task.uploadBps ?? 0)) : "");
  set("eta", active ? duration(task.etaS, t) : "");
  // Un torrent compte des pairs, pas des connexions.
  $("d-connections-label").textContent = torrent ? t("Pairs") : t("Connexions");
  set("connections", (active || seeding) && task.connections > 0 ? String(task.connections) : "");
  const ratio = torrent ? shareRatio(task) : null;
  set("uploaded", ratio === null ? "" : t("{0} (ratio {1})", bytes(task.uploadedBytes, lang, t), ratioText(ratio)));
  // F-04 : un serveur sans reprise le dit clairement.
  set("resume", source?.acceptsRanges === true
    ? t("Oui")
    : source?.acceptsRanges === false
      ? t("Non : ce serveur ne permet pas de reprendre ; un arrêt ou une coupure fait repartir de zéro")
      : "");
  $("d-resume").classList.toggle("warn", source?.acceptsRanges === false);
  // F-11 : empreinte attendue ; celle d'un fichier terminé a été vérifiée. Lue dans un
  // fichier voisin (`source`), elle le dit.
  const sum = task.checksum;
  const label = sum ? ALGO_LABELS[sum.algo] ?? sum.algo : "";
  const verified = task.state === "completed";
  set("checksum", !sum
    ? ""
    : sum.source
      ? t(verified ? "{0} {1} (lue dans {2}, vérifiée)" : "{0} {1} (lue dans {2})", label, sum.value, sum.source)
      : t(verified ? "{0} {1} (vérifiée)" : "{0} {1}", label, sum.value));
  $("d-checksum").classList.toggle("warn", task.errorCode === "CHECKSUM_MISMATCH");
  // F-12 : empreintes de morceaux d'un Metalink, et morceaux téléchargés de nouveau.
  const pieces = detailsFull?.pieces;
  const piecesHead = pieces
    ? t("{0} morceaux de {1} ({2})", pieces.count, bytes(pieces.length, lang, t), ALGO_LABELS[pieces.algo] ?? pieces.algo)
    : "";
  set("pieces", !pieces
    ? ""
    : pieces.redone === 0
      ? piecesHead
      : pieces.redone === 1
        ? t("{0}, 1 retéléchargé", piecesHead)
        : t("{0}, {1} retéléchargés", piecesHead, pieces.redone));
  $("d-pieces").classList.toggle("warn", task.errorCode === "CHECKSUM_MISMATCH");
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
  paintMeter($("d-meter"), task);

  const toggleButton = $("d-toggle");
  const retry = task.state === "failed";
  const resumable = canResume(task.state);
  toggleButton.textContent = retry
    ? t("Relancer")
    : resumable ? t("Reprendre") : seeding ? t("Arrêter le partage") : t("Pause");
  toggleButton.hidden = !(resumable || canPause(task.state));
  toggleButton.disabled = !connected;
  // Pendant le partage, les fichiers sont complets : l'annulation les effacerait.
  $("d-cancel").hidden = !canCancel(task.state) || seeding;
  $("d-cancel").disabled = !connected;
  const done = task.state === "completed" && task.fileName;
  // Programmes et scripts : jamais ouverts depuis l'application (SPEC §8.1).
  $("d-open").hidden = !done || isExecutable(task.fileName);
  $("d-reveal").hidden = !done;
  $("d-limit-field").hidden = seeding || !(canPause(task.state) || canResume(task.state));
  $("d-limit-apply").disabled = !connected;
  $("d-limit-torrent-note").hidden = !torrent;
  renderSegments(task);
  renderSources(task);
  renderTorrentFiles(task);
  renderHostKey(task);
}

/// Nom court d'une source, traduit : « Principale » ou « Miroir 2 ».
function sourceText(source) {
  const [key, n] = sourceName(source);
  return t(key, n);
}

/// Sources d'un téléchargement HTTP (miroirs, SPEC §7.7) : rôle, adresse, état avec la raison
/// d'un refus, débit moyen ; et le champ « Ajouter un miroir » tant qu'il en accepte.
function renderSources(task) {
  const sources = detailsFull?.sources ?? [];
  const addable = canAddMirror(task, detailsFull?.sources?.[0]?.url);
  const section = $("d-sources-section");
  section.hidden = (task.kind ?? "http") !== "http" || !detailsFull || (sources.length < 2 && !addable);
  if (section.hidden) return;
  // Une seule source : son adresse est déjà plus haut.
  $("d-sources-box").hidden = sources.length < 2;
  $("d-mirror-field").hidden = !addable;
  $("d-mirror-add").disabled = !connected || addingMirror;
  $("d-sources").tBodies[0].replaceChildren(...sources.map((s) => {
    const row = document.createElement("tr");
    row.className = s.state;
    const state = t(SOURCE_STATES[s.state] ?? s.state);
    // Raison d'un refus ou du dernier échec ; une bonne source n'en a plus besoin.
    const status = s.lastError && s.state !== "ok" ? t("{0} : {1}", state, s.lastError) : state;
    const speed = s.avgBps > 0 ? rate(s.avgBps) : "";
    for (const [text, path] of [[sourceText(s), false], [s.url, true], [status, false], [speed, false]]) {
      const cell = document.createElement("td");
      cell.textContent = text;
      if (path) cell.className = "path";
      row.append(cell);
    }
    return row;
  }));
}

/// Un ajout de miroir attend la réponse du moteur (vérification du miroir comprise).
let addingMirror = false;

/// Ajoute un miroir à la tâche affichée (`task.addSource`) ; le moteur le vérifie aussitôt
/// si la tâche est préparée, et refuse un autre fichier (`SOURCE_MISMATCH`).
async function addMirror() {
  const input = $("d-mirror-input");
  const url = input.value.trim();
  const taskId = detailsTask;
  if (!url || !taskId || addingMirror) return;
  if (!/^https?:\/\/\S+$/i.test(url)) {
    fieldError(input, mirrorsError({ error: "scheme", url }));
    input.focus();
    return;
  }
  addingMirror = true;
  $("d-mirror-add").disabled = true;
  announce(t("Vérification du miroir…"));
  try {
    await host.call("task.addSource", { taskId, url });
    input.value = "";
    showError("");
    announce(t("Miroir ajouté"));
  } catch (e) {
    showError(refused(e));
  } finally {
    addingMirror = false;
    if (detailsTask === taskId) {
      refreshDetails();
      input.focus();
    }
  }
}

/// Choix des fichiers d'un torrent pas encore appliqué : rang → coché. Il passe avant ce que
/// dit le moteur, relu chaque seconde.
const fileChoice = new Map();

/// Fichiers d'un torrent (SPEC §7.16) : cases à cocher tant qu'on peut choisir, taille et
/// part reçue. Mis à jour sur place, pour garder le focus d'une case.
function renderTorrentFiles(task) {
  const section = $("d-files-section");
  section.hidden = task.kind !== "torrent";
  if (section.hidden) return;
  const files = detailsFull?.files ?? [];
  $("d-files-wait").hidden = files.length > 0;
  $("d-files-box").hidden = !files.length;
  const editable = canChooseFiles(task) && connected;
  $("d-files-apply").hidden = !files.length || !canChooseFiles(task);
  $("d-files-apply").disabled = !editable || fileChoice.size === 0;
  const tbody = $("d-files").tBodies[0];
  if (tbody.rows.length !== files.length) {
    tbody.replaceChildren(...files.map((f) => {
      const row = document.createElement("tr");
      const box = document.createElement("input");
      box.type = "checkbox";
      box.dataset.index = String(f.index);
      const check = document.createElement("td");
      check.append(box);
      row.append(check, ...[0, 1, 2].map(() => document.createElement("td")));
      return row;
    }));
  }
  files.forEach((f, i) => {
    const row = tbody.rows[i];
    const box = row.cells[0].firstElementChild;
    box.dataset.index = String(f.index);
    box.checked = fileChoice.get(f.index) ?? f.selected;
    box.disabled = !editable;
    box.setAttribute("aria-label", t("Télécharger {0}", f.path));
    row.cells[1].textContent = f.path;
    row.cells[1].className = "path";
    row.cells[2].textContent = bytes(f.length, lang, t);
    const part = f.length > 0 ? Math.floor((100 * Math.min(f.doneBytes, f.length)) / f.length) : 100;
    row.cells[3].textContent = f.selected || f.doneBytes > 0 ? t("{0} %", part) : "";
    row.classList.toggle("skipped", !box.checked);
  });
}

/// Applique le choix des fichiers (`task.setFiles`) ; au moins un.
async function applyFileChoice() {
  const boxes = [...$("d-files").querySelectorAll("input[type=checkbox]")];
  const selected = boxes.filter((b) => b.checked).map((b) => Number(b.dataset.index));
  if (!selected.length) {
    showError(t("Choisissez au moins un fichier."));
    return;
  }
  try {
    const task = await host.call("task.setFiles", { taskId: detailsTask, selected });
    fileChoice.clear();
    detailsFull = null;
    showError("");
    announce(t("{0} fichier(s) choisi(s), {1} à télécharger", selected.length, bytes(task.totalBytes, lang, t)));
    refreshDetails();
  } catch (e) {
    showError(refused(e));
  }
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
  // Les segments déjà dessinés sont gardés : leurs reflets continuent d'une lecture à la
  // suivante au lieu de repartir chaque seconde.
  while (map.children.length > segments.length) map.lastElementChild.remove();
  segments.forEach((s, i) => {
    let part = map.children[i];
    if (!part) {
      part = document.createElement("div");
      const fill = document.createElement("div");
      fill.className = "fill";
      part.append(fill);
      map.append(part);
    }
    part.className = s.receiving ? "part receiving" : "part";
    part.style.left = pct(s.start);
    part.style.width = pct(size(s));
    part.firstElementChild.style.width = `${Math.min(100, (100 * s.doneBytes) / size(s))}%`;
  });
  const receiving = segments.filter((s) => s.receiving).length;
  map.setAttribute("aria-label", t("{0} segments, {1} en cours de réception", segments.length, receiving));
  // Miroirs (SPEC §7.7) : la source de chaque segment reçu, quand il y en a plusieurs.
  const sources = detailsFull?.sources ?? [];
  $("d-segments").classList.toggle("mirrored", sources.length > 1);
  const from = (s) => {
    const source = s.sourceId && sources.find((x) => x.sourceId === s.sourceId);
    return source ? sourceText(source) : "";
  };
  $("d-segments").tBodies[0].replaceChildren(...segments.map((s, i) => {
    const row = document.createElement("tr");
    const finished = s.doneBytes >= size(s);
    const status = s.receiving ? t("Réception des données") : finished ? t("Terminé") : t("En attente");
    row.className = s.receiving ? "receiving" : finished ? "finished" : "";
    for (const text of [String(i + 1), t("{0} sur {1}", bytes(s.doneBytes, lang, t), bytes(size(s), lang, t)), status, from(s)]) {
      const cell = document.createElement("td");
      cell.textContent = text;
      row.append(cell);
    }
    row.lastElementChild.className = "source-column";
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

/// Message d'un champ « Miroirs » refusé par `parseMirrors`.
function mirrorsError({ error, url }) {
  switch (error) {
    case "many": return t("Des miroirs ne valent que pour une seule adresse.");
    case "torrent": return t("Un torrent trouve ses pairs lui-même : pas de miroir à donner.");
    case "metalink": return t("Un Metalink donne lui-même les miroirs de chaque fichier.");
    case "transfer": return t("Une adresse FTP ou SFTP n'a qu'une source : pas de miroir.");
    case "plugin": return t("Une page d'hébergeur donne ses liens par son plugin : pas de miroir à donner.");
    case "scheme": return t("Un miroir est une adresse http:// ou https:// : {0}", url);
    case "twice": return t("Adresse donnée deux fois : {0}", url);
    default: return t("{0} sources au plus, la principale comprise.", MAX_SOURCES);
  }
}

/// Message d'une empreinte mal écrite.
function checksumError() {
  return t("Empreinte illisible : 64 (SHA-256), 40 (SHA-1) ou 32 (MD5) chiffres hexadécimaux, ou « sha256:… »");
}

/// Empreinte attendue d'un téléchargement (F-11) : donnée, changée, ou retirée (champ vide).
async function applyChecksum() {
  const { checksum, error } = parseChecksum($("d-checksum-input").value);
  if (error) {
    fieldError($("d-checksum-input"), checksumError());
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
  if ($("end-action-text").textContent !== text) {
    $("end-action-text").textContent = text;
    // Derrière une boîte modale, le bandeau est inerte : l'annonce est dite dans la boîte
    // (Échap la ferme et rend « Annuler » joignable).
    if (topDialog()) announce(text);
  }
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
  if (!caps.read) {
    renderAll();
    return;
  }
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
  renderTorrentSettings();
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
    // Vidéo : seulement si le moteur la sait (Windows et Linux, SPEC §7.17).
    mediaCapable = status.engine?.capabilities?.includes("media") === true;
    applyScope(status.engine?.scope);
    $("tb-video").hidden = !mediaCapable || !caps.add;
    // Analyse de liens (SPEC §7.18) : un moteur plus ancien ne la sait pas.
    linkscanCapable = status.engine?.capabilities?.includes("linkscan") === true;
    $("tb-links").hidden = !linkscanCapable || !caps.add;
    $("tab-media").hidden = !mediaCapable;
    resync();
    loadTools();
    if (openedWaiting) {
      showError("");
      openAdd();
    }
  } else {
    banner.hidden = false;
    banner.textContent = status.state === "connecting"
      ? t("Connexion au moteur…")
      : status.message
        ? t("Moteur injoignable : {0}. Nouvel essai automatique.", status.message)
        : t("Moteur injoignable.");
  }
  renderTorrentSettings();
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
      if (result.limits || result.settings || result.torrentInterface) renderTorrentSettings();
      if (result.tools || result.settings) renderTools();
      // Réglages de l'écoute changés ailleurs (CLI) pendant que la rubrique est ouverte.
      if (result.settings && $("settings").open && !$("panel-remote").hidden) settleRemote();
      // Outils installés pendant que la boîte « Vidéo » attendait : la page est analysée.
      if (result.tools && probeAfterInstall && store.tools?.ready && $("video-dialog").open) {
        probeAfterInstall = false;
        probeVideo();
      }
      showWarnings(result.warnings);
      // Une file en attente de son interface change d'état affiché.
      if (result.torrentInterface) renderAll();
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
  for (const id of ["dest-change", "pref-dest-change", "tb-history", "pref-preallocate", "pref-neighbor"]) {
    $(id).disabled = !connected;
  }
  // F-10 : vrai tant que le moteur ne dit pas le contraire.
  $("pref-preallocate").checked = store.settings["downloads.preallocate"] !== false;
  // F-11 : de même.
  $("pref-neighbor").checked = store.settings["downloads.neighborChecksum"] !== false;
  // Journaux (N-12) : `info` tant que le moteur ne dit pas autre chose.
  $("pref-logs-level").value = store.settings["logs.level"] ?? "info";
  $("pref-logs-level").disabled = !connected;
  $("diagnostic-export").disabled = !connected;
  syncLoginButtons();
  syncHostKeyButtons();
  syncRuleButtons();
}

/// Niveau des journaux du moteur (N-12), appliqué sans redémarrer.
async function saveLogsLevel() {
  const level = $("pref-logs-level").value;
  try {
    await host.call("settings.set", { values: { "logs.level": level } });
    announce(t("Détail des journaux changé"));
  } catch (e) {
    showError(refused(e));
    renderDest();
  }
}

/// Proxy du moteur (F-07), lu à l'ouverture des Options (`proxy.get`) ; jamais son mot de
/// passe, seulement s'il y en a un.
let proxyView = null;

async function loadProxy() {
  $("proxy-error").hidden = true;
  try {
    proxyView = await host.call("proxy.get", {});
  } catch (e) {
    proxyView = null;
    $("proxy-error").textContent = refused(e);
    $("proxy-error").hidden = false;
  }
  renderProxy();
}

function renderProxy() {
  const view = proxyView ?? { mode: "system", url: null, exceptions: [], username: null, hasPassword: false };
  for (const mode of ["system", "none", "manual"]) $(`proxy-${mode}`).checked = view.mode === mode;
  $("proxy-url").value = view.url ?? "";
  $("proxy-except").value = (view.exceptions ?? []).join(", ");
  $("proxy-user").value = view.username ?? "";
  $("proxy-password").value = "";
  $("proxy-password-note").hidden = !view.hasPassword;
  syncProxyFields();
}

/// Les champs du proxy manuel ne servent qu'en mode manuel.
function syncProxyFields() {
  const manual = $("proxy-manual").checked;
  for (const id of ["proxy-url", "proxy-except", "proxy-user", "proxy-password"]) $(id).disabled = !manual || !connected;
  $("proxy-apply").disabled = !connected;
}

async function saveProxy() {
  const mode = ["system", "none", "manual"].find((m) => $(`proxy-${m}`).checked) ?? "system";
  const params = { mode };
  if (mode === "manual") {
    params.url = $("proxy-url").value.trim();
    params.exceptions = $("proxy-except").value.split(/[\s,]+/).filter(Boolean);
    const user = $("proxy-user").value.trim();
    if (user) {
      params.username = user;
      // Vide : le mot de passe enregistré reste.
      const password = $("proxy-password").value;
      if (password) params.password = password;
      else if (!proxyView?.hasPassword) params.password = null;
    }
  }
  $("proxy-apply").disabled = true;
  try {
    proxyView = await host.call("proxy.set", params);
    $("proxy-error").hidden = true;
    announce(t("Proxy appliqué"));
    renderProxy();
  } catch (e) {
    $("proxy-error").textContent = e?.message ?? String(e);
    $("proxy-error").hidden = false;
  } finally {
    // Le mot de passe ne reste pas dans la page.
    $("proxy-password").value = "";
    $("proxy-apply").disabled = !connected;
  }
}

/// Lignes de journal montrées avant l'enregistrement (les dernières).
const DIAGNOSTIC_SHOWN = 20;
/// Diagnostic lu (`engine.diagnostic`), montré puis enregistré tel quel.
let pendingDiagnostic = null;

async function openDiagnostic() {
  let found;
  try {
    found = await host.call("engine.diagnostic", {});
  } catch (e) {
    showError(refused(e));
    return;
  }
  pendingDiagnostic = found;
  const logs = found.logs ?? [];
  $("diag-report").textContent = JSON.stringify(found.report, null, 2);
  $("diag-count").textContent = logs.length
    ? t("{0} ligne(s) de journal ; les {1} dernières :", logs.length, Math.min(DIAGNOSTIC_SHOWN, logs.length))
    : t("Aucune ligne de journal.");
  $("diag-logs").textContent = logs.slice(-DIAGNOSTIC_SHOWN).join("\n");
  $("diag-logs").hidden = !logs.length;
  $("diagnostic-error").hidden = true;
  const dialog = $("diagnostic-dialog");
  dialog.returnValue = "";
  dialog.showModal();
  $("diagnostic-save").focus();
}

async function saveDiagnostic() {
  if (!pendingDiagnostic) return;
  $("diagnostic-save").disabled = true;
  try {
    const path = await host.saveDiagnostic(pendingDiagnostic);
    if (path) {
      $("diagnostic-dialog").close();
      announce(t("Diagnostic enregistré : {0}", path));
    }
  } catch (e) {
    $("diagnostic-error").textContent = e?.message ?? String(e);
    $("diagnostic-error").hidden = false;
  } finally {
    $("diagnostic-save").disabled = false;
  }
}

/// Réservation de la place des fichiers (F-10), réglage du moteur.
async function savePreallocate() {
  const on = $("pref-preallocate").checked;
  try {
    await host.call("settings.set", { values: { "downloads.preallocate": on } });
    announce(on ? t("Place réservée dès le départ") : t("Place prise au fil du téléchargement"));
  } catch (e) {
    showError(refused(e));
    renderDest();
  }
}

/// Empreintes lues dans les fichiers voisins (F-11), réglage du moteur.
async function saveNeighbor() {
  const on = $("pref-neighbor").checked;
  try {
    await host.call("settings.set", { values: { "downloads.neighborChecksum": on } });
    announce(on ? t("Empreintes cherchées à côté des fichiers") : t("Empreintes non cherchées à côté des fichiers"));
  } catch (e) {
    showError(refused(e));
    renderDest();
  }
}

/// Fichiers `.torrent` à ajouter : `{name, data}` (base64), lus par l'hôte.
let addTorrents = [];
/// Des liens reçus attendent que la boîte « Ajouter » puisse s'ouvrir (moteur pas encore joint).
let openedWaiting = false;

function renderAddTorrents() {
  const list = $("add-torrents");
  list.replaceChildren(...addTorrents.map((file, i) => {
    const item = document.createElement("li");
    const name = document.createElement("span");
    name.className = "path";
    name.textContent = file.name;
    const drop = document.createElement("button");
    drop.type = "button";
    drop.textContent = t("Retirer");
    drop.setAttribute("aria-label", t("Retirer {0}", file.name));
    drop.addEventListener("click", () => {
      addTorrents.splice(i, 1);
      renderAddTorrents();
      $("add-torrent-pick").focus();
    });
    item.append(name, drop);
    return item;
  }));
  list.hidden = !addTorrents.length;
  renderAddMetalinks();
  // Sans hôte capable de lire un fichier (interface distante), pas de choix de fichier ;
  // un Metalink lu à son adresse s'y montre quand même.
  const local = Boolean(host.readTorrent);
  $("add-torrent-pick").closest(".row").hidden = !local;
  $("add-torrents-note").hidden = !local;
  $("add-torrents-field").hidden = !local && !addMetalinks.length;
}

/// Lit des fichiers `.torrent` (chemins) et les ajoute à la boîte ; un refus est montré.
async function takeTorrentFiles(paths) {
  for (const path of paths) {
    try {
      const file = await host.readTorrent(path);
      if (isMetalinkName(file.name)) {
        await takeMetalink(file);
      } else if (!addTorrents.some((f) => f.data === file.data)) {
        addTorrents.push(file);
      }
    } catch (e) {
      showError(refused(e));
    }
  }
  renderAddTorrents();
}

/// Fichiers Metalink de la boîte « Ajouter » (F-27, SPEC §7.7) : `{name, data, doc, chosen}`,
/// `doc` lu par le moteur (`metalink.read`), `chosen` les rangs des fichiers cochés.
let addMetalinks = [];

const metalinkEntry = (name, data, doc) => ({ name, data, doc, chosen: new Set(doc.files.map((f) => f.index)) });

/// Lit un fichier Metalink choisi ou reçu (`{name, data}`), tous ses fichiers cochés.
async function takeMetalink(file) {
  if (addMetalinks.some((m) => m.data === file.data)) return;
  // Moteur pas encore joint (fichier ouvert au lancement) : lu à l'ouverture de la boîte.
  if (!connected) {
    addMetalinks.push({ name: file.name, data: file.data, doc: null, chosen: new Set() });
    return;
  }
  const doc = await host.call("metalink.read", { data: file.data });
  addMetalinks.push(metalinkEntry(file.name, file.data, doc));
}

/// Lit les Metalink reçus avant que le moteur soit joint ; un refus est montré.
async function readWaitingMetalinks() {
  for (const m of addMetalinks.filter((x) => !x.doc)) {
    try {
      Object.assign(m, metalinkEntry(m.name, m.data, await host.call("metalink.read", { data: m.data })));
    } catch (e) {
      addMetalinks.splice(addMetalinks.indexOf(m), 1);
      showError(refused(e));
    }
  }
  renderAddMetalinks();
}

/// Fichiers de chaque Metalink, à cocher, et ceux que le moteur a écartés, avec la raison.
function renderAddMetalinks() {
  const box = $("add-metalinks");
  box.replaceChildren(...addMetalinks.filter((m) => m.doc).map((m) => {
    const group = document.createElement("fieldset");
    const legend = document.createElement("legend");
    const title = document.createElement("span");
    title.className = "path";
    title.textContent = m.name;
    const drop = document.createElement("button");
    drop.type = "button";
    drop.textContent = t("Retirer");
    drop.setAttribute("aria-label", t("Retirer {0}", m.name));
    drop.addEventListener("click", () => {
      addMetalinks.splice(addMetalinks.indexOf(m), 1);
      renderAddMetalinks();
      $("add-torrent-pick").focus();
    });
    legend.append(title, drop);
    group.append(legend);
    for (const f of m.doc.files) {
      const label = document.createElement("label");
      label.className = "check";
      const tick = document.createElement("input");
      tick.type = "checkbox";
      tick.checked = m.chosen.has(f.index);
      tick.addEventListener("change", () => {
        if (tick.checked) m.chosen.add(f.index);
        else m.chosen.delete(f.index);
      });
      const text = document.createElement("span");
      text.className = "path";
      text.textContent = f.size != null ? t("{0} ({1})", f.path, bytes(f.size, lang, t)) : f.path;
      label.append(tick, text);
      group.append(label);
    }
    for (const ignored of m.doc.ignored) {
      const note = document.createElement("p");
      note.className = "note flush";
      note.textContent = t("Écarté : {0} ({1})", ignored.name, ignored.reason);
      group.append(note);
    }
    return group;
  }));
  box.hidden = !addMetalinks.length;
}

/// Pages d'hébergeurs lues par leur plugin (SPEC §7.20, étape 4c), en attente de l'ajout :
/// `{resolution, chosen}`, `resolution` rendue par `plugin.resolve` (gardée 10 minutes par
/// le moteur), `chosen` les rangs des liens cochés.
let addPluginPages = [];
/// Numéro de la dernière recherche des plugins des adresses saisies : une réponse plus
/// ancienne est écartée.
let pluginMatchSeq = 0;
let pluginMatchTimer = 0;

/// Plugins des adresses (`plugin.match`, sans requête vers les sites) : adresse → `{pluginId,
/// name}`. Un moteur sans plugins, ou un refus, ne donne rien.
async function matchPlugins(urls) {
  const found = new Map();
  for (const url of urls.filter(pluginCandidate).slice(0, 50)) {
    try {
      const { plugin } = await host.call("plugin.match", { url });
      if (plugin) found.set(url, plugin);
    } catch {
      break;
    }
  }
  return found;
}

/// Montre, sous les adresses, les plugins qui liront certaines d'entre elles, et la case
/// pour les ajouter telles quelles.
async function renderPluginMatch() {
  const seq = ++pluginMatchSeq;
  const urls = $("urls").value.split(/\s+/).filter(Boolean);
  const found = connected ? await matchPlugins(urls) : new Map();
  if (seq !== pluginMatchSeq) return;
  const names = [...new Set([...found.values()].map((p) => p.name))];
  $("add-plugin-note").textContent = names.length === 1
    ? t("Page lue par le plugin {0}, qui en donnera les liens.", names[0])
    : t("Pages lues par les plugins {0}, qui en donneront les liens.", names.join(", "));
  $("add-plugin-row").hidden = !names.length;
}

function schedulePluginMatch() {
  clearTimeout(pluginMatchTimer);
  pluginMatchTimer = setTimeout(renderPluginMatch, 300);
}

/// Liens de chaque page lue, à cocher, et les messages du plugin, repliés.
function renderAddPluginPages() {
  const box = $("add-plugin-pages");
  box.replaceChildren(...addPluginPages.map((entry) => {
    const r = entry.resolution;
    const group = document.createElement("fieldset");
    const legend = document.createElement("legend");
    const title = document.createElement("span");
    title.className = "path";
    title.textContent = t("{0} (plugin {1})", r.page, r.pluginName);
    const drop = document.createElement("button");
    drop.type = "button";
    drop.textContent = t("Retirer");
    drop.setAttribute("aria-label", t("Retirer {0}", r.page));
    drop.addEventListener("click", () => {
      addPluginPages.splice(addPluginPages.indexOf(entry), 1);
      renderAddPluginPages();
      $("urls").focus();
    });
    legend.append(title, drop);
    group.append(legend);
    r.links.forEach((link, index) => {
      const label = document.createElement("label");
      label.className = "check";
      const tick = document.createElement("input");
      tick.type = "checkbox";
      tick.checked = entry.chosen.has(index);
      tick.addEventListener("change", () => {
        if (tick.checked) entry.chosen.add(index);
        else entry.chosen.delete(index);
      });
      const text = document.createElement("span");
      text.className = "path";
      const name = pluginLinkName(link);
      text.textContent = link.size != null ? t("{0} ({1})", name, bytes(link.size, lang, t)) : name;
      text.title = link.url;
      label.append(tick, text);
      group.append(label);
    });
    if (!r.links.length) {
      const none = document.createElement("p");
      none.className = "note flush";
      none.textContent = t("Le plugin n'a trouvé aucun lien sur cette page.");
      group.append(none);
    }
    // Messages du plugin (`auro.log`) : repliés, ouverts quand il n'a rien trouvé.
    if (r.logs?.length) {
      const details = document.createElement("details");
      details.open = !r.links.length;
      const summary = document.createElement("summary");
      summary.textContent = t("Messages du plugin ({0})", r.logs.length);
      const pre = document.createElement("pre");
      pre.textContent = r.logs.join("\n");
      details.append(summary, pre);
      group.append(details);
    }
    return group;
  }));
  box.hidden = !addPluginPages.length;
}

async function pickTorrentFiles() {
  const paths = await host.pickTorrents().catch(() => []);
  await takeTorrentFiles(paths);
  $("add-torrent-pick").focus();
}

/// Liens magnet et fichiers `.torrent` ouverts avec l'application (« Ouvrir avec… »,
/// association, glisser-déposer) : mis dans la boîte « Ajouter », jamais ajoutés d'office.
async function takeOpened() {
  let opened;
  try {
    opened = await host.takeOpened();
  } catch {
    return;
  }
  if (!opened?.length) return;
  const field = $("urls");
  const lines = field.value.split(/\s+/).filter(Boolean);
  for (const item of opened) {
    if ((item.kind === "magnet" || item.kind === "link") && !lines.includes(item.link)) lines.push(item.link);
  }
  field.value = lines.join("\n");
  await takeTorrentFiles(opened.filter((item) => item.kind === "torrent" || item.kind === "metalink").map((item) => item.path));
  if ($("add-dialog").open) return;
  if (!connected || document.querySelector("dialog[open]")) {
    openedWaiting = true;
    showError(t("Un lien ou un fichier vous attend dans « Ajouter »."));
    return;
  }
  openAdd();
}

function openAdd() {
  if (!connected || !caps.add || document.querySelector("dialog[open]")) return;
  openedWaiting = false;
  renderDest();
  renderAddTorrents();
  readWaitingMetalinks();
  renderAddPluginPages();
  renderPluginMatch();
  const dialog = $("add-dialog");
  dialog.returnValue = "";
  dialog.showModal();
  $("urls").focus();
}

/// Ajout refusé, ou choix à faire : la boîte reste ouverte, le message lié au champ en cause
/// (`fieldError`), qui reprend le focus. Elle ne se ferme qu'une fois tout ajouté : refermée
/// puis rouverte, elle était relue en entier par NVDA, le message en dernier (essai du
/// 2026-11-18).
function addFailed(text, fieldId = "urls") {
  if (!$("add-dialog").open) {
    // Fermée entre-temps (Échap, moteur perdu) : le message va au bandeau de la fenêtre.
    showError(text);
    return;
  }
  renderAddTorrents();
  renderAddPluginPages();
  renderPluginMatch();
  fieldError($(fieldId), text);
  $(fieldId).focus();
}

/// Un ajout attend le moteur : un second « Démarrer » n'en lance pas un autre.
let addBusy = false;

/// « Démarrer » ou « Télécharger plus tard » (`choice`), la boîte ouverte.
async function submitAdd(choice) {
  if (addBusy) return;
  addBusy = true;
  try {
    await addFromDialog(choice);
  } finally {
    addBusy = false;
  }
}

async function addFromDialog(choice) {
  const dialog = $("add-dialog");
  const field = $("urls");
  let urls = field.value.split(/\s+/).filter(Boolean);
  if ((!urls.length && !addTorrents.length && !addMetalinks.length && !addPluginPages.length) || !connected) {
    dialog.close(choice);
    return;
  }
  // Pages d'hébergeurs (SPEC §7.20) : celles qu'un plugin lira, sauf « sans plugin ».
  const noPlugin = $("add-no-plugin").checked;
  const pluginUrls = noPlugin ? new Map() : await matchPlugins(urls);
  const hosted = pluginUrls.size > 0 || addPluginPages.length > 0;
  // F-11 : l'empreinte est celle d'un seul fichier, téléchargé en HTTP.
  const { checksum, error } = parseChecksum($("add-checksum").value);
  const torrents = addTorrents.length > 0 || urls.some(isMagnet);
  const metalinks = addMetalinks.length > 0 || urls.some(isMetalinkUrl);
  const items = urls.length + addTorrents.length + addMetalinks.length + addPluginPages.length;
  if (error || (checksum && (items > 1 || torrents || metalinks || hosted))) {
    addFailed(error
      ? checksumError()
      : torrents
        ? t("Un torrent vérifie ses morceaux lui-même : pas d'empreinte à donner.")
        : metalinks
          ? t("Un Metalink donne lui-même l'empreinte de chaque fichier.")
          : hosted
            ? t("Une page d'hébergeur donne ses liens par son plugin : pas d'empreinte à donner.")
            : t("Une empreinte ne vaut que pour une seule adresse."), "add-checksum");
    return;
  }
  const sub = subDirParam("add-subdir");
  if (sub === null) {
    addFailed(t("Sous-dossier refusé : « . » et « .. » ne sont pas permis."), "add-subdir");
    return;
  }
  // Miroirs (SPEC §7.7) : d'autres adresses du même fichier, pour une seule adresse HTTP.
  const parsed = parseMirrors($("add-mirrors").value, urls, addTorrents.length, addMetalinks.length);
  if (!parsed.error && parsed.mirrors.length && hosted) parsed.error = "plugin";
  if (parsed.error) {
    addFailed(mirrorsError(parsed), "add-mirrors");
    return;
  }
  const mirrors = parsed.mirrors.map((url) => ({ url }));
  // Adresses de Metalink : lues d'abord par le moteur. Un Metalink de plusieurs fichiers, ou
  // qui en écarte, revient dans la boîte pour le choix ; rien n'est ajouté avant.
  const metalinkUrls = urls.filter(isMetalinkUrl);
  if (metalinkUrls.length) {
    const unread = [];
    let readError = null;
    let toChoose = false;
    for (const url of metalinkUrls) {
      try {
        const doc = await host.call("metalink.read", { url });
        const name = url.split(/[?#]/)[0].split("/").pop() || url;
        addMetalinks.push(metalinkEntry(name, doc.data, doc));
        toChoose ||= doc.files.length > 1 || doc.ignored.length > 0;
      } catch (e) {
        unread.push(url);
        readError ??= e;
      }
    }
    urls = urls.filter((u) => !isMetalinkUrl(u) || unread.includes(u));
    field.value = urls.join("\n");
    if (readError || toChoose) {
      addFailed(readError
        ? refused(readError)
        : t("Choisissez les fichiers du Metalink, puis « Démarrer » ou « Télécharger plus tard »."));
      return;
    }
  }
  // Pages d'hébergeurs : lues d'abord par leur plugin. Une page d'un seul lien s'ajoute
  // ensuite avec le reste ; une page de plusieurs liens, ou d'aucun, revient dans la boîte
  // pour le choix. Une page que le plugin n'a pas pu lire reste dans le champ.
  if (pluginUrls.size) {
    const unread = [];
    let readError = null;
    let toChoose = false;
    for (const url of pluginUrls.keys()) {
      try {
        const resolution = await host.call("plugin.resolve", { url });
        addPluginPages.push({ resolution, chosen: new Set(resolution.links.map((_, i) => i)) });
        toChoose ||= resolution.links.length !== 1;
      } catch (e) {
        unread.push(url);
        readError ??= { message: t("{0} : {1}", url, e?.message ?? String(e)) };
      }
    }
    urls = urls.filter((u) => !pluginUrls.has(u) || unread.includes(u));
    field.value = urls.join("\n");
    renderPluginMatch();
    if (readError || toChoose) {
      addFailed(readError
        ? refused(readError)
        : t("Choisissez les liens des pages d'hébergeurs, puis « Démarrer » ou « Télécharger plus tard »."));
      return;
    }
  }
  const failed = [];
  const failedTorrents = [];
  const failedMetalinks = [];
  const failedPages = [];
  let firstError = null;
  // Sans destDir : le dossier des téléchargements du moteur. « Sans plugin » : la page
  // telle quelle (`plugin: false`).
  const add = async (source) => {
    const params = { sources: [source, ...(source.url ? mirrors : [])], startPaused: choice === "later", ...sub };
    if (checksum) params.checksum = checksum;
    if (noPlugin && source.url) params.plugin = false;
    await host.call("task.add", params);
  };
  for (const url of urls) {
    try {
      await add({ url });
    } catch (e) {
      failed.push(url);
      firstError ??= e;
    }
  }
  for (const file of addTorrents) {
    try {
      await add({ torrent: file.data });
    } catch (e) {
      failedTorrents.push(file);
      firstError ??= e;
    }
  }
  // Une tâche par fichier coché ; un Metalink dont un fichier est refusé reste dans la boîte,
  // ce fichier seul coché.
  for (const m of addMetalinks.filter((x) => x.doc)) {
    const select = [...m.chosen].sort((a, b) => a - b);
    if (!select.length) {
      failedMetalinks.push(m);
      firstError ??= { message: t("aucun fichier choisi dans {0}", m.name) };
      continue;
    }
    try {
      const result = await host.call("metalink.add", { data: m.data, select, startPaused: choice === "later", ...sub });
      if (result.errors.length) {
        m.chosen = new Set(result.errors.map((e) => e.index));
        failedMetalinks.push(m);
        firstError ??= { message: t("{0} : {1}", result.errors[0].path, result.errors[0].message) };
      }
    } catch (e) {
      failedMetalinks.push(m);
      firstError ??= e;
    }
  }
  // Une tâche par lien coché d'une page lue ; une page dont un lien est refusé reste dans
  // la boîte, ce lien seul coché.
  for (const page of addPluginPages) {
    const r = page.resolution;
    const links = [...page.chosen].sort((a, b) => a - b);
    if (!links.length) {
      failedPages.push(page);
      firstError ??= { message: t("aucun lien choisi dans {0}", r.page) };
      continue;
    }
    try {
      const result = await host.call("plugin.add", { resolutionId: r.resolutionId, links, startPaused: choice === "later", ...sub });
      if (result.errors.length) {
        page.chosen = new Set(result.errors.map((e) => e.index));
        failedPages.push(page);
        const first = result.errors[0];
        firstError ??= { message: t("{0} : {1}", pluginLinkName(r.links[first.index] ?? { url: r.page }), first.message) };
      }
    } catch (e) {
      failedPages.push(page);
      firstError ??= e;
    }
  }
  field.value = failed.join("\n");
  addTorrents = failedTorrents;
  addMetalinks = failedMetalinks;
  addPluginPages = failedPages;
  const refusedCount = failed.length + failedTorrents.length + failedMetalinks.length + failedPages.length;
  if (!refusedCount) {
    $("add-checksum").value = "";
    $("add-mirrors").value = "";
    $("add-no-plugin").checked = false;
    $("add-plugin-row").hidden = true;
  }
  if (refusedCount) {
    addFailed(t("{0} ajout(s) refusé(s) : {1}", refusedCount, firstError?.message ?? ""));
    return;
  }
  dialog.close(choice);
  showError("");
  // Les nouvelles tâches doivent se voir.
  if (view.startsWith("completed")) chooseView("all");
}

// ---------- Vidéo (SPEC §7.17, étape 5c) ----------

/// Le moteur sait télécharger des vidéos (capacité `media` : Windows et Linux).
let mediaCapable = false;
/// Dernière analyse (`media.probe`) et l'adresse analysée ; `null` tant qu'il n'y en a pas.
let videoProbe = null;
let probedPage = "";
/// Numéro de la dernière analyse demandée : une réponse plus ancienne est écartée.
let probeSeq = 0;
/// Analyser dès que les outils seront installés.
let probeAfterInstall = false;

/// Taille estimée : « ≈ 12,3 Mio » ou « 12,3 Mio ».
function sizeText(size) {
  if (!size) return "";
  const text = bytes(size.bytes, lang, t);
  return size.approx ? t("≈ {0}", text) : text;
}

/// Nom d'une langue dans la langue de l'interface, sinon son code.
function languageName(code) {
  try {
    const name = new Intl.DisplayNames([lang], { type: "language" }).of(code);
    return name && name !== code ? t("{0} ({1})", name, code) : code;
  } catch {
    return code;
  }
}

function toolText(tool) {
  if (!tool?.version) return t("absent");
  return tool.origin === "system"
    ? t("{0}, celui du système", tool.version)
    : t("{0}, téléchargé", tool.version);
}

/// État des outils, dans les Options et dans la boîte « Vidéo ».
function renderTools() {
  const tools = store.tools;
  const ready = tools?.ready === true;
  const installing = tools?.installing === true;
  $("m-ytdlp").textContent = toolText(tools?.ytdlp);
  $("m-ffmpeg").textContent = toolText(tools?.ffmpeg);
  $("m-state").textContent = installing
    ? t("Installation en cours…")
    : tools?.error
      ? t("Dernière installation en échec : {0}", tools.error)
      : ready ? t("Outils prêts.") : t("Outils absents : les vidéos ne peuvent pas être téléchargées.");
  $("m-state").classList.toggle("warn", Boolean(tools?.error));
  $("m-install").hidden = ready;
  $("m-reinstall").hidden = !ready;
  for (const id of ["m-install", "m-reinstall", "v-tools-install"]) $(id).disabled = !connected || installing;
  $("pref-ytdlp-update").checked = store.settings["media.autoUpdate"] !== false;
  $("pref-ytdlp-update").disabled = !connected;
  // Dans la boîte « Vidéo » : l'offre d'installation, tant que les outils manquent.
  $("v-tools").hidden = ready || tools === null;
  $("v-tools-install").textContent = installing ? t("Installation en cours…") : t("Installer yt-dlp et ffmpeg");
}

async function loadTools() {
  if (!mediaCapable || !connected) return;
  try {
    store.tools = await host.call("media.tools");
  } catch {
    return;
  }
  renderTools();
}

/// Installe les outils (`media.installTools`) : la commande de l'utilisateur vaut accord
/// (SPEC §7.17). La fin s'annonce par `media.tools`.
async function installTools(force) {
  try {
    store.tools = await host.call("media.installTools", { force });
  } catch (e) {
    showError(refused(e));
  }
  renderTools();
}

function openVideo() {
  if (!connected || !mediaCapable || document.querySelector("dialog[open]")) return;
  $("v-dest").textContent = store.settings["downloads.dir"] ?? t("(aucun)");
  $("v-error").hidden = true;
  renderVideo();
  loadTools();
  const dialog = $("video-dialog");
  dialog.returnValue = "";
  dialog.showModal();
  $("v-page").focus();
}

/// Analyse la page (`media.probe`) ; sans outils, propose de les installer.
async function probeVideo() {
  const page = $("v-page").value.trim();
  if (!/^https?:\/\/\S+$/i.test(page)) {
    $("v-status").textContent = t("Donnez l'adresse d'une page web (http ou https).");
    $("v-page").focus();
    return;
  }
  const mine = ++probeSeq;
  videoProbe = null;
  probedPage = "";
  $("v-error").hidden = true;
  $("v-status").textContent = t("Analyse de la page par yt-dlp…");
  renderVideo();
  try {
    const probe = await host.call("media.probe", { pageUrl: page });
    if (mine !== probeSeq) return;
    videoProbe = probe;
    probedPage = page;
    $("v-status").textContent = "";
  } catch (e) {
    if (mine !== probeSeq) return;
    if (e?.code === "TOOLS_MISSING") {
      probeAfterInstall = true;
      $("v-status").textContent = t("yt-dlp et ffmpeg sont nécessaires pour lire cette page.");
      await loadTools();
    } else {
      $("v-status").textContent = t("Page non reconnue : {0}", e?.message ?? String(e));
    }
  }
  renderVideo();
  if (videoProbe) $("v-quality").focus();
}

/// Résultat de l'analyse : titre, qualités, formats, sous-titres.
function renderVideo() {
  const probe = videoProbe;
  $("v-result").hidden = !probe;
  $("v-probe").disabled = !connected;
  const usable = Boolean(probe) && probe.formats.some((f) => !f.drm);
  $("v-start").disabled = !usable || !connected;
  $("v-later").disabled = !usable || !connected;
  if (!probe) return;
  if (!usable) $("v-status").textContent = t("Cette vidéo est protégée (DRM) : AuroDownloader ne la télécharge pas.");
  $("v-name").textContent = probe.title;
  $("v-duration").textContent = duration(probe.durationS, t);
  $("v-duration-row").hidden = probe.durationS == null;
  $("v-site").textContent = probe.extractor ?? "";
  $("v-site-row").hidden = !probe.extractor;

  const choices = qualityChoices(probe);
  const label = (c) => {
    const base = c.key === "best"
      ? t("Meilleure qualité")
      : c.key === "audio" ? t("Son seul") : t("{0}p", c.height);
    const size = sizeText(c.size);
    return size ? t("{0} — {1}", base, size) : base;
  };
  $("v-quality").replaceChildren(...choices.map((c) => {
    const option = new Option(label(c), c.key);
    option.dataset.format = c.format ?? "";
    return option;
  }));

  $("v-formats").tBodies[0].replaceChildren(...probe.formats.map((f) => {
    const row = document.createElement("tr");
    const pick = document.createElement("input");
    pick.type = "radio";
    pick.name = "v-format";
    pick.value = f.formatId;
    pick.disabled = f.drm;
    pick.setAttribute("aria-label", t("Format {0}", f.formatId));
    const cell = (text) => {
      const td = document.createElement("td");
      td.textContent = text;
      return td;
    };
    const first = document.createElement("td");
    first.append(pick);
    const content = { av: t("image et son"), v: t("image"), a: t("son"), "?": "?" }[formatContent(f)];
    const picture = f.width && f.height ? `${f.width}×${f.height}` : f.height ? t("{0}p", f.height) : "";
    row.append(
      first,
      cell(f.formatId),
      cell(f.drm ? t("{0}, protégé (DRM)", content) : content),
      cell(picture),
      cell(sizeText(f.filesize > 0 ? { bytes: f.filesize, approx: f.filesizeApprox } : null)),
      cell(f.protocol),
    );
    if (f.drm) row.classList.add("skipped");
    return row;
  }));

  const subs = probe.subtitles ?? [];
  $("v-subs").replaceChildren(...subs.map((code) => {
    const label = document.createElement("label");
    label.className = "check";
    const box = document.createElement("input");
    box.type = "checkbox";
    box.value = code;
    const text = document.createElement("span");
    text.textContent = languageName(code);
    label.append(box, text);
    return label;
  }));
  $("v-subs-none").hidden = subs.length > 0;
}

/// Un format précis choisi dans le détail : la liste des qualités le dit.
function chooseExactFormat(formatId) {
  const format = videoProbe?.formats.find((f) => f.formatId === formatId);
  if (!format) return;
  const select = $("v-quality");
  select.querySelector('option[value="exact"]')?.remove();
  const option = new Option(t("Format choisi : {0}", formatId), "exact");
  option.dataset.format = exactFormat(format, videoProbe);
  select.append(option);
  select.value = "exact";
}

/// Ajoute la vidéo analysée (`task.add` avec `media`) ; la boîte ne se ferme qu'en cas de
/// succès.
async function addVideo(choice) {
  const page = $("v-page").value.trim();
  if (!videoProbe || page !== probedPage) {
    // L'adresse a changé depuis l'analyse : on analyse de nouveau d'abord.
    await probeVideo();
    return;
  }
  const media = { pageUrl: page, title: videoProbe.title };
  const format = $("v-quality").selectedOptions[0]?.dataset.format;
  if (format) media.format = format;
  const container = $("v-container").value;
  if (container) media.container = container;
  const subtitles = [...$("v-subs").querySelectorAll("input:checked")].map((b) => b.value);
  if (subtitles.length) media.subtitles = subtitles;
  const sub = subDirParam("v-subdir");
  if (sub === null) {
    $("v-error").textContent = t("Sous-dossier refusé : « . » et « .. » ne sont pas permis.");
    $("v-error").hidden = false;
    return;
  }
  $("v-start").disabled = true;
  $("v-later").disabled = true;
  try {
    await host.call("task.add", { media, startPaused: choice === "later", ...sub });
    $("video-dialog").close("added");
    if (view.startsWith("completed")) chooseView("all");
    announce(t("Vidéo « {0} » ajoutée", videoProbe.title));
  } catch (e) {
    $("v-error").textContent = refused(e);
    $("v-error").hidden = false;
    renderVideo();
  }
}

function closeVideo() {
  probeSeq++;
  probeAfterInstall = false;
  $("tb-video").focus();
}

/// Réinitialise la boîte pour la prochaine vidéo, une fois celle-ci ajoutée.
function resetVideo() {
  videoProbe = null;
  probedPage = "";
  $("v-page").value = "";
  $("v-status").textContent = "";
  $("v-container").value = "";
  $("v-details").open = false;
}

/// Avertissements du moteur : sous-titres manquants et échec d'une mise à jour de yt-dlp
/// montrés ; une mise à jour réussie seulement annoncée.
function showWarnings(warnings) {
  for (const w of warnings) {
    if (w.code === "YTDLP_UPDATED") announce(w.message);
    else {
      showError(w.message);
      announce(w.message);
    }
  }
}

// ---------- Analyse de liens (F-29, F-31, SPEC §7.18, étape 2b) ----------

/// Le moteur sait analyser des liens (capacité `linkscan`).
let linkscanCapable = false;
/// Liens de la dernière analyse, tels que le moteur les rend.
let scanLinks = [];
/// Tailles lues : adresse → octets, `null` si inconnue, `undefined` pendant la lecture.
const linkSizes = new Map();
/// Adresses décochées ; les autres liens montrés sont cochés.
const linkUnchecked = new Set();
/// Numéro de la dernière analyse : une réponse plus ancienne est écartée.
let scanSeq = 0;
/// Tailles demandées pour l'analyse en cours (500 au plus, décision de l'utilisateur).
let sizedCount = 0;
let probing = false;
const MAX_SIZED = 500;
const PROBE_CHUNK = 50;
const BATCH_CHUNK = 1000;
/// Lignes affichées au plus : la liste reste rapide, l'ajout prend tous les liens cochés.
const MAX_ROWS = 1000;

function openLinks() {
  if (!connected || !linkscanCapable || document.querySelector("dialog[open]")) return;
  $("l-dest").textContent = store.settings["downloads.dir"] ?? t("(aucun)");
  $("l-import").hidden = !host.readList;
  showLinksError("");
  renderLinkList();
  const dialog = $("links-dialog");
  dialog.returnValue = "";
  dialog.showModal();
  $("l-source").focus();
}

function showLinksError(text) {
  delete $("l-error").dataset.kind;
  $("l-error").textContent = text;
  $("l-error").hidden = !text;
}

/// Analyse d'une page ou d'un texte (`linkscan.run`) ; `label` dit d'où viennent les liens.
async function scanLinksOf(request, label) {
  const seq = ++scanSeq;
  showLinksError("");
  $("l-status").textContent = t("Analyse en cours…");
  $("l-scan").disabled = true;
  try {
    const result = await host.call("linkscan.run", request);
    if (seq !== scanSeq) return;
    scanLinks = result.links;
    linkSizes.clear();
    linkUnchecked.clear();
    sizedCount = 0;
    $("l-status").textContent = t("{0} lien(s) trouvé(s) dans {1}", scanLinks.length, result.pageUrl ?? label);
    renderLinkList();
    probeSizes();
  } catch (e) {
    if (seq !== scanSeq) return;
    $("l-status").textContent = "";
    showLinksError(refused(e));
  } finally {
    if (seq === scanSeq) $("l-scan").disabled = !connected;
  }
}

function scanSource() {
  const request = scanRequest($("l-source").value);
  if (!request) {
    showLinksError(t("Collez l'adresse d'une page web, ou un texte qui contient des adresses."));
    $("l-source").focus();
    return;
  }
  scanLinksOf(request, request.pageUrl ?? t("le texte collé"));
}

/// Liste importée (F-31) : un fichier texte ou CSV lu par l'hôte, analysé comme un texte.
async function importLinks() {
  const path = await host.pickList().catch(() => null);
  if (path) {
    try {
      const file = await host.readList(path);
      await scanLinksOf({ text: file.text }, file.name);
    } catch (e) {
      showLinksError(refused(e));
    }
  }
  $("l-import").focus();
}

/// Filtres des champs de la boîte ; `sizes` faux : sans ceux de taille.
function linkFields(sizes = true, all = $("l-all").checked) {
  return linkFilter({
    extensions: $("l-ext").value,
    min: sizes ? $("l-min").value : "",
    max: sizes ? $("l-max").value : "",
    regex: $("l-regex").value,
    all,
  });
}

function filterError(filter) {
  switch (filter.error) {
    case "min": return t("Taille minimale illisible (exemples : 500K, 10M, 1G).");
    case "max": return t("Taille maximale illisible (exemples : 500K, 10M, 1G).");
    default: return t("Expression régulière illisible : {0}", filter.message ?? "");
  }
}

/// Tailles des liens montrés (hors filtre de taille), par lots de 50, 500 au plus par analyse ;
/// les liens magnet n'en ont pas.
async function probeSizes() {
  if (probing) return;
  probing = true;
  const seq = scanSeq;
  try {
    while (seq === scanSeq && sizedCount < MAX_SIZED && connected && $("links-dialog").open) {
      const filter = linkFields(false);
      if (filter.error) break;
      // Les liens montrés d'abord ; puis les « pages » masquées, que le serveur dira peut-être
      // des fichiers (sans extension : SHA256SUMS, /download?id=…).
      const wide = linkFields(false, true);
      const waiting = scanLinks.filter((l) => l.kind !== "magnet" && !linkSizes.has(l.url));
      const batch = [
        ...waiting.filter((l) => filter.keep(l)),
        ...waiting.filter((l) => !filter.keep(l) && l.kind === "page" && wide.keep(l)),
      ].slice(0, Math.min(PROBE_CHUNK, MAX_SIZED - sizedCount));
      if (!batch.length) break;
      sizedCount += batch.length;
      for (const l of batch) linkSizes.set(l.url, undefined);
      renderLinkList();
      let result;
      try {
        result = await host.call("linkscan.probe", { urls: batch.map((l) => l.url) });
      } catch {
        for (const l of batch) linkSizes.set(l.url, null);
        break;
      }
      if (seq !== scanSeq) return;
      const kinds = new Map();
      for (const item of result.items) {
        linkSizes.set(item.url, item.size ?? null);
        if (item.kind) kinds.set(item.url, item.kind);
      }
      scanLinks = scanLinks.map((l) => reclassifyLink(l, kinds.get(l.url)));
      renderLinkList();
    }
  } finally {
    probing = false;
    // Une nouvelle analyse est arrivée pendant la lecture : ses tailles à leur tour.
    if (seq !== scanSeq && $("links-dialog").open) probeSizes();
  }
}

function linkSizeText(url) {
  if (!linkSizes.has(url)) return "";
  const size = linkSizes.get(url);
  return size === undefined ? "…" : size === null ? "?" : bytes(size, lang, t);
}

/// Liens montrés et cochés, d'après les filtres ; `null` si un filtre est illisible.
function chosenLinks() {
  const filter = linkFields();
  if (filter.error) return null;
  const shown = scanLinks.filter((l) => filter.keep(l, linkSizes.get(l.url)));
  return { shown, chosen: shown.filter((l) => !linkUnchecked.has(l.url)) };
}

function renderLinkCount() {
  const lists = chosenLinks();
  const chosen = lists?.chosen.length ?? 0;
  $("l-count").textContent = lists
    ? t("{0} coché(s) sur {1} montré(s), {2} trouvé(s)", chosen, lists.shown.length, scanLinks.length)
    : "";
  $("l-start").disabled = !chosen || !connected;
  $("l-later").disabled = !chosen || !connected;
}

function renderLinkList() {
  $("l-result").hidden = !scanLinks.length;
  const lists = chosenLinks();
  if (!lists) {
    showLinksError(filterError(linkFields()));
    renderLinkCount();
    return;
  }
  if ($("l-error").dataset.kind !== "add") showLinksError("");
  const rows = lists.shown.slice(0, MAX_ROWS).map((l) => {
    const row = document.createElement("tr");
    const tick = document.createElement("input");
    tick.type = "checkbox";
    tick.dataset.url = l.url;
    tick.checked = !linkUnchecked.has(l.url);
    tick.setAttribute("aria-label", t("Télécharger {0}", l.name));
    const check = document.createElement("td");
    check.append(tick);
    row.append(check);
    for (const [text, path] of [[l.name, true], [linkSizeText(l.url), false], [l.url, true]]) {
      const cell = document.createElement("td");
      cell.textContent = text;
      if (path) cell.className = "path";
      row.append(cell);
    }
    return row;
  });
  $("l-links").tBodies[0].replaceChildren(...rows);
  $("l-more").hidden = lists.shown.length <= MAX_ROWS;
  $("l-more").textContent = t("Seuls les {0} premiers liens sont affichés ; tous ceux qui sont cochés seront ajoutés.", MAX_ROWS);
  renderLinkCount();
}

function checkShownLinks(on) {
  const lists = chosenLinks();
  if (!lists) return;
  for (const l of lists.shown) {
    if (on) linkUnchecked.delete(l.url);
    else linkUnchecked.add(l.url);
  }
  renderLinkList();
}

/// Ajoute les liens cochés (`task.addBatch`, 1 000 par appel). Les refusés restent dans la
/// boîte, seuls cochés ; la boîte se ferme si tout est ajouté.
async function addLinks(choice) {
  const lists = chosenLinks();
  if (!lists?.chosen.length) return;
  const sub = subDirParam("l-subdir");
  if (sub === null) {
    showLinksError(t("Sous-dossier refusé : « . » et « .. » ne sont pas permis."));
    return;
  }
  $("l-start").disabled = true;
  $("l-later").disabled = true;
  const chosen = lists.chosen;
  const refusedLinks = [];
  let firstError = null;
  let added = 0;
  for (let i = 0; i < chosen.length; i += BATCH_CHUNK) {
    const part = chosen.slice(i, i + BATCH_CHUNK);
    try {
      const items = part.map((l) => ({ sources: [{ url: l.url }], startPaused: choice === "later", ...sub }));
      const result = await host.call("task.addBatch", { items });
      added += result.tasks.length;
      for (const e of result.errors) {
        refusedLinks.push(part[e.index]);
        firstError ??= e;
      }
    } catch (e) {
      refusedLinks.push(...part);
      firstError ??= e;
    }
  }
  if (view.startsWith("completed") && added) chooseView("all");
  if (!refusedLinks.length) {
    $("links-dialog").close("added");
    announce(t("{0} téléchargement(s) ajouté(s)", added));
    return;
  }
  const done = new Set(chosen.map((l) => l.url));
  for (const l of refusedLinks) done.delete(l.url);
  scanLinks = scanLinks.filter((l) => !done.has(l.url));
  const refusedUrls = new Set(refusedLinks.map((l) => l.url));
  linkUnchecked.clear();
  for (const l of scanLinks) if (!refusedUrls.has(l.url)) linkUnchecked.add(l.url);
  showLinksError(t("{0} ajouté(s), {1} refusé(s) : {2}", added, refusedLinks.length, firstError?.message ?? ""));
  $("l-error").dataset.kind = "add";
  renderLinkList();
}

function closeLinks() {
  scanSeq++;
  if ($("links-dialog").returnValue === "added") {
    scanLinks = [];
    linkSizes.clear();
    linkUnchecked.clear();
    $("l-source").value = "";
    $("l-status").textContent = "";
  }
  delete $("l-error").dataset.kind;
  $("tb-links").focus();
}

// ---------- Historique (F-35) ----------

const HISTORY_PAGE = 200;
/// Entrées affichées, curseur de la page suivante, et numéro de la dernière lecture : une
/// réponse arrivée après une autre recherche est écartée.
let historyItems = [];
let historyNext = null;
let historySeq = 0;
let historyTimer = null;

function openHistory() {
  if (!connected || document.querySelector("dialog[open]")) return;
  $("h-query").value = "";
  historyItems = [];
  historyNext = null;
  renderHistory();
  const dialog = $("history-dialog");
  dialog.returnValue = "";
  dialog.showModal();
  $("h-query").focus();
  loadHistory(true);
}

/// Lit une page de l'historique : la première pour la recherche en cours (`fresh`), ou la
/// suivante.
async function loadHistory(fresh) {
  const seq = ++historySeq;
  const query = $("h-query").value.trim();
  const params = { limit: HISTORY_PAGE };
  if (query) params.query = query;
  if (!fresh && historyNext) params.cursor = historyNext;
  try {
    const page = await host.call("history.list", params);
    if (seq !== historySeq) return;
    historyItems = fresh ? page.items : historyItems.concat(page.items);
    historyNext = page.nextCursor ?? null;
    renderHistory();
  } catch (e) {
    showError(refused(e));
  }
}

function renderHistory() {
  const body = $("h-table").tBodies[0];
  body.replaceChildren(...historyItems.map((h) => {
    const row = document.createElement("tr");
    row.className = h.finalState;
    const cell = (className, text, title) => {
      const td = document.createElement("td");
      td.className = className;
      td.textContent = text;
      if (title) td.title = title;
      row.append(td);
    };
    cell("when", dateText(h.finishedAt));
    cell("name", h.displayName, h.finalPath ?? "");
    cell("size", h.totalBytes != null ? bytes(h.totalBytes, lang, t) : "");
    cell("state", stateText({ state: h.finalState }, t));
    cell("host", h.sourceHost ?? "");
    return row;
  }));
  const n = historyItems.length;
  $("h-status").textContent = n === 0
    ? t("Aucune entrée")
    : historyNext ? t("{0} entrées affichées, d'autres suivent", n) : t("{0} entrée(s)", n);
  $("h-more").hidden = !historyNext;
  for (const id of ["h-more", "h-export", "h-clear"]) $(id).disabled = !connected;
  $("h-export").disabled ||= n === 0;
}

/// Toutes les entrées de la recherche en cours, en CSV, dans un fichier choisi par
/// l'utilisateur.
async function exportHistory() {
  const query = $("h-query").value.trim();
  const items = [];
  let cursor = null;
  try {
    do {
      const params = { limit: 1000 };
      if (query) params.query = query;
      if (cursor) params.cursor = cursor;
      const page = await host.call("history.list", params);
      items.push(...page.items);
      cursor = page.nextCursor ?? null;
    } while (cursor);
    const csv = historyCsv(items, {
      headers: [t("Fini le"), t("État"), t("Taille (octets)"), t("Nom"), t("Site"), t("Chemin"), "SHA-256"],
      stateLabel: (state) => stateText({ state }, t),
      sep: lang === "fr" ? ";" : ",",
    });
    const day = localStamp(Date.now()).slice(0, 10);
    const saved = await host.saveText(t("historique-{0}.csv", day), csv);
    if (saved) announce(t("Historique exporté : {0}", saved));
  } catch (e) {
    showError(refused(e));
  }
}

function openClearHistory() {
  if (!connected) return;
  const dialog = $("history-clear-dialog");
  dialog.returnValue = "";
  dialog.showModal();
}

async function closeClearHistory() {
  if ($("history-clear-dialog").returnValue !== "confirm") {
    $("h-clear").focus();
    return;
  }
  try {
    const { removed } = await host.call("history.clear", {});
    announce(t("{0} entrée(s) effacée(s)", removed));
  } catch (e) {
    showError(refused(e));
  }
  await loadHistory(true);
  $("h-query").focus();
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

// ---------- BitTorrent (SPEC §7.16) ----------

/// Ratios proposés ; `null` : sans limite, jusqu'à l'arrêt du partage.
const RATIOS = [0, 0.5, 1, 2, 3, null];
/// Interfaces réseau de la machine (`torrent.interfaces`), lues à l'ouverture de la rubrique.
let interfaces = [];

function ratioOption(value) {
  const option = document.createElement("option");
  option.value = value === null ? "none" : String(value);
  option.textContent = value === null
    ? t("Sans limite, jusqu'à ce que je l'arrête")
    : value === 0
      ? t("Ne pas partager")
      : value === 1
        ? t("Jusqu'à avoir envoyé autant que reçu (ratio 1)")
        : t("Jusqu'au ratio {0}", value.toLocaleString(lang));
  return option;
}

/// Réglages des torrents : suivis par l'instantané, `settings.updated`, `limits.updated` et
/// `engine.torrentInterface`.
function renderTorrentSettings() {
  const ratio = store.settings["torrent.ratioLimit"];
  const current = ratio === undefined ? 1 : ratio;
  const values = RATIOS.includes(current) ? RATIOS : [...RATIOS, current].sort((a, b) =>
    (a ?? Infinity) - (b ?? Infinity));
  $("pref-ratio").replaceChildren(...values.map(ratioOption));
  $("pref-ratio").value = current === null ? "none" : String(current);
  fillLimits($("pref-upload"), store.limits?.uploadBps ?? null);
  $("pref-upnp").checked = store.settings["torrent.upnp"] === true;
  const chosen = store.settings["torrent.interface"] ?? "";
  const names = [...interfaces];
  if (chosen && !names.includes(chosen)) names.push(chosen);
  const select = $("pref-interface");
  select.replaceChildren(
    new Option(t("Aucune : toutes les interfaces"), ""),
    ...names.map((name) => new Option(name, name)),
  );
  select.value = chosen;
  $("pref-interface-state").textContent = !chosen
    ? ""
    : store.torrentInterfaceUp
      ? t("{0} est là : les torrents ne passent que par elle.", chosen)
      : t("{0} est absente : les torrents attendent son retour.", chosen);
  $("pref-interface-state").classList.toggle("warn", Boolean(chosen) && !store.torrentInterfaceUp);
  for (const id of ["pref-ratio", "pref-upload", "pref-upnp", "pref-interface", "pref-interface-refresh"]) {
    $(id).disabled = !connected;
  }
}

async function loadInterfaces() {
  try {
    interfaces = (await host.call("torrent.interfaces")).items ?? [];
  } catch {
    interfaces = [];
  }
  renderTorrentSettings();
}

/// Change un réglage du moteur ; le moteur l'annonce, la réponse le donne aussitôt.
async function saveSetting(key, value, done) {
  try {
    const result = await host.call("settings.set", { values: { [key]: value } });
    Object.assign(store.settings, result.values);
    if (done) announce(done);
  } catch (e) {
    showError(refused(e));
  }
  renderTorrentSettings();
}

async function changeUploadLimit(event) {
  const value = Number(event.target.value);
  try {
    store.limits = await host.call("limits.set", { uploadBps: value > 0 ? value : null });
  } catch (e) {
    showError(refused(e));
  }
  renderTorrentSettings();
}

/// Association aux liens magnet et aux `.torrent` (option de l'application, désactivée par
/// défaut) ; absente d'un hôte qui ne la permet pas.
async function renderLinks() {
  const box = $("pref-links-box");
  if (!host.linksStatus) {
    box.hidden = true;
    return;
  }
  try {
    const status = await host.linksStatus();
    box.hidden = !status.available;
    $("pref-links").checked = status.enabled;
  } catch {
    box.hidden = true;
  }
}

async function changeLinks(event) {
  const wanted = event.target.checked;
  try {
    const status = await host.setLinks(wanted);
    event.target.checked = status.enabled;
    announce(status.enabled
      ? t("AuroDownloader ouvre les liens magnet et les fichiers .torrent")
      : t("Liens magnet et fichiers .torrent rendus au programme d'avant"));
  } catch (e) {
    event.target.checked = !wanted;
    showError(refused(e));
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
    option.textContent = login.keyPath
      ? t("{0} — {1} (clé privée)", login.site, login.username)
      : `${login.site} — ${login.username}`;
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
  $("login-key").value = login?.keyPath ?? "";
  $("login-plain").checked = login?.allowPlain === true;
  // Le choix d'un fichier : seulement avec un hôte qui sait le faire (bureau).
  $("login-key-pick").hidden = typeof host.pickKey !== "function";
  // Nouveaux identifiants : un mot de passe (une clé peut s'en passer) ; sinon, vide garde
  // celui enregistré.
  $("login-password").required = !login && !$("login-key").value.trim();
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
  // Clé privée (SFTP) : un chemin, ou vide pour la retirer ; « sans chiffrement » (FTP).
  const key = $("login-key").value.trim();
  const editing = logins.find((l) => l.loginId === editingLogin);
  if (key || editing?.keyPath) params.keyPath = key;
  params.allowPlain = $("login-plain").checked;
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
  let error = null;
  try {
    await host.call("login.delete", { loginId: login.loginId });
  } catch (e) {
    error = e;
  }
  // Liste relue, focus placé, puis le message : ni effacé par la relecture, ni coupé par
  // l'annonce du focus.
  await loadLogins();
  (logins.length ? $("logins") : $("login-new")).focus();
  if (error) {
    $("logins-error").hidden = false;
    speak($("logins-error"), refused(error));
  } else {
    announce(t("Identifiants de {0} supprimés", login.site));
  }
}

// ---------- Clés des serveurs SSH (SFTP, SPEC §7.22, étape 6d) ----------

/// Clés lues pour les Propriétés (`hostkey.list`) et la tâche (avec sa révision d'état) pour
/// laquelle : relues quand la tâche s'arrête de nouveau.
let detailsKeys = null;
let detailsKeysFor = null;
/// Clé en attente proposée dans la boîte « Accepter la clé ».
let offeredKey = null;

/// Section « Clé du serveur SSH » des Propriétés, pour une tâche arrêtée par une clé inconnue
/// ou changée : l'empreinte vue par le moteur, et « Accepter la clé… ». Le moteur ne garde
/// la clé en attente qu'en mémoire : après son redémarrage, relancer la tâche la remontre.
function renderHostKey(task) {
  const section = $("d-hostkey");
  const url = detailsFull?.sources?.[0]?.url;
  section.hidden = !blockedByHostKey(task) || !url;
  if (section.hidden) return;
  const wanted = `${task.taskId}:${task.stateRev ?? ""}`;
  if (caps.admin && connected && detailsKeysFor !== wanted) {
    detailsKeysFor = wanted;
    host.call("hostkey.list").then((keys) => {
      detailsKeys = keys;
      if (detailsTask === task.taskId) renderHostKey(task);
    }).catch(() => {});
  }
  const key = pendingKeyFor(detailsKeys, url);
  const changed = task.errorCode === "HOST_KEY_CHANGED";
  $("d-hostkey-text").textContent = !key
    ? t("La clé n'est plus en attente (le moteur a redémarré) : relancer la tâche pour la revoir.")
    : changed
      ? t("La clé de ce serveur a changé depuis que vous l'avez acceptée. Quelqu'un pourrait se faire passer pour lui.")
      : t("Ce serveur n'est pas encore connu. Vérifiez son empreinte auprès de son administrateur avant de l'accepter.");
  $("d-hostkey-text").classList.toggle("warn", changed);
  $("d-hostkey-fingerprint").textContent = key ? `${key.algorithm} ${key.fingerprint}` : "";
  $("d-hostkey-fingerprint").hidden = !key;
  $("d-hostkey-accept").hidden = !key;
  $("d-hostkey-accept").disabled = !connected;
}

/// Boîte de confirmation : serveur, empreinte, et l'avertissement d'une clé changée.
function offerHostKey() {
  const task = store.tasks.get(detailsTask);
  const key = pendingKeyFor(detailsKeys, detailsFull?.sources?.[0]?.url);
  if (!task || !key) return;
  offeredKey = key;
  const server = key.host.includes(":") ? `[${key.host}]:${key.port}` : `${key.host}:${key.port}`;
  $("hostkey-text").textContent = t(
    "Serveur {0}. Les téléchargements SFTP de ce serveur ne se feront qu'avec cette clé ; ceux qu'elle bloquait repartiront.",
    server,
  );
  $("hostkey-fingerprint").textContent = `${key.algorithm} ${key.fingerprint}`;
  $("hostkey-warning").hidden = !key.changed;
  $("hostkey-error").hidden = true;
  const dialog = $("hostkey-dialog");
  dialog.returnValue = "";
  dialog.showModal();
  // Le choix sûr d'abord : Annuler.
  dialog.querySelector('button[value="cancel"]').focus();
}

async function acceptHostKey(event) {
  event.preventDefault();
  const key = offeredKey;
  if (!key) return;
  $("hostkey-confirm").disabled = true;
  try {
    const done = await host.call("hostkey.accept", {
      host: key.host,
      port: key.port,
      fingerprint: key.fingerprint,
    });
    $("hostkey-dialog").close();
    detailsKeysFor = null;
    const n = done.resumed?.length ?? 0;
    announce(n === 1
      ? t("Clé acceptée ; 1 téléchargement relancé")
      : t("Clé acceptée ; {0} téléchargements relancés", n));
    loadHostKeys();
  } catch (e) {
    $("hostkey-error").textContent = refused(e);
    $("hostkey-error").hidden = false;
  } finally {
    $("hostkey-confirm").disabled = false;
  }
}

/// Clés acceptées, dans la rubrique « Identifiants de sites » des Options.
let hostKeys = [];

async function loadHostKeys() {
  $("hostkeys-error").hidden = true;
  try {
    hostKeys = (await host.call("hostkey.list")).items ?? [];
  } catch (e) {
    hostKeys = [];
    $("hostkeys-error").textContent = refused(e);
    $("hostkeys-error").hidden = false;
  }
  const list = $("hostkeys");
  const keep = list.value;
  list.replaceChildren(...hostKeys.map((key, i) => {
    const option = document.createElement("option");
    option.value = String(i);
    const server = key.host.includes(":") ? `[${key.host}]:${key.port}` : `${key.host}:${key.port}`;
    option.textContent = `${server} — ${key.algorithm} ${key.fingerprint}`;
    // L'empreinte dépasse souvent la largeur de la liste : entière au survol.
    option.title = option.textContent;
    return option;
  }));
  if (keep && Number(keep) < hostKeys.length) list.value = keep;
  list.hidden = !hostKeys.length;
  $("hostkeys-empty").hidden = hostKeys.length > 0;
  syncHostKeyButtons();
}

function syncHostKeyButtons() {
  $("hostkey-forget").disabled = !connected || !$("hostkeys").value;
}

async function forgetHostKey() {
  const key = hostKeys[Number($("hostkeys").value)];
  if (!key) return;
  let error = null;
  try {
    await host.call("hostkey.remove", { host: key.host, port: key.port });
  } catch (e) {
    error = e;
  }
  // Comme `deleteLogin` : liste relue, focus placé, puis le message.
  await loadHostKeys();
  (hostKeys.length ? $("hostkeys") : $("login-new")).focus();
  if (error) {
    $("hostkeys-error").hidden = false;
    speak($("hostkeys-error"), refused(error));
  } else {
    announce(t("Clé de {0} oubliée", key.host));
  }
}

// ---------- Règles de rangement (F-32, F-34, SPEC §7.21, étape 4c) ----------

/// Règles du moteur (`rules.list`), dans leur ordre. Une connexion à jeton les lit sans
/// leurs commandes et ne les écrit pas : ses boutons sont cachés (`data-needs="admin"`).
let rules = [];
/// Règle en cours de modification ; `null` : nouvelle.
let editingRule = null;

async function loadRules(select) {
  $("rules-error").hidden = true;
  try {
    rules = (await host.call("rules.list")).rules ?? [];
  } catch (e) {
    rules = [];
    $("rules-error").textContent = refused(e);
    $("rules-error").hidden = false;
  }
  renderRules(select);
}

/// Une ligne du résumé « si… alors… » d'une règle.
function ruleText(rule) {
  const { when, then } = ruleSummary(rule);
  const kind = (k) => t(CATEGORY_LABELS[k] ?? k);
  const parts = {
    // Une extension se lit avec son point : « type Archives, .bin ».
    type: (v) => t("type {0}", v.map((x) => (CATEGORY_LABELS[x] ? kind(x) : `.${x}`)).join(", ")),
    domain: (v) => t("site {0}", v.join(", ")),
    url: (v) => t("adresse ~ {0}", v),
    name: (v) => t("nom ~ {0}", v),
    folder: (v) => t("dossier {0}", v),
    rename: (v) => t("nom {0}", v),
    command: (v) => t("commande {0}", v),
    hasCommand: () => t("une commande de fin"),
  };
  const say = (list) => list.map(([k, v]) => parts[k](v)).join(" ; ");
  const ifPart = when.length ? say(when) : t("toute tâche");
  const thenPart = then.length ? say(then) : t("rien");
  return t("Si {0}, alors {1}.", ifPart, thenPart);
}

function renderRules(select) {
  const list = $("rules");
  const keep = select ?? list.value;
  list.replaceChildren(...rules.map((rule, i) => {
    const option = document.createElement("option");
    option.value = rule.ruleId;
    const name = rule.name || t("(sans nom)");
    option.textContent = rule.enabled
      ? `${i + 1}. ${name}`
      : t("{0}. {1} (désactivée)", i + 1, name);
    return option;
  }));
  if (rules.some((r) => r.ruleId === keep)) list.value = keep;
  else if (rules.length) list.selectedIndex = 0;
  list.hidden = !rules.length;
  $("rules-empty").hidden = rules.length > 0;
  syncRuleButtons();
}

function chosenRuleIndex() {
  return rules.findIndex((r) => r.ruleId === $("rules").value);
}

function syncRuleButtons() {
  const i = chosenRuleIndex();
  const rule = rules[i];
  $("rule-summary").textContent = rule ? ruleText(rule) : "";
  $("rule-new").disabled = !connected;
  for (const id of ["rule-edit", "rule-delete", "rule-toggle"]) $(id).disabled = !connected || !rule;
  $("rule-up").disabled = !connected || i <= 0;
  $("rule-down").disabled = !connected || i < 0 || i >= rules.length - 1;
  $("rule-toggle").textContent = rule && !rule.enabled ? t("Activer") : t("Désactiver");
}

/// Enregistre toute la liste (`rules.set`), puis la relit ; `select` : la règle à garder
/// choisie (ou le rang d'une règle nouvelle).
async function saveRules(list, select) {
  const saved = (await host.call("rules.set", { rules: list.map(ruleForSet) })).rules ?? [];
  rules = saved;
  const id = typeof select === "number" ? saved[select]?.ruleId : select;
  renderRules(id);
}

function openRule(rule) {
  editingRule = rule?.ruleId ?? null;
  const m = rule?.match ?? {};
  const a = rule?.actions ?? {};
  $("rule-name").value = rule?.name ?? "";
  $("rule-enabled").checked = rule?.enabled ?? true;
  // Refaites à chaque ouverture : la langue a pu changer.
  renderRuleCategories();
  for (const box of $("rule-categories").querySelectorAll("input")) {
    box.checked = (m.categories ?? []).includes(box.value);
  }
  $("rule-extensions").value = (m.extensions ?? []).join(", ");
  $("rule-domains").value = (m.domains ?? []).join(", ");
  $("rule-url-regex").value = m.urlRegex ?? "";
  $("rule-name-regex").value = m.nameRegex ?? "";
  $("rule-folder").value = a.folder ?? "";
  $("rule-rename").value = a.rename ?? "";
  $("rule-program").value = a.command?.program ?? "";
  $("rule-args").value = (a.command?.args ?? []).join("\n");
  $("rule-timeout").value = a.command?.timeoutS ?? "";
  $("rule-error").hidden = true;
  const dialog = $("rule-dialog");
  dialog.returnValue = "";
  dialog.showModal();
  $("rule-name").focus();
}

async function saveRule() {
  const fields = {
    ruleId: editingRule,
    name: $("rule-name").value,
    enabled: $("rule-enabled").checked,
    categories: [...$("rule-categories").querySelectorAll("input:checked")].map((b) => b.value),
    extensions: $("rule-extensions").value,
    domains: $("rule-domains").value,
    urlRegex: $("rule-url-regex").value,
    nameRegex: $("rule-name-regex").value,
    folder: $("rule-folder").value,
    rename: $("rule-rename").value,
    program: $("rule-program").value,
    args: $("rule-args").value,
    timeout: $("rule-timeout").value,
  };
  const rule = ruleFromFields(fields);
  const at = rules.findIndex((r) => r.ruleId === editingRule);
  const list = at >= 0 ? rules.map((r, i) => (i === at ? rule : r)) : [...rules, rule];
  $("rule-save").disabled = true;
  try {
    await saveRules(list, at >= 0 ? editingRule : list.length - 1);
    $("rule-dialog").close();
    announce(t("Règle enregistrée"));
    $("rules").focus();
  } catch (e) {
    // Le moteur dit quel champ il refuse (`rules[0] : actions.folder : …`) : le message
    // sans ce chemin, et le curseur dans le champ.
    const { field, text } = ruleError(e?.message ?? String(e));
    if (field) {
      fieldError($(field), text, $("rule-error"));
      $(field).focus();
    } else {
      clearFieldErrors($("rule-dialog"));
      $("rule-error").hidden = false;
      speak($("rule-error"), text);
    }
  } finally {
    $("rule-save").disabled = false;
  }
}

/// Change la liste (ordre, activation, retrait) et l'enregistre ; un refus la relit.
async function changeRules(list, select, message) {
  $("rules-error").hidden = true;
  try {
    await saveRules(list, select);
    if (message) announce(message);
  } catch (e) {
    $("rules-error").textContent = refused(e);
    $("rules-error").hidden = false;
    await loadRules();
  }
}

async function moveChosenRule(delta) {
  const i = chosenRuleIndex();
  if (i < 0) return;
  const id = rules[i].ruleId;
  await changeRules(moveRule(rules, i, delta), id, t("Règle déplacée au rang {0}", i + delta + 1));
  $("rules").focus();
}

async function toggleChosenRule() {
  const i = chosenRuleIndex();
  if (i < 0) return;
  const on = !rules[i].enabled;
  const list = rules.map((r, j) => (j === i ? { ...r, enabled: on } : r));
  await changeRules(list, rules[i].ruleId, on ? t("Règle activée") : t("Règle désactivée"));
}

async function deleteChosenRule() {
  const i = chosenRuleIndex();
  if (i < 0) return;
  const name = rules[i].name || t("(sans nom)");
  const list = rules.filter((_, j) => j !== i);
  await changeRules(list, list[Math.min(i, list.length - 1)]?.ruleId, t("Règle {0} supprimée", name));
  (rules.length ? $("rules") : $("rule-new")).focus();
}

/// Cases des types de fichier de la boîte d'une règle, dans l'ordre de l'arborescence.
function renderRuleCategories() {
  $("rule-categories").replaceChildren(...CATEGORIES.map((cat) => {
    const label = document.createElement("label");
    label.className = "check";
    const box = document.createElement("input");
    box.type = "checkbox";
    box.value = cat;
    label.append(box, " ", t(CATEGORY_LABELS[cat]));
    return label;
  }));
}

// ---------- Plugins d'hébergeurs (SPEC §7.20, étape 4c) ----------

/// Plugins installés (`plugin.list`), lus à l'ouverture des Options et après chaque changement.
let plugins = [];
/// Fichier lu et vérifié par le moteur (`plugin.inspect`), en attente de l'accord :
/// `{name, data, inspected}`.
let pendingPlugin = null;

async function loadPlugins(select) {
  $("plugins-error").hidden = true;
  try {
    plugins = (await host.call("plugin.list")).plugins ?? [];
  } catch (e) {
    plugins = [];
    $("plugins-error").textContent = refused(e);
    $("plugins-error").hidden = false;
  }
  renderPlugins(select);
}

const pluginLabel = (p) => p.enabled ? t("{0} {1}", p.name, p.version) : t("{0} {1} (désactivé)", p.name, p.version);

function renderPlugins(select) {
  const list = $("plugins");
  const keep = select ?? list.value;
  list.replaceChildren(...plugins.map((p) => {
    const option = document.createElement("option");
    option.value = p.pluginId;
    option.textContent = pluginLabel(p);
    return option;
  }));
  if (plugins.some((p) => p.pluginId === keep)) list.value = keep;
  else if (plugins.length) list.value = plugins[0].pluginId;
  list.hidden = !plugins.length;
  $("plugins-empty").hidden = plugins.length > 0;
  const chosen = plugins.find((p) => p.pluginId === list.value);
  $("plugin-facts").hidden = !chosen;
  if (chosen) {
    $("pf-sites").textContent = chosen.sites.join(", ");
    $("pf-fetch").textContent = chosen.fetch.join(", ");
    $("pf-author").textContent = chosen.author ?? "";
    $("pf-author-row").hidden = !chosen.author;
    $("pf-description").textContent = chosen.description ?? "";
    $("pf-description-row").hidden = !chosen.description;
    $("pf-digest").textContent = chosen.digest;
    $("pf-installed").textContent = dateFormat.format(new Date(chosen.installedAt));
  }
  $("plugin-install").disabled = !connected;
  $("plugin-toggle").disabled = !connected || !chosen;
  $("plugin-remove").disabled = !connected || !chosen;
  $("plugin-toggle").textContent = chosen && !chosen.enabled ? t("Activer") : t("Désactiver");
}

/// Choix d'un fichier `.wasm`, lu par le moteur sans être installé, puis montré pour accord.
async function pickPlugin() {
  $("plugins-error").hidden = true;
  let file;
  try {
    const path = await host.pickPlugin();
    if (!path) return;
    file = await host.readPlugin(path);
  } catch (e) {
    $("plugins-error").textContent = refused(e);
    $("plugins-error").hidden = false;
    return;
  }
  let inspected;
  try {
    inspected = await host.call("plugin.inspect", { data: file.data });
  } catch (e) {
    $("plugins-error").textContent = t("{0} : {1}", file.name, e?.message ?? String(e));
    $("plugins-error").hidden = false;
    return;
  }
  pendingPlugin = { name: file.name, data: file.data, inspected };
  const fill = (id, text) => {
    $(id).textContent = text ?? "";
    const row = $(`${id}-row`);
    if (row) row.hidden = !text;
  };
  fill("pi-file", file.name);
  fill("pi-name", inspected.name);
  fill("pi-id", inspected.pluginId);
  fill("pi-version", inspected.version);
  fill("pi-author", inspected.author);
  fill("pi-description", inspected.description);
  fill("pi-sites", inspected.sites.join(", "));
  fill("pi-fetch", inspected.fetch.join(", "));
  fill("pi-size", bytes(inspected.size, lang, t));
  fill("pi-digest", inspected.digest);
  // Même identifiant : une mise à jour, que l'utilisateur voit comme telle.
  const installed = plugins.find((p) => p.pluginId === inspected.pluginId);
  $("pi-replace").textContent = !installed
    ? ""
    : installed.digest === inspected.digest
      ? t("Ce plugin est déjà installé, identique.")
      : t("Remplace la version {0} installée ; ses permissions sont celles montrées ici.", installed.version);
  $("pi-replace").hidden = !installed;
  $("plugin-error").hidden = true;
  const dialog = $("plugin-dialog");
  dialog.returnValue = "";
  dialog.showModal();
  $("plugin-accept").focus();
}

/// Installe le fichier montré, sur l'empreinte que l'utilisateur vient d'accepter.
async function installPlugin() {
  if (!pendingPlugin) return;
  $("plugin-accept").disabled = true;
  try {
    const row = await host.call("plugin.install", {
      data: pendingPlugin.data,
      digest: pendingPlugin.inspected.digest,
    });
    $("plugin-dialog").close();
    announce(t("Plugin {0} installé", row.name));
    await loadPlugins(row.pluginId);
    $("plugins").focus();
  } catch (e) {
    $("plugin-error").textContent = e?.message ?? String(e);
    $("plugin-error").hidden = false;
  } finally {
    $("plugin-accept").disabled = false;
  }
}

async function togglePlugin() {
  const chosen = plugins.find((p) => p.pluginId === $("plugins").value);
  if (!chosen) return;
  try {
    await host.call("plugin.enable", { pluginId: chosen.pluginId, enabled: !chosen.enabled });
    announce(chosen.enabled ? t("Plugin {0} désactivé", chosen.name) : t("Plugin {0} activé", chosen.name));
  } catch (e) {
    $("plugins-error").textContent = refused(e);
    $("plugins-error").hidden = false;
  }
  await loadPlugins(chosen.pluginId);
  $("plugin-toggle").focus();
}

async function removePlugin() {
  const chosen = plugins.find((p) => p.pluginId === $("plugins").value);
  if (!chosen) return;
  try {
    await host.call("plugin.remove", { pluginId: chosen.pluginId });
    announce(t("Plugin {0} retiré", chosen.name));
  } catch (e) {
    $("plugins-error").textContent = refused(e);
    $("plugins-error").hidden = false;
  }
  await loadPlugins();
  (plugins.length ? $("plugins") : $("plugin-install")).focus();
}

// ---------- Accès à distance (SPEC §7.19, étape 3d) ----------

/// État de l'écoute (`remote.status`) et jetons (`token.list`), lus à l'ouverture de la
/// rubrique et après chaque changement.
let remoteStatus = null;
let remoteTokens = [];
/// Une attente que l'écoute suive les réglages est en cours.
let remoteSettling = false;
/// Jeton de l'appareil qu'on vient d'ajouter, montré une fois puis oublié.
let deviceToken = null;

const SCOPE_LABELS = { read: "Regarder seulement", add: "Ajouter seulement", control: "Tout piloter" };

function remoteFailed(e) {
  $("r-error").textContent = refused(e);
  $("r-error").hidden = false;
}

async function loadRemote() {
  $("r-error").hidden = true;
  try {
    remoteStatus = await host.call("remote.status");
    remoteTokens = (await host.call("token.list")).tokens ?? [];
  } catch (e) {
    remoteFailed(e);
  }
  renderRemote();
}

function remoteStateText(status) {
  if (!status) return "";
  if (!status.enabled) return t("Accès coupé.");
  if (status.listening) return t("Ouvert. Adresses de la page : {0}", (status.urls ?? []).join(", "));
  if (status.error) return t("Activé, mais fermé : {0}", status.error);
  return t("Ouverture…");
}

function renderRemote() {
  const status = remoteStatus;
  const listen = splitListen(store.settings["remote.listen"]) ?? { host: "127.0.0.1", port: 7380 };
  $("r-enabled").checked = store.settings["remote.enabled"] === true;
  const options = [new Option(t("Cet ordinateur seulement ({0})", "127.0.0.1"), "127.0.0.1")];
  for (const choice of status?.addresses ?? []) {
    options.push(new Option(`${choice.interface} — ${choice.address}`, choice.address));
  }
  // Une adresse réglée ailleurs (CLI) ou d'une interface absente reste visible.
  if (!options.some((o) => o.value === listen.host)) {
    const all = listen.host === "0.0.0.0" || listen.host === "::";
    options.push(new Option(all ? t("Toutes les interfaces ({0})", listen.host) : t("{0} (absente)", listen.host), listen.host));
  }
  $("r-address").replaceChildren(...options);
  $("r-address").value = listen.host;
  if (document.activeElement !== $("r-port")) $("r-port").value = String(listen.port);
  const state = $("r-state");
  state.textContent = remoteStateText(status);
  state.classList.toggle("warn", Boolean(status?.enabled && !status.listening && status.error));
  const cert = status?.certificate;
  $("r-cert-box").hidden = !cert;
  if (cert) {
    $("r-cert-kind").textContent = cert.generated ? t("fabriqué par AuroDownloader") : t("fourni");
    $("r-fingerprint").textContent = cert.fingerprint;
    $("r-expires").textContent = longDateFormat.format(new Date(cert.expiresAt));
    $("r-cert-note").hidden = !cert.generated;
  }
  $("r-cert-generated").hidden = !store.settings["remote.tlsCert"];
  const list = $("r-tokens");
  const keep = list.value;
  list.replaceChildren(...remoteTokens.map((token) => {
    const used = token.lastUsedAt
      ? t("utilisé le {0}", dateFormat.format(new Date(token.lastUsedAt)))
      : t("jamais utilisé");
    return new Option(`${token.name} — ${t(SCOPE_LABELS[token.scope] ?? token.scope)} — ${used}`, token.id);
  }));
  if (remoteTokens.some((token) => token.id === keep)) list.value = keep;
  list.hidden = !remoteTokens.length;
  $("r-tokens-empty").hidden = remoteTokens.length > 0;
  for (const id of ["r-enabled", "r-address", "r-port", "r-refresh", "r-cert-provide", "r-cert-generated", "r-token-new"]) {
    $(id).disabled = !connected;
  }
  $("r-token-revoke").disabled = !connected || !list.value;
}

/// Attend que l'écoute suive les réglages enregistrés (au plus 5 s), en montrant l'état.
async function settleRemote() {
  if (remoteSettling) return;
  remoteSettling = true;
  try {
    for (let i = 0; i < 50; i++) {
      remoteStatus = await host.call("remote.status");
      renderRemote();
      if (remoteSettled(remoteStatus, store.settings)) break;
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
  } catch (e) {
    remoteFailed(e);
  } finally {
    remoteSettling = false;
  }
}

async function changeRemote(values, done) {
  $("r-error").hidden = true;
  try {
    const result = await host.call("settings.set", { values });
    Object.assign(store.settings, result.values);
    if (done) announce(done);
  } catch (e) {
    remoteFailed(e);
  }
  renderRemote();
  await settleRemote();
}

function changeListen() {
  const port = Number($("r-port").value);
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    $("r-port").reportValidity();
    return;
  }
  const listen = joinListen($("r-address").value, port);
  if (listen !== store.settings["remote.listen"]) changeRemote({ "remote.listen": listen });
}

function openCertDialog() {
  $("cert-file").value = store.settings["remote.tlsCert"] ?? "";
  $("cert-key").value = store.settings["remote.tlsKey"] ?? "";
  $("cert-error").hidden = true;
  const dialog = $("cert-dialog");
  dialog.returnValue = "";
  dialog.showModal();
  $("cert-file").focus();
}

/// Fichier de clé privée SSH des identifiants (SFTP) : choisi par l'hôte, le moteur le relit.
async function pickKey() {
  const path = await host.pickKey?.().catch(() => null);
  if (path) {
    $("login-key").value = path;
    $("login-password").required = false;
  }
  $("login-key").focus();
}

async function pickPem(field) {
  const path = await host.pickPem().catch(() => null);
  if (path) $(field).value = path;
  $(field).focus();
}

async function saveCert() {
  $("cert-save").disabled = true;
  try {
    const values = { "remote.tlsCert": $("cert-file").value.trim(), "remote.tlsKey": $("cert-key").value.trim() };
    const result = await host.call("settings.set", { values });
    Object.assign(store.settings, result.values);
    $("cert-dialog").close();
    await settleRemote();
    $("r-cert-provide").focus();
  } catch (e) {
    $("cert-error").textContent = e?.message ?? String(e);
    $("cert-error").hidden = false;
  } finally {
    $("cert-save").disabled = false;
  }
}

async function revokeDevice() {
  const token = remoteTokens.find((x) => x.id === $("r-tokens").value);
  if (!token) return;
  try {
    await host.call("token.revoke", { tokenId: token.id });
    announce(t("Appareil « {0} » retiré : ses sessions sont fermées", token.name));
  } catch (e) {
    remoteFailed(e);
  }
  await loadRemote();
  (remoteTokens.length ? $("r-tokens") : $("r-token-new")).focus();
}

function openDevice() {
  forgetDevice();
  $("device-ask").hidden = false;
  $("device-show").hidden = true;
  $("device-create").hidden = false;
  $("device-cancel").hidden = false;
  $("device-done").hidden = true;
  $("device-name").value = "";
  $("device-scope").value = "control";
  $("device-error").hidden = true;
  const dialog = $("device-dialog");
  dialog.returnValue = "";
  dialog.showModal();
  $("device-name").focus();
}

/// Le jeton ne reste pas dans la page une fois la boîte fermée.
function forgetDevice() {
  deviceToken = null;
  $("device-token").value = "";
  $("device-qr").replaceChildren();
  $("device-url").replaceChildren();
}

async function createDevice() {
  $("device-create").disabled = true;
  try {
    const made = await host.call("token.create", {
      name: $("device-name").value.trim(),
      scope: $("device-scope").value,
    });
    deviceToken = made.token;
    $("device-token").value = made.token;
    // L'écoute peut avoir changé depuis l'ouverture de la rubrique.
    remoteStatus = await host.call("remote.status").catch(() => remoteStatus);
    // Accès pas encore ouvert : l'adresse qu'aura la page, d'après les réglages.
    const urls = qrUrls(remoteStatus?.listening
      ? remoteStatus.urls
      : plannedUrls(store.settings, remoteStatus?.addresses));
    $("device-url").replaceChildren(...urls.map((url) => new Option(url, url)));
    $("device-url-field").hidden = urls.length < 2;
    $("device-ask").hidden = true;
    $("device-show").hidden = false;
    $("device-create").hidden = true;
    $("device-cancel").hidden = true;
    $("device-done").hidden = false;
    await drawDeviceQr();
    $("device-done").focus();
    announce(t("Appareil « {0} » ajouté", made.name));
    loadRemote();
  } catch (e) {
    $("device-error").textContent = e?.message ?? String(e);
    $("device-error").hidden = false;
  } finally {
    $("device-create").disabled = false;
  }
}

async function drawDeviceQr() {
  const url = $("device-url").value;
  const box = $("device-qr");
  box.replaceChildren();
  const local = $("device-local");
  const notes = [];
  if (!remoteStatus?.listening) {
    notes.push(t("L'accès n'est pas encore ouvert : ce code servira dès que la case « Piloter les téléchargements depuis un autre appareil » sera cochée."));
  }
  if (!url || isLoopbackUrl(url)) {
    notes.push(t("L'écoute est réservée à cet ordinateur : choisissez une adresse du réseau local pour qu'un téléphone puisse joindre la page."));
  }
  local.textContent = notes.join(" ");
  local.hidden = !notes.length;
  box.hidden = !url;
  $("device-qr-note").hidden = !url;
  if (!url || !deviceToken) return;
  try {
    box.append(qrSvg(await host.qrCode(pageLink(url, deviceToken))));
  } catch (e) {
    $("device-error").textContent = e?.message ?? String(e);
    $("device-error").hidden = false;
  }
}

/// Le QR code dessiné en SVG, noir sur blanc, avec sa marge.
function qrSvg({ width, modules }) {
  const NS = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(NS, "svg");
  const size = width + 8;
  svg.setAttribute("viewBox", `0 0 ${size} ${size}`);
  svg.setAttribute("shape-rendering", "crispEdges");
  svg.setAttribute("aria-hidden", "true");
  const background = document.createElementNS(NS, "rect");
  background.setAttribute("width", String(size));
  background.setAttribute("height", String(size));
  background.setAttribute("fill", "#fff");
  const dark = document.createElementNS(NS, "path");
  dark.setAttribute("d", qrPath(width, modules));
  dark.setAttribute("fill", "#000");
  svg.append(background, dark);
  return svg;
}

async function copyDeviceToken() {
  const field = $("device-token");
  try {
    await navigator.clipboard.writeText(field.value);
    announce(t("Jeton copié"));
  } catch {
    field.focus();
    field.select();
  }
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
  // Sous Linux, pas d'animation : la page y est dessinée sans composition (style.css).
  document.documentElement.dataset.platform = info.platform;
  translatePage();
  renderDest();
  $("pref-lang").value = info.prefs.lang;
  $("pref-tray").checked = info.prefs.closeToTray && info.tray;
  $("pref-tray").disabled = !info.tray;
  $("pref-no-tray").hidden = info.tray || !caps.app;
  // Absente d'un ancien fichier de préférences : activée, comme le défaut de l'application.
  $("pref-show-on-add").checked = info.prefs.showOnAdd !== false;
  // Presse-papiers (F-30) : seulement avec un hôte qui sait le lire (pas l'interface distante).
  $("pref-clipboard").checked = info.prefs.watchClipboard === true;
  $("pref-clipboard-row").hidden = !host.watchesClipboard;
  $("pref-clipboard-note").hidden = !host.watchesClipboard;
  renderClipboardWarning();
  $("pref-autostart").checked = info.autostart;
  renderUpdates();
  $("grid").setAttribute("aria-label", t("Téléchargements : {0}", viewLabel(view)));
  renderAbout();
  renderLimit();
  renderTorrentSettings();
  renderAddTorrents();
  renderTools();
  renderPlanning();
  renderAll();
}

// Option du presse-papiers cochée alors que le système n'en affichera pas les notifications
// (Windows) : averti sous la case, relu à chaque ouverture des Options.
async function renderClipboardWarning() {
  let blocked = false;
  if (host.notificationsBlocked && $("pref-clipboard").checked) {
    try {
      blocked = await host.notificationsBlocked();
    } catch {
      blocked = false;
    }
  }
  $("pref-clipboard-off").hidden = !blocked;
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
  if (host.logout) $("tb-logout").addEventListener("click", () => host.logout());
  $("tb-video").addEventListener("click", openVideo);
  // Analyse de liens (étape 2b).
  $("tb-links").addEventListener("click", openLinks);
  $("l-scan").addEventListener("click", scanSource);
  $("l-source").addEventListener("keydown", (event) => {
    // Ctrl+Entrée analyse ; Entrée seule passe à la ligne (texte de plusieurs adresses).
    if (event.key === "Enter" && event.ctrlKey) {
      event.preventDefault();
      scanSource();
    }
  });
  $("l-import").addEventListener("click", importLinks);
  for (const id of ["l-ext", "l-min", "l-max", "l-regex", "l-all"]) {
    $(id).addEventListener(id === "l-all" ? "change" : "input", () => {
      renderLinkList();
      probeSizes();
    });
  }
  $("l-check-all").addEventListener("click", () => checkShownLinks(true));
  $("l-check-none").addEventListener("click", () => checkShownLinks(false));
  $("l-links").addEventListener("change", (event) => {
    const url = event.target.dataset?.url;
    if (!url) return;
    if (event.target.checked) linkUnchecked.delete(url);
    else linkUnchecked.add(url);
    renderLinkCount();
  });
  $("links-form").addEventListener("submit", (event) => {
    const value = event.submitter?.value;
    if (value !== "start" && value !== "later") return;
    // Ajoutés d'abord ; la boîte ne se ferme que si tout est ajouté.
    event.preventDefault();
    addLinks(value);
  });
  $("links-dialog").addEventListener("close", closeLinks);
  $("v-probe").addEventListener("click", probeVideo);
  $("v-page").addEventListener("keydown", (event) => {
    // Entrée analyse la page, au lieu d'actionner le premier bouton du formulaire.
    if (event.key === "Enter") {
      event.preventDefault();
      probeVideo();
    }
  });
  $("v-tools-install").addEventListener("click", () => {
    probeAfterInstall = true;
    installTools(false);
  });
  $("v-formats").addEventListener("change", (event) => {
    if (event.target.name === "v-format") chooseExactFormat(event.target.value);
  });
  $("video-form").addEventListener("submit", (event) => {
    const value = event.submitter?.value;
    if (value !== "start" && value !== "later") return;
    // Ajoutée d'abord ; la boîte ne se ferme qu'en cas de succès.
    event.preventDefault();
    addVideo(value);
  });
  $("video-dialog").addEventListener("close", () => {
    if ($("video-dialog").returnValue === "added") resetVideo();
    closeVideo();
  });
  $("m-install").addEventListener("click", () => installTools(false));
  $("m-reinstall").addEventListener("click", () => installTools(true));
  $("pref-ytdlp-update").addEventListener("change", async (e) => {
    await saveSetting("media.autoUpdate", e.target.checked,
      e.target.checked ? t("yt-dlp se mettra à jour de lui-même") : t("yt-dlp ne se mettra plus à jour de lui-même"));
    renderTools();
  });
  $("tb-resume").addEventListener("click", () => resume(selectedTasks()));
  $("tb-pause").addEventListener("click", () => pause(selectedTasks()));
  $("tb-resume-all").addEventListener("click", () => resume([...store.tasks.values()]));
  $("tb-pause-all").addEventListener("click", () => pause([...store.tasks.values()]));
  $("tb-remove").addEventListener("click", openRemove);
  $("tb-clear").addEventListener("click", clearFinished);
  $("tb-properties").addEventListener("click", () => cursor && openDetails(cursor));
  $("tb-schedule").addEventListener("click", openSchedule);
  $("tb-history").addEventListener("click", openHistory);
  $("pref-preallocate").addEventListener("change", savePreallocate);
  // Proxy (F-07).
  for (const mode of ["system", "none", "manual"]) $(`proxy-${mode}`).addEventListener("change", syncProxyFields);
  $("proxy-apply").addEventListener("click", saveProxy);
  // Journaux et diagnostic (N-12).
  $("pref-logs-level").addEventListener("change", saveLogsLevel);
  $("diagnostic-export").addEventListener("click", openDiagnostic);
  $("diagnostic-form").addEventListener("submit", (event) => {
    if (event.submitter?.value !== "save") return;
    // Enregistré d'abord ; la boîte ne se ferme qu'en cas de succès.
    event.preventDefault();
    saveDiagnostic();
  });
  $("diagnostic-dialog").addEventListener("close", () => { pendingDiagnostic = null; });
  $("pref-neighbor").addEventListener("change", saveNeighbor);
  $("h-query").addEventListener("input", () => {
    // Recherche pendant la frappe, une fois la saisie posée.
    clearTimeout(historyTimer);
    historyTimer = setTimeout(() => loadHistory(true), 250);
  });
  $("h-query").addEventListener("keydown", (event) => {
    // Sans cela, Entrée actionnerait le premier bouton du formulaire, qui ferme.
    if (event.key === "Enter") {
      event.preventDefault();
      clearTimeout(historyTimer);
      loadHistory(true);
    }
  });
  $("h-more").addEventListener("click", () => loadHistory(false));
  $("h-export").addEventListener("click", exportHistory);
  $("h-clear").addEventListener("click", openClearHistory);
  $("history-clear-dialog").addEventListener("close", closeClearHistory);
  $("history-dialog").addEventListener("close", () => {
    clearTimeout(historyTimer);
    historySeq++;
    $("tb-history").focus();
  });
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
    if (connected && caps.admin) loadHostKeys();
    if (connected) loadRules();
    if (connected) loadInterfaces();
    if (connected && caps.qr && caps.admin) loadRemote();
    if (connected && caps.plugins && caps.admin) loadPlugins();
    if (connected && caps.admin) loadProxy();
    renderLinks();
    loadTools();
    renderClipboardWarning();
  });
  // Accès à distance (étape 3d).
  $("r-enabled").addEventListener("change", (e) => changeRemote(
    { "remote.enabled": e.target.checked },
    e.target.checked ? t("Accès à distance ouvert") : t("Accès à distance coupé"),
  ));
  $("r-address").addEventListener("change", changeListen);
  $("r-port").addEventListener("change", changeListen);
  $("r-refresh").addEventListener("click", loadRemote);
  $("r-cert-provide").addEventListener("click", openCertDialog);
  $("r-cert-generated").addEventListener("click", () => changeRemote(
    { "remote.tlsCert": null, "remote.tlsKey": null },
    t("Certificat d'AuroDownloader rétabli"),
  ));
  $("cert-file-pick").addEventListener("click", () => pickPem("cert-file"));
  $("cert-key-pick").addEventListener("click", () => pickPem("cert-key"));
  $("cert-form").addEventListener("submit", (event) => {
    if (event.submitter?.value !== "save") return;
    event.preventDefault();
    saveCert();
  });
  $("r-tokens").addEventListener("change", renderRemote);
  $("r-token-new").addEventListener("click", openDevice);
  $("r-token-revoke").addEventListener("click", revokeDevice);
  $("device-form").addEventListener("submit", (event) => {
    if (event.submitter?.value !== "create") return;
    event.preventDefault();
    createDevice();
  });
  $("device-url").addEventListener("change", drawDeviceQr);
  $("device-copy").addEventListener("click", copyDeviceToken);
  $("device-dialog").addEventListener("close", () => {
    forgetDevice();
    $("r-token-new").focus();
  });
  // Plugins (étape 4c).
  $("plugins").addEventListener("change", () => renderPlugins());
  $("plugin-install").addEventListener("click", pickPlugin);
  $("plugin-toggle").addEventListener("click", togglePlugin);
  $("plugin-remove").addEventListener("click", removePlugin);
  $("plugin-form").addEventListener("submit", (event) => {
    if (event.submitter?.value !== "install") return;
    // Installé d'abord ; la boîte ne se ferme qu'en cas de succès.
    event.preventDefault();
    installPlugin();
  });
  // Le fichier lu ne reste pas dans la page ; la boîte rend elle-même le focus au bouton.
  $("plugin-dialog").addEventListener("close", () => { pendingPlugin = null; });
  // Règles de rangement (SPEC §7.21).
  $("rules").addEventListener("change", syncRuleButtons);
  $("rules").addEventListener("dblclick", () => {
    const rule = rules[chosenRuleIndex()];
    if (rule && caps.admin) openRule(rule);
  });
  $("rule-new").addEventListener("click", () => openRule(null));
  $("rule-edit").addEventListener("click", () => {
    const rule = rules[chosenRuleIndex()];
    if (rule) openRule(rule);
  });
  $("rule-up").addEventListener("click", () => moveChosenRule(-1));
  $("rule-down").addEventListener("click", () => moveChosenRule(1));
  $("rule-toggle").addEventListener("click", toggleChosenRule);
  $("rule-delete").addEventListener("click", deleteChosenRule);
  $("rule-folder-pick").addEventListener("click", async () => {
    const folder = await host.pickFolder($("rule-folder").value || undefined).catch(() => null);
    if (folder) $("rule-folder").value = folder;
  });
  $("rule-form").addEventListener("submit", (event) => {
    if (event.submitter?.value !== "save") return;
    // Enregistrée d'abord ; la boîte ne se ferme qu'en cas de succès.
    event.preventDefault();
    saveRule();
  });
  $("rule-dialog").addEventListener("close", () => { editingRule = null; });
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
  $("login-key-pick").addEventListener("click", pickKey);
  $("login-key").addEventListener("input", () => {
    $("login-password").required = !editingLogin && !$("login-key").value.trim();
  });
  $("hostkeys").addEventListener("change", syncHostKeyButtons);
  $("hostkey-forget").addEventListener("click", forgetHostKey);
  $("d-hostkey-accept").addEventListener("click", offerHostKey);
  $("hostkey-dialog").querySelector("form").addEventListener("submit", (event) => {
    if (event.submitter?.value === "accept") acceptHostKey(event);
  });
  $("hostkey-dialog").addEventListener("close", () => { offeredKey = null; });
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

  // « Démarrer » et « Télécharger plus tard » ajoutent, la boîte ouverte ; « Annuler » et
  // Échap la ferment.
  $("add-form").addEventListener("submit", (event) => {
    const choice = event.submitter?.value;
    if (choice !== "start" && choice !== "later") return;
    event.preventDefault();
    submitAdd(choice);
  });
  $("add-dialog").addEventListener("close", () => $("tb-add").focus());
  // Plugins des adresses saisies (étape 4c) : `plugin.match` seulement, sans requête aux sites.
  $("urls").addEventListener("input", schedulePluginMatch);
  $("urls").addEventListener("keydown", (event) => {
    // Entrée démarre ; Maj+Entrée passe à la ligne.
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      submitAdd("start");
    }
  });
  // Sans cela, Entrée dans un champ actionnerait le premier bouton du formulaire.
  $("add-checksum").addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      submitAdd("start");
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
  $("d-mirror-add").addEventListener("click", addMirror);
  $("d-mirror-input").addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      addMirror();
    }
  });
  $("d-files-apply").addEventListener("click", applyFileChoice);
  $("d-files").addEventListener("change", (event) => {
    const box = event.target.closest("input[type=checkbox]");
    if (!box) return;
    fileChoice.set(Number(box.dataset.index), box.checked);
    box.closest("tr").classList.toggle("skipped", !box.checked);
    $("d-files-apply").disabled = !connected;
  });
  $("add-torrent-pick").addEventListener("click", pickTorrentFiles);
  $("pref-ratio").addEventListener("change", (e) => {
    const value = e.target.value === "none" ? null : Number(e.target.value);
    saveSetting("torrent.ratioLimit", value, t("Partage des torrents réglé"));
  });
  $("pref-upload").addEventListener("change", changeUploadLimit);
  $("pref-upnp").addEventListener("change", (e) =>
    saveSetting("torrent.upnp", e.target.checked, e.target.checked ? t("UPnP activé") : t("UPnP désactivé")));
  $("pref-interface").addEventListener("change", (e) =>
    saveSetting("torrent.interface", e.target.value || null, t("Interface des torrents réglée")));
  $("pref-interface-refresh").addEventListener("click", loadInterfaces);
  $("pref-links").addEventListener("change", changeLinks);

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
  $("pref-show-on-add").addEventListener("change", (e) => savePrefs({ showOnAdd: e.target.checked }));
  // Mises à jour (étape 8b).
  $("pref-updates").addEventListener("change", (e) => savePrefs({ checkUpdates: e.target.checked }));
  $("update-check-now").addEventListener("click", checkUpdatesNow);
  $("update-install").addEventListener("click", installUpdate);
  $("update-page").addEventListener("click", () => {
    if (proposedUpdate) host.updateOpenPage(proposedUpdate.version).catch((e) => showError(refused(e)));
  });
  $("update-notice").addEventListener("close", () => {
    const off = $("update-notice").returnValue === "off";
    savePrefs({ updateNoticeShown: true, checkUpdates: !off });
    $("tb-add").focus();
  });
  $("pref-clipboard").addEventListener("change", (e) => {
    savePrefs({ watchClipboard: e.target.checked });
    renderClipboardWarning();
  });
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
    // Guide de l'utilisateur (étape 9d), de partout.
    if (event.key === "F1" && !event.ctrlKey && !event.altKey && !event.metaKey && !event.shiftKey) {
      event.preventDefault();
      openGuide();
    }
  });
  $("guide-open").addEventListener("click", openGuide);
  $("guide-content").addEventListener("click", followGuideLink);
}

/// Page distante (étape 3c) : la session de ce navigateur, ouverte au besoin avec le jeton de
/// l'ancre de l'adresse (QR code du bureau) ou celui saisi ; rend sa portée.
async function signIn() {
  const fromHash = host.tokenFromHash();
  if (fromHash) await host.login(fromHash).catch(() => null);
  let who = await host.session();
  const dialog = $("remote-login");
  dialog.addEventListener("cancel", (event) => event.preventDefault());
  while (!who) {
    dialog.returnValue = "";
    dialog.showModal();
    $("rl-token").focus();
    await new Promise((resolve) => dialog.addEventListener("close", resolve, { once: true }));
    const pasted = $("rl-token").value;
    const token = pastedToken(pasted);
    $("rl-token").value = "";
    // Un texte sans jeton complet n'est pas envoyé : le dire, sans le montrer.
    if (!isToken(token)) {
      $("rl-error").textContent = t("Pas de jeton dans le texte collé ({0} caractères) : un jeton commence par auro_, suivi de 43 lettres, chiffres, - ou _.", pasted.length);
      $("rl-error").hidden = false;
      continue;
    }
    try {
      who = await host.login(token);
      $("rl-error").hidden = true;
    } catch (e) {
      $("rl-error").textContent = e.code === "TOO_MANY_ATTEMPTS"
        ? t("Trop d'essais refusés : réessayez dans une minute.")
        : e.code === "UNAUTHORIZED"
          ? t("Jeton inconnu du moteur, ou révoqué.")
          : refused(e);
      $("rl-error").hidden = false;
    }
  }
  return who.scope;
}

async function start() {
  wire();
  applyInfo(await host.info());
  applyScope(host.remote ? await signIn() : "admin");
  // S'abonner d'abord, puis demander l'état de la connexion : rien n'est manqué.
  await host.listen(onEngine);
  onStatus(await host.status());
  // Liens magnet et .torrent ouverts avec l'application (étape 5b) : ceux du lancement,
  // puis ceux d'une relance ou d'un glisser-déposer.
  if (host.onOpened) {
    await host.onOpened(takeOpened);
    takeOpened();
  }
  (cursor ? rows.get(cursor) : $("tb-add"))?.focus();
  // Après le focus de la fenêtre : l'avis du premier lancement le prend à son tour.
  await setupUpdates();
}

// ---------- Mises à jour de l'application (SPEC §4.6, étape 8b) ----------

/// État tenu par l'hôte (`updateStatus`) ; `null` sans mises à jour (page distante).
let updateStatus = null;
/// Version proposée par la boîte « Nouvelle version ».
let proposedUpdate = null;
/// Version trouvée par la recherche de fond pendant qu'une autre boîte était ouverte.
let waitingUpdate = null;
let updateChecking = false;

async function setupUpdates() {
  if (typeof host.updateStatus !== "function") return;
  await host.onUpdate(async (found) => {
    await refreshUpdateStatus();
    proposeUpdate(found);
  });
  await host.onUpdateProgress(showUpdateProgress);
  await refreshUpdateStatus();
  // Avis du premier lancement, une fois : la recherche, et comment la couper.
  if (caps.updates && updateStatus.configured && info.prefs.updateNoticeShown !== true
    && !document.querySelector("dialog[open]")) {
    $("update-notice").returnValue = "";
    $("update-notice").showModal();
  }
}

async function refreshUpdateStatus() {
  try {
    updateStatus = await host.updateStatus();
  } catch {
    updateStatus = null;
  }
  caps.updates = Boolean(updateStatus) && updateStatus.kind !== "none";
  markNeeds(document);
  renderUpdates();
}

/// Rubrique « Mises à jour » d'Options › Général.
function renderUpdates() {
  if (!updateStatus) return;
  $("pref-updates").checked = info.prefs.checkUpdates !== false;
  $("pref-updates").disabled = !updateStatus.configured;
  $("update-check-now").disabled = !updateStatus.configured || updateChecking;
  if (updateChecking) return;
  const text = !updateStatus.configured
    ? t("Mises à jour non configurées dans cette version.")
    : updateStatus.found
      ? t("Version {0} disponible.", updateStatus.found.version)
      : updateStatus.lastCheck
        ? t("Dernière recherche : {0}.", dateText(updateStatus.lastCheck * 1000))
        : t("Aucune recherche faite pour l'instant.");
  if ($("update-state").textContent !== text) $("update-state").textContent = text;
}

/// « Rechercher maintenant » : sans attendre un jour.
async function checkUpdatesNow() {
  if (updateChecking) return;
  updateChecking = true;
  renderUpdates();
  $("update-state").textContent = t("Recherche d'une version nouvelle…");
  let found = null;
  let failure = null;
  try {
    found = (await host.updateCheck(true))?.found ?? null;
  } catch (e) {
    failure = e?.message ?? String(e);
  }
  updateChecking = false;
  await refreshUpdateStatus();
  if (failure) $("update-state").textContent = t("Recherche impossible : {0}", failure);
  else if (found) proposeUpdate(found);
  else $("update-state").textContent = t("AuroDownloader est à jour (version {0}).", info.version);
}

/// Propose une version : tout de suite si rien n'est ouvert, ou par-dessus les Options
/// (« Rechercher maintenant ») ; sinon à la fermeture de la boîte ouverte.
function proposeUpdate(found) {
  const open = [...document.querySelectorAll("dialog[open]")];
  if (open.some((dialog) => dialog.id === "update-dialog")) return;
  if (open.length && !(open.length === 1 && open[0].id === "settings")) {
    waitingUpdate = found;
    return;
  }
  waitingUpdate = null;
  proposedUpdate = found;
  const install = updateStatus?.kind === "install";
  $("update-text").textContent = t("AuroDownloader {0} est disponible ; vous avez la version {1}.", found.version, found.current);
  // Notes de la publication : du texte, jamais du HTML.
  const notes = (found.notes ?? "").trim();
  $("update-notes").textContent = notes;
  $("update-notes-field").hidden = !notes;
  $("update-install").hidden = !install;
  $("update-install").disabled = false;
  $("update-install-note").hidden = !install;
  $("update-page-note").hidden = install;
  $("update-progress-row").hidden = true;
  const dialog = $("update-dialog");
  dialog.returnValue = "";
  dialog.showModal();
  (install ? $("update-install") : $("update-page")).focus();
}

document.addEventListener("close", (event) => {
  if (waitingUpdate && event.target?.tagName === "DIALOG") setTimeout(() => waitingUpdate && proposeUpdate(waitingUpdate), 0);
}, true);

async function installUpdate() {
  if (!proposedUpdate) return;
  $("update-install").disabled = true;
  $("update-progress").removeAttribute("value");
  $("update-progress-label").textContent = "";
  $("update-progress-row").hidden = false;
  announce(t("Téléchargement de la version {0}…", proposedUpdate.version));
  try {
    // L'application se ferme ou redémarre d'elle-même une fois la version installée.
    await host.updateInstall();
  } catch (e) {
    $("update-progress-row").hidden = true;
    $("update-install").disabled = false;
    showError(t("Mise à jour impossible : {0}", e?.message ?? String(e)));
  }
}

/// Avancement du téléchargement : la barre seule (pas d'annonce à chaque morceau).
function showUpdateProgress({ done, total }) {
  const bar = $("update-progress");
  if (total) {
    bar.max = total;
    bar.value = done;
    $("update-progress-label").textContent = t("{0} sur {1}", bytes(done, lang, t), bytes(total, lang, t));
  } else {
    $("update-progress-label").textContent = bytes(done, lang, t);
  }
}

// ---------- Guide de l'utilisateur (étape 9d) ----------

/// Copies du guide (ui/src/guide/<langue>.html, faites par scripts/guide.mjs), lues une fois.
const guides = new Map();

/// Ouvre le guide, dans la langue de la page ; par-dessus une boîte ouverte (Options).
async function openGuide() {
  const dialog = $("guide-dialog");
  if (dialog.open) return;
  const want = lang === "en" ? "en" : "fr";
  const content = $("guide-content");
  if (content.dataset.lang !== want) {
    let html = guides.get(want);
    if (html === undefined) {
      try {
        const response = await fetch(`guide/${want}.html`);
        if (!response.ok) throw new Error(`HTTP ${response.status}`);
        html = await response.text();
        guides.set(want, html);
      } catch (e) {
        showError(t("Guide introuvable : {0}", e?.message ?? String(e)));
        return;
      }
    }
    // Fragment de l'application elle-même, sans script, tout son texte échappé.
    content.innerHTML = html;
    content.dataset.lang = want;
  }
  dialog.showModal();
  content.scrollTop = 0;
  content.focus();
}

/// Lien interne du guide : le titre visé défile en vue et prend le focus (le lecteur d'écran
/// le lit) ; l'adresse de la page ne change pas.
function followGuideLink(event) {
  const link = event.target.closest?.('a[href^="#guide-"]');
  if (!link) return;
  event.preventDefault();
  const target = $(link.getAttribute("href").slice(1));
  if (!target) return;
  target.scrollIntoView({ block: "start" });
  target.focus();
}

start().catch((e) => showError(String(e?.message ?? e)));

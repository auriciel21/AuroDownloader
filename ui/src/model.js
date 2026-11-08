// État des tâches tenu à jour par `sync.snapshot` et les notifications du moteur
// (SPEC §6.5, §6.10). Sans lien avec la page : testé seul (ui/tests).
//
// Règles de la SPEC §6.10 :
// - s'abonner, puis charger l'instantané ; ignorer les notifications de révision
//   inférieure ou égale à la sienne ;
// - `rev` augmente de 1 à chaque changement (tâches et files, les deux sujets suivis) :
//   un trou signale une notification perdue, il faut recharger l'instantané ;
// - l'avancement porte la révision courante sans l'augmenter.

/// Branches de l'arborescence : tout, pas encore terminé, terminé.
export const GROUPS = ["all", "incomplete", "completed"];

/// Types de fichier, d'après l'extension du nom ; « other » pour le reste.
export const CATEGORIES = ["compressed", "documents", "images", "music", "programs", "video", "other"];

const EXTENSIONS = {
  compressed: "7z ace arj bz2 cab gz lz lzma rar tar tbz2 tgz txz xz z zip zst",
  documents: "chm csv djvu doc docx epub md mobi odp ods odt pdf pps ppt pptx rtf txt xls xlsx",
  images: "avif bmp gif heic ico jpeg jpg png psd svg tif tiff webp",
  music: "aac aif aiff alac ape flac m4a mid mp3 mpa oga opus ra wav wma",
  programs: "apk appimage appx bat deb dmg exe flatpak img iso jar msi msix msu pkg rpm run sh snap",
  video: "3gp asf avi flv m2ts m4v mkv mov mp4 mpe mpeg mpg ogv qt rm rmvb ts vob webm wmv",
};

const CATEGORY_OF = new Map(
  Object.entries(EXTENSIONS).flatMap(([cat, list]) => list.split(" ").map((ext) => [ext, cat])),
);

/// Nom affiché d'une tâche : celui du fichier, ou le nom provisoire avant préparation.
export const taskFileName = (task) => task.fileName || task.displayName || "";

/// Type d'un fichier d'après son extension ; les archives découpées (`.r00`, `.7z.001`)
/// sont des archives.
export function category(name) {
  const dot = name.lastIndexOf(".");
  if (dot <= 0) return "other";
  const ext = name.slice(dot + 1).toLowerCase();
  if (/^(r|z)\d\d$|^\d{3}$/.test(ext)) return "compressed";
  return CATEGORY_OF.get(ext) ?? "other";
}

/// La tâche est dans cette branche : les tâches annulées ne sont que sous « tout ».
export function inGroup(task, group) {
  if (group === "completed") return task.state === "completed";
  if (group === "incomplete") return task.state !== "completed" && task.state !== "cancelled";
  return true;
}

/// Vue de l'arborescence : `all`, `incomplete/video`, `queue:<id>` (le travail restant
/// d'une file).
export function inView(task, view) {
  if (view.startsWith("queue:")) return task.queueId === view.slice(6) && inGroup(task, "incomplete");
  const [group, cat] = view.split("/");
  return inGroup(task, group) && (!cat || category(taskFileName(task)) === cat);
}

/// Colonnes du tableau, dans l'ordre d'affichage.
export const COLUMNS = ["name", "size", "progress", "state", "eta", "rate", "added"];

/// Colonnes dont la valeur change avec l'avancement : la liste se retrie à chaque lot.
export const LIVE_COLUMNS = new Set(["progress", "eta", "rate", "size"]);

const STATE_ORDER = ["active", "retry_wait", "queued", "verifying", "post_processing", "seeding",
  "paused", "failed", "completed", "cancelled"];

function fraction(task) {
  if (task.state === "completed") return 1;
  return task.totalBytes > 0 ? Math.min(task.doneBytes ?? 0, task.totalBytes) / task.totalBytes : -1;
}

const KEYS = {
  name: (task) => taskFileName(task),
  size: (task) => task.totalBytes ?? -1,
  progress: fraction,
  state: (task) => STATE_ORDER.indexOf(task.state),
  // Sans estimation, à la fin dans l'ordre croissant.
  eta: (task) => (task.state === "active" && task.etaS != null ? task.etaS : Infinity),
  rate: (task) => task.bps ?? 0,
  added: (task) => task.createdAt ?? 0,
};

const byName = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

/// Trie les tâches selon une colonne ; à égalité, les plus récentes d'abord
/// (identifiants UUID v7, ordonnés dans le temps).
export function sortTasks(tasks, column = "added", descending = true) {
  const key = KEYS[column] ?? KEYS.added;
  const sign = descending ? -1 : 1;
  const compare = column === "name"
    ? (a, b) => byName.compare(a, b)
    : (a, b) => (a < b ? -1 : a > b ? 1 : 0);
  return tasks
    .map((task) => ({ task, value: key(task) }))
    .sort((a, b) => sign * compare(a.value, b.value)
      || (a.task.taskId < b.task.taskId ? 1 : a.task.taskId > b.task.taskId ? -1 : 0))
    .map(({ task }) => task);
}

// Actions permises par la table des états (SPEC §5.5).
// `verifying` : le calcul de l'empreinte s'interrompt, et reprend depuis le début (F-11).
export const canPause = (state) => ["queued", "active", "retry_wait", "verifying"].includes(state);
export const canResume = (state) => ["paused", "failed"].includes(state);
export const canCancel = (state) =>
  ["queued", "active", "paused", "retry_wait", "verifying", "seeding"].includes(state);
// Retrait de la liste (`task.remove`) : tâches finies seulement.
export const canRemove = (state) => ["completed", "failed", "cancelled"].includes(state);
/// Détail d'une tâche (`task.get` avec segments) à relire : sauf s'il a été lu une fois la
/// tâche finie. Un détail lu pendant le téléchargement montre encore des segments en
/// réception, même si la tâche s'est terminée depuis.
export const detailsOutdated = (details, taskId) =>
  details?.taskId !== taskId || !["completed", "failed", "cancelled"].includes(details.state);
// Retéléchargement depuis le début (`task.restart`) : tout ce qui ne tourne pas ni n'attend
// son tour.
export const canRestart = (state) => ["completed", "failed", "cancelled", "paused", "retry_wait"].includes(state);
/// « Déplacer/Renommer » et « Actualiser l'adresse » (SPEC §7.12 et §7.13) : une tâche qui ne
/// tourne pas, et dont le fichier n'est pas déjà en cours de déplacement.
export const canRelocate = (task) =>
  !task.moving && ["queued", "paused", "retry_wait", "failed", "cancelled", "completed"].includes(task.state);
/// Empreinte attendue (F-11) : modifiable tant que le fichier n'est ni terminé, ni annulé, ni en
/// cours de vérification.
export const canSetChecksum = (task) =>
  ["queued", "active", "paused", "retry_wait", "failed"].includes(task.state);

const HEX_LENGTHS = { 64: "sha256", 40: "sha1", 32: "md5" };

/// Empreinte écrite par l'utilisateur : « sha256:… », « sha1:… », « md5:… », ou la valeur
/// seule, dont la longueur donne l'algorithme (comme `auro --checksum`). Rend
/// `{ checksum: {algo, value} }`, `{ checksum: null }` pour un champ vide, ou `{ error }`.
export function parseChecksum(text) {
  let value = (text ?? "").trim();
  if (!value) return { checksum: null };
  let algo = null;
  const colon = value.indexOf(":");
  if (colon >= 0) {
    algo = value.slice(0, colon).trim().toLowerCase().replace("-", "");
    value = value.slice(colon + 1).trim();
    if (!Object.values(HEX_LENGTHS).includes(algo)) return { error: "algo" };
  }
  algo ??= HEX_LENGTHS[value.length] ?? null;
  if (!algo) return { error: "length" };
  const length = Number(Object.keys(HEX_LENGTHS).find((n) => HEX_LENGTHS[n] === algo));
  if (value.length !== length || !/^[0-9a-fA-F]+$/.test(value)) return { error: "value" };
  return { checksum: { algo, value: value.toLowerCase() } };
}

/// Nom affiché d'un algorithme d'empreinte.
export const ALGO_LABELS = { sha256: "SHA-256", sha1: "SHA-1", md5: "MD5" };

/// Ce qu'un changement demande à l'affichage.
function outcome() {
  return {
    /// Tâches à redessiner.
    changed: new Set(),
    /// L'ordre ou le contenu de la liste peut avoir changé.
    list: false,
    /// Une notification a été perdue : recharger l'instantané.
    resync: false,
    /// Tâches terminées ou en échec, à annoncer : `{taskId, to}`.
    finished: [],
    /// Les limites globales ont changé.
    limits: false,
    /// Les réglages du moteur (dossier des téléchargements) ont changé.
    settings: false,
    /// Les planifications, les files ou l'action de fin annoncée ont changé.
    planning: false,
    /// Le refus d'une action de fin par le système, à montrer.
    endActionError: null,
    /// Déplacements vers un autre volume qui ont échoué, à montrer : `{taskId, error}`.
    moveErrors: [],
  };
}

function merge(into, other) {
  for (const id of other.changed) into.changed.add(id);
  into.list ||= other.list;
  into.resync ||= other.resync;
  into.limits ||= other.limits;
  into.settings ||= other.settings;
  into.planning ||= other.planning;
  into.endActionError ??= other.endActionError;
  into.finished.push(...other.finished);
}

/// Tâche telle que la liste la garde : l'avancement manque à l'instantané.
function fresh(task) {
  return { bps: 0, etaS: null, connections: 0, createdAt: null, finishedAt: null, ...task };
}

const FINAL = new Set(["completed", "failed", "cancelled"]);

export class TaskStore {
  constructor() {
    this.tasks = new Map();
    this.queues = new Map();
    /// Limites globales (`limits.get`) ; `null` avant le premier instantané.
    this.limits = null;
    /// Réglages du moteur (`settings.get`), par clé.
    this.settings = {};
    /// Planifications (F-16), par identifiant, avec leur ouverture.
    this.schedules = new Map();
    /// Action de fin annoncée (`{queueId, action, at}`), ou `null`.
    this.endAction = null;
    /// Révision de l'état chargé ; `null` tant qu'un instantané est attendu.
    this.rev = null;
    /// Notifications reçues en attendant l'instantané.
    this.waiting = [];
  }

  /// Un instantané va être demandé (connexion nouvelle ou rechargement) : les
  /// notifications sont gardées de côté jusqu'à son arrivée. Les tâches restent affichées.
  reset() {
    this.rev = null;
    this.waiting = [];
  }

  /// Charge l'instantané, puis les notifications plus récentes reçues entre-temps.
  load(snapshot) {
    this.tasks = new Map(snapshot.tasks.map((t) => [t.taskId, fresh(t)]));
    this.queues = new Map(snapshot.queues.map((q) => [q.queueId, q]));
    this.limits = snapshot.limits ?? null;
    this.settings = { ...snapshot.settings };
    this.schedules = new Map((snapshot.schedules ?? []).map((s) => [s.scheduleId, s]));
    this.endAction = snapshot.endAction ?? null;
    this.rev = snapshot.rev;
    const result = outcome();
    result.list = true;
    result.limits = true;
    result.settings = true;
    result.planning = true;
    for (const id of this.tasks.keys()) result.changed.add(id);
    const waiting = this.waiting;
    this.waiting = [];
    for (const { method, params, now } of waiting) merge(result, this.apply(method, params, now));
    return result;
  }

  /// Applique une notification du moteur ; `now` date la fin d'une tâche, que
  /// `task.stateChanged` ne porte pas (l'instantané suivant donne celle du moteur).
  apply(method, params, now = Date.now()) {
    const result = outcome();
    if (this.rev === null) {
      this.waiting.push({ method, params, now });
      return result;
    }
    switch (method) {
      case "task.progress":
        // Antérieur à l'état chargé : périmé.
        if (params.rev < this.rev) return result;
        for (const item of params.items) {
          const task = this.tasks.get(item.taskId);
          if (!task) continue;
          Object.assign(task, item);
          result.changed.add(item.taskId);
        }
        return result;
      case "engine.warning":
        if (params.code === "EVENTS_LOST") result.resync = true;
        return result;
      case "task.added":
      case "task.stateChanged":
      case "task.updated":
      case "task.removed":
      case "queue.updated":
      case "limits.updated":
      case "settings.updated":
      case "schedule.updated":
      case "schedule.removed":
      case "engine.endAction":
      // Réseau utilisable ou non (SPEC §7.15, application Android) : numérotée, sans effet ici.
      case "engine.network":
        break;
      default:
        return result;
    }
    // Déjà compris dans l'état chargé.
    if (params.rev <= this.rev) return result;
    if (params.rev > this.rev + 1) result.resync = true;
    this.rev = params.rev;
    switch (method) {
      case "task.added":
        this.tasks.set(params.task.taskId, fresh(params.task));
        result.changed.add(params.task.taskId);
        result.list = true;
        break;
      case "task.stateChanged": {
        const task = this.tasks.get(params.taskId);
        if (!task) {
          result.resync = true;
          break;
        }
        task.state = params.to;
        task.pauseReason = params.pauseReason ?? null;
        task.errorCode = params.errorCode ?? null;
        if (params.to !== "active") {
          task.bps = 0;
          task.etaS = null;
          task.connections = 0;
        }
        // Octets lus par la vérification de l'empreinte : seulement pendant celle-ci.
        if (params.to !== "verifying") task.verifiedBytes = null;
        if (params.to === "completed" && task.totalBytes > 0) task.doneBytes = task.totalBytes;
        task.finishedAt = FINAL.has(params.to) ? now : null;
        result.changed.add(params.taskId);
        // Change de branche, ou de place dans un tri par état.
        result.list = true;
        if (params.to === "completed" || params.to === "failed") {
          result.finished.push({ taskId: params.taskId, to: params.to });
        }
        break;
      }
      case "task.updated": {
        const task = this.tasks.get(params.taskId);
        if (!task) {
          result.resync = true;
          break;
        }
        const { rev: _rev, taskId: _id, moveError, ...fields } = params;
        Object.assign(task, fields);
        // Échec d'une copie vers un autre volume : annoncé, pas gardé dans la tâche.
        if (moveError) result.moveErrors.push({ taskId: params.taskId, error: moveError });
        result.changed.add(params.taskId);
        // Un nom change le type de fichier, donc les vues, et l'ordre par nom.
        result.list = "fileName" in fields || "displayName" in fields || "totalBytes" in fields;
        break;
      }
      case "task.removed":
        // Déjà absente : rien à redessiner, rien de perdu.
        if (this.tasks.delete(params.taskId)) result.list = true;
        break;
      case "queue.updated":
        this.queues.set(params.queue.queueId, params.queue);
        result.planning = true;
        // L'ouverture de la file change l'état affiché de ses tâches en attente.
        this.touchQueue(params.queue.queueId, result);
        break;
      case "schedule.updated":
        this.schedules.set(params.schedule.scheduleId, params.schedule);
        result.planning = true;
        for (const q of this.queues.values()) {
          if (q.scheduleId === params.schedule.scheduleId) this.touchQueue(q.queueId, result);
        }
        break;
      case "schedule.removed":
        this.schedules.delete(params.scheduleId);
        result.planning = true;
        break;
      case "engine.endAction":
        this.endAction = params.pending ?? null;
        result.endActionError = params.error ?? null;
        result.planning = true;
        break;
      case "limits.updated": {
        const { rev: _rev, ...limits } = params;
        this.limits = limits;
        result.limits = true;
        break;
      }
      case "settings.updated":
        Object.assign(this.settings, params.values);
        result.settings = true;
        break;
    }
    return result;
  }

  /// Redessine les tâches d'une file.
  touchQueue(queueId, result) {
    for (const task of this.tasks.values()) {
      if (task.queueId === queueId) result.changed.add(task.taskId);
    }
  }

  /// Planification d'une file, ou `null`.
  scheduleOf(queueId) {
    const id = this.queues.get(queueId)?.scheduleId;
    return id ? this.schedules.get(id) ?? null : null;
  }

  /// La file attend sa plage horaire : planifiée, activée et fermée.
  waitsForWindow(queueId) {
    const s = this.scheduleOf(queueId);
    return Boolean(s && s.enabled && !s.open);
  }

  /// Tâches d'une vue de l'arborescence, triées.
  list(view, column = "added", descending = true) {
    return sortTasks([...this.tasks.values()].filter((t) => inView(t, view)), column, descending);
  }

  /// Nombre de tâches de chaque vue : branches, branches par type, files.
  counts() {
    const counts = {};
    const add = (view) => { counts[view] = (counts[view] ?? 0) + 1; };
    for (const task of this.tasks.values()) {
      const cat = category(taskFileName(task));
      for (const group of GROUPS) {
        if (!inGroup(task, group)) continue;
        add(group);
        add(`${group}/${cat}`);
      }
      if (inGroup(task, "incomplete")) add(`queue:${task.queueId}`);
    }
    return counts;
  }

  /// Débit total et nombre de tâches en cours de transfert.
  activity() {
    let bps = 0;
    let active = 0;
    for (const task of this.tasks.values()) {
      if (task.state !== "active") continue;
      active++;
      bps += task.bps ?? 0;
    }
    return { active, bps };
  }
}

// ---------- Historique (F-35) ----------

/// Champ CSV (RFC 4180) : entre guillemets s'il contient le séparateur, un guillemet ou un
/// saut de ligne, les guillemets doublés.
export function csvField(text, sep) {
  const s = text == null ? "" : String(text);
  return s.includes(sep) || /["\r\n]/.test(s) ? `"${s.replaceAll('"', '""')}"` : s;
}

/// Date et heure locales « 2026-11-08 10:30:12 » : se trie comme du texte, et les tableurs
/// la reconnaissent.
export function localStamp(ms) {
  const d = new Date(ms);
  const two = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${two(d.getMonth() + 1)}-${two(d.getDate())} `
    + `${two(d.getHours())}:${two(d.getMinutes())}:${two(d.getSeconds())}`;
}

/// Historique en CSV : en-têtes `headers` (déjà traduits), une ligne par entrée ; tailles en
/// octets, états par `stateLabel`. Le point-virgule par défaut est celui qu'attendent les
/// tableurs réglés en français ; la marque d'ordre des octets leur fait lire l'UTF-8.
export function historyCsv(items, { headers, stateLabel, sep = ";" }) {
  const line = (fields) => fields.map((f) => csvField(f, sep)).join(sep);
  const lines = [line(headers)];
  for (const h of items) {
    lines.push(line([
      localStamp(h.finishedAt), stateLabel(h.finalState), h.totalBytes ?? "", h.displayName,
      h.sourceHost ?? "", h.finalPath ?? "", h.sha256 ?? "",
    ]));
  }
  return `\ufeff${lines.join("\r\n")}\r\n`;
}

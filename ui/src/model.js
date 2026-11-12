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
// `seeding` : arrêter un torrent qui partage le termine (SPEC §7.16), ses fichiers restent.
export const canPause = (state) => ["queued", "active", "retry_wait", "verifying", "seeding"].includes(state);
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
/// Seulement un fichier HTTP : un torrent n'a ni adresse à actualiser, ni nom ou dossier à
/// changer (SPEC §6.5) ; une vidéo non plus (sous-titres, adresse de la page, SPEC §7.17).
export const canRelocate = (task) =>
  (task.kind ?? "http") === "http" && !task.moving
  && ["queued", "paused", "retry_wait", "failed", "cancelled", "completed"].includes(task.state);
/// Empreinte attendue (F-11) : modifiable tant que le fichier n'est ni terminé, ni annulé, ni en
/// cours de vérification. Un torrent vérifie ses morceaux lui-même ; une vidéo est assemblée
/// sur place, sans empreinte publiée.
export const canSetChecksum = (task) =>
  (task.kind ?? "http") === "http" && ["queued", "active", "paused", "retry_wait", "failed"].includes(task.state);
/// Fichiers d'un torrent à choisir (`task.setFiles`, SPEC §6.5) : pas une fois reçu ni annulé.
export const canChooseFiles = (task) =>
  task.kind === "torrent" && ["queued", "active", "paused", "retry_wait", "failed"].includes(task.state);
/// Un torrent qui partage a fini de recevoir : supprimé, il garde ses fichiers sauf demande,
/// comme un téléchargement terminé.
export const isFinishedDownload = (task) => task.state === "completed" || task.state === "seeding";

/// Lien magnet (BitTorrent).
export const isMagnet = (text) => /^magnet:\?/i.test(text.trim());

/// Ratio de partage d'un torrent : octets envoyés sur sa taille ; `null` s'il est inconnu.
export function shareRatio(task) {
  if (task.uploadedBytes == null || !(task.totalBytes > 0)) return null;
  return task.uploadedBytes / task.totalBytes;
}

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

/// Sources d'une tâche HTTP, la référence comprise (SPEC §7.7).
export const MAX_SOURCES = 16;

/// Nom de fichier Metalink (F-27, SPEC §7.7) : `.meta4` ou `.metalink`.
export const isMetalinkName = (name) => /\.(meta4|metalink)$/i.test((name ?? "").trim());

/// Adresse HTTP(S) d'un fichier Metalink : son chemin finit par `.meta4` ou `.metalink`
/// (paramètres et ancre exclus). Le moteur la lit lui-même (`metalink.read`).
export const isMetalinkUrl = (text) =>
  /^https?:\/\/[^?#\s]*\.(meta4|metalink)([?#]\S*)?$/i.test((text ?? "").trim());

/// Adresse qu'un plugin d'hébergeur pourrait lire (SPEC §7.20) : une page HTTP(S), pas un
/// Metalink. Le moteur dit lui-même quel plugin la prend en charge (`plugin.match`).
export const pluginCandidate = (url) => /^https?:\/\//i.test(url ?? "") && !isMetalinkUrl(url);

/// Nom d'un lien rendu par un plugin : le sien, sinon la fin de son adresse, sinon l'adresse.
export const pluginLinkName = (link) =>
  link.name || link.url.split(/[?#]/)[0].split("/").pop() || link.url;

/// Adresse HTTP ou HTTPS : la seule que prend un miroir.
const isWebUrl = (text) => /^https?:\/\/\S+$/i.test(text);

/// Miroirs écrits dans la boîte « Ajouter » (SPEC §7.7), un par ligne, pour les adresses de
/// la première zone : ils ne valent que pour une seule adresse HTTP. Rend `{ mirrors }`
/// (liste vide sans miroir) ou `{ error }` : `many` (plusieurs adresses ou fichiers),
/// `torrent`, `metalink` (il donne ses miroirs lui-même), `scheme` (pas une adresse HTTP), `twice` (adresse donnée deux fois), `count`
/// (plus de 16 sources) ; `url` est alors l'adresse en cause.
export function parseMirrors(text, urls, torrents = 0, metalinks = 0) {
  const mirrors = (text ?? "").split(/\s+/).filter(Boolean);
  if (!mirrors.length) return { mirrors };
  if (urls.length + torrents + metalinks > 1) return { error: "many" };
  if (metalinks > 0 || urls.some(isMetalinkUrl)) return { error: "metalink" };
  if (torrents > 0 || urls.some(isMagnet)) return { error: "torrent" };
  const bad = mirrors.find((m) => !isWebUrl(m));
  if (bad) return { error: "scheme", url: bad };
  const seen = new Set(urls);
  for (const m of mirrors) {
    if (seen.has(m)) return { error: "twice", url: m };
    seen.add(m);
  }
  if (urls.length + mirrors.length > MAX_SOURCES) return { error: "count" };
  return { mirrors };
}

/// Miroir à ajouter dans les Propriétés (`task.addSource`) : un fichier HTTP pas encore fini
/// ni annulé ; une vidéo ou un torrent n'a qu'une source.
export const canAddMirror = (task) =>
  (task.kind ?? "http") === "http" && ["queued", "active", "paused", "retry_wait", "failed"].includes(task.state);

/// Nom court d'une source : « principale » pour la référence, sinon « miroir N » (son rang).
/// Rend la clé du texte et son numéro, à traduire par l'appelant.
export const sourceName = (source) => (source.rank === 0 ? ["Principale"] : ["Miroir {0}", source.rank]);

/// Textes des états d'une source (SPEC §6.5), comme ceux de la CLI.
export const SOURCE_STATES = {
  unknown: "À vérifier",
  ok: "Bonne",
  slow: "Lente",
  failed: "Écartée",
};

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
    /// Avertissements du moteur, à montrer : `{code, message, taskId}` (SPEC §6.5).
    warnings: [],
    /// L'interface réseau des torrents est apparue ou a disparu (SPEC §7.16).
    torrentInterface: false,
    /// L'état des outils de la vidéo a changé (SPEC §7.17).
    tools: false,
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
  into.moveErrors.push(...other.moveErrors);
  into.warnings.push(...other.warnings);
  into.torrentInterface ||= other.torrentInterface;
  into.tools ||= other.tools;
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
    /// L'interface réseau des torrents est là, ou aucune n'est demandée (SPEC §7.16).
    this.torrentInterfaceUp = true;
    /// État des outils de la vidéo (`media.tools`), une fois annoncé ; `null` avant.
    this.tools = null;
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
    this.torrentInterfaceUp = snapshot.torrentInterfaceUp ?? true;
    this.rev = snapshot.rev;
    const result = outcome();
    result.list = true;
    result.limits = true;
    result.settings = true;
    result.planning = true;
    result.torrentInterface = true;
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
        // Notifications perdues : propre à la connexion, sans révision.
        if (params.code === "EVENTS_LOST") {
          result.resync = true;
          return result;
        }
        // Les autres (yt-dlp mis à jour, sous-titres manquants…) sont numérotées.
        break;
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
      case "engine.torrentInterface":
      case "media.tools":
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
      case "engine.torrentInterface":
        this.torrentInterfaceUp = params.up !== false;
        result.torrentInterface = true;
        break;
      case "media.tools": {
        const { rev: _rev, ...tools } = params;
        this.tools = tools;
        result.tools = true;
        break;
      }
      case "engine.warning":
        result.warnings.push({ code: params.code, message: params.message, taskId: params.taskId ?? null });
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

// ---------- Vidéo (SPEC §7.17) ----------

/// Un format a de l'image (codec connu, ou codecs inconnus mais une hauteur : yt-dlp le
/// prend pour complet).
const hasVideo = (f) => Boolean(f.vcodec) || (!f.vcodec && !f.acodec && f.height > 0);
/// Un format n'a que du son.
const audioOnly = (f) => !f.vcodec && Boolean(f.acodec);

/// Taille d'un format : `{bytes, approx}`, ou `null` si inconnue.
const sizeOf = (f) => (f.filesize > 0 ? { bytes: f.filesize, approx: Boolean(f.filesizeApprox) } : null);

function addSizes(a, b) {
  if (!a) return null;
  if (!b) return a;
  return { bytes: a.bytes + b.bytes, approx: a.approx || b.approx };
}

/// Qualités proposées après l'analyse d'une page (`media.probe`) : « meilleure », chaque
/// hauteur d'image (de la plus grande à la plus petite), puis « son seul » s'il existe.
/// Chacune porte son sélecteur de yt-dlp (`null` : celui du moteur) et une taille estimée
/// (image et meilleur son), ou `null`. Les formats protégés (DRM) sont écartés.
export function qualityChoices(probe) {
  const formats = (probe?.formats ?? []).filter((f) => !f.drm);
  const audios = formats.filter(audioOnly);
  const bestAudio = audios.map(sizeOf).filter(Boolean).sort((a, b) => b.bytes - a.bytes)[0] ?? null;
  const heights = [...new Set(formats.filter((f) => hasVideo(f) && f.height > 0).map((f) => f.height))]
    .sort((a, b) => b - a);
  const sizeAt = (height) => {
    const sizes = formats
      .filter((f) => hasVideo(f) && f.height === height)
      .map((f) => {
        const own = sizeOf(f);
        // Image seule : le son s'y ajoute.
        return f.vcodec && !f.acodec ? addSizes(own, bestAudio) : own;
      })
      .filter(Boolean)
      .sort((a, b) => b.bytes - a.bytes);
    return sizes[0] ?? null;
  };
  const choices = [{ key: "best", format: null, height: null, size: heights.length ? sizeAt(heights[0]) : null }];
  for (const height of heights) {
    choices.push({
      key: `h${height}`,
      format: `bv*[height<=${height}]+ba/b[height<=${height}]`,
      height,
      size: sizeAt(height),
    });
  }
  if (audios.length) choices.push({ key: "audio", format: "ba", height: null, size: bestAudio });
  return choices;
}

/// Sélecteur d'un format précis choisi dans le détail : une image seule reçoit le meilleur
/// son, s'il y en a un à part.
export function exactFormat(format, probe) {
  const withAudio = format.vcodec && !format.acodec && (probe?.formats ?? []).some((f) => !f.drm && audioOnly(f));
  return withAudio ? `${format.formatId}+ba` : format.formatId;
}

/// Ce que contient un format, pour l'affichage : `av`, `v`, `a`, ou `?` (codecs inconnus).
export function formatContent(f) {
  if (f.vcodec && f.acodec) return "av";
  if (f.vcodec) return "v";
  if (f.acodec) return "a";
  return "?";
}

// ---------- Analyse de liens (F-29, F-31, SPEC §7.18) ----------

/// Extensions des pages web, comme celles du moteur : un lien vers elles n'est pas un fichier.
const PAGE_EXTENSIONS = new Set(["htm", "html", "xhtml", "shtml", "php", "php3", "php5", "asp", "aspx", "jsp", "jspx", "cgi", "pl", "cfm", "do", "action", "py"]);

/// Ce que demande la zone « Analyser des liens » : une seule adresse HTTP(S) qui n'a pas
/// l'air d'un fichier est une page à lire (`{pageUrl}`) ; tout le reste est un texte dont
/// on relève les adresses (`{text}`) ; `null` si la zone est vide.
export function scanRequest(value) {
  const text = (value ?? "").trim();
  if (!text) return null;
  if (/^https?:\/\/\S+$/i.test(text)) {
    const path = text.split(/[?#]/)[0].replace(/^https?:\/\/[^/]*/i, "");
    const last = path.split("/").pop() ?? "";
    const dot = last.lastIndexOf(".");
    const ext = dot > 0 ? last.slice(dot + 1).toLowerCase() : "";
    if (!ext || PAGE_EXTENSIONS.has(ext) || !/^[a-z0-9]{1,10}$/.test(ext)) return { pageUrl: text };
  }
  return { text };
}

/// Une taille écrite : « 800000 », « 500K », « 2M », « 1,5G » (unités binaires, comme la
/// CLI) ; `null` pour un champ vide, `NaN` si elle est illisible.
export function parseSizeText(text) {
  const value = (text ?? "").trim();
  if (!value) return null;
  const match = /^(\d+(?:[.,]\d+)?)\s*([kmgt]?)(?:i?o|i?b)?$/i.exec(value);
  if (!match) return NaN;
  const factor = { "": 1, k: 1024, m: 1024 ** 2, g: 1024 ** 3, t: 1024 ** 4 }[match[2].toLowerCase()];
  return Math.round(Number(match[1].replace(",", ".")) * factor);
}

/// Filtres de l'analyse de liens, lus des champs de la boîte : rend `{ keep(link, size) }`, ou
/// `{ error }` (`min`, `max` : taille illisible ; `regex` : expression illisible, avec
/// `message`). Mêmes règles que `auro links` : pages masquées sauf `all`, extensions sans
/// point ni casse, expression cherchée dans l'adresse (casse comprise), et un filtre de
/// taille écarte un lien de taille inconnue.
export function linkFilter({ extensions = "", min = "", max = "", regex = "", all = false } = {}) {
  const exts = extensions.split(/[\s,;]+/).map((e) => e.trim().replace(/^\./, "").toLowerCase()).filter(Boolean);
  const minSize = parseSizeText(min);
  const maxSize = parseSizeText(max);
  if (Number.isNaN(minSize)) return { error: "min" };
  if (Number.isNaN(maxSize)) return { error: "max" };
  let pattern = null;
  if (regex.trim()) {
    try {
      pattern = new RegExp(regex.trim());
    } catch (e) {
      return { error: "regex", message: e.message };
    }
  }
  const sized = minSize !== null || maxSize !== null;
  return {
    sized,
    keep(link, size) {
      if (!all && link.kind === "page") return false;
      if (exts.length && !exts.includes(link.ext ?? "")) return false;
      if (pattern && !pattern.test(link.url)) return false;
      if (!sized) return true;
      if (size == null) return false;
      return (minSize === null || size >= minSize) && (maxSize === null || size <= maxSize);
    },
  };
}

/// Un lien classé « page » (sans extension, ou extension de page web) que le serveur décrit
/// comme un fichier (`linkscan.probe`, `kind`) devient un fichier (décision du 2026-11-11) ;
/// jamais l'inverse : un fichier qui rend une page d'erreur reste un fichier.
export const reclassifyLink = (link, probedKind) =>
  link.kind === "page" && probedKind === "file" ? { ...link, kind: "file" } : link;

// --- Interface distante (SPEC §7.19, étape 3c) ---

/// Jeton apporté par l'ancre de l'adresse (`#token=auro_…`, QR code du bureau) ; null sans
/// lui. L'ancre n'est jamais envoyée au serveur.
export function takeHashToken(hash) {
  const params = new URLSearchParams((hash ?? "").replace(/^#/, ""));
  const token = params.get("token");
  return token && /^auro_[A-Za-z0-9_-]{43}$/.test(token) ? token : null;
}

/// Un jeton complet : `auro_` suivi de 43 caractères base64url.
export const isToken = (text) => /^auro_[A-Za-z0-9_-]{43}$/.test(text ?? "");

/// Le jeton contenu dans un texte collé dans la connexion : le jeton seul, l'adresse du QR
/// code (`…#token=auro_…`) ou une ligne qui le contient ; sinon le texte, sans ses espaces.
export function pastedToken(text) {
  const found = (text ?? "").match(/auro_[A-Za-z0-9_-]{43}(?![A-Za-z0-9_-])/);
  return found ? found[0] : (text ?? "").trim();
}

/// Erreur d'une connexion refusée (`/login`), d'après son statut HTTP.
export function sessionError(status) {
  if (status === 401) return { code: "UNAUTHORIZED", message: "jeton refusé" };
  if (status === 429) return { code: "TOO_MANY_ATTEMPTS", message: "trop d'essais refusés" };
  return { code: "ENGINE_DOWN", message: `serveur : ${status}` };
}

/// Sous-dossier saisi dans la page distante, tel que `task.add` l'attend (`subDir`) : parties
/// séparées par `/` (ou `\`), sans vide ; null s'il contient `.` ou `..` (refusé), "" s'il
/// est vide.
export function subDirParts(text) {
  const parts = (text ?? "").split(/[\\/]+/).map((p) => p.trim()).filter((p) => p !== "");
  if (parts.some((p) => p === "." || p === "..")) return null;
  return parts.join("/");
}

/// Ce que permet une portée (SPEC §6.9) : `read` lit, `add` ajoute, `control` lit, ajoute
/// et agit sur les tâches, `admin` règle aussi le moteur.
export function scopeAllows(scope, need) {
  if (scope === "admin") return true;
  if (scope === "control") return need !== "admin";
  return scope === need;
}

// ---------- Accès à distance (SPEC §7.19, étape 3d) ----------

/// Le réglage `remote.listen` (`127.0.0.1:7380`, `[fd00::20]:7380`) en adresse et port ;
/// null s'il n'a pas cette forme.
export function splitListen(text) {
  const found = /^(?:\[([^\]]+)\]|([^:[\]]+)):(\d{1,5})$/.exec(text ?? "");
  if (!found) return null;
  const port = Number(found[3]);
  return port >= 1 && port <= 65535 ? { host: found[1] ?? found[2], port } : null;
}

/// L'inverse : une adresse IPv6 entre crochets.
export function joinListen(host, port) {
  return host.includes(":") ? `[${host}]:${port}` : `${host}:${port}`;
}

/// L'adresse de la page (`https://192.168.1.20:7380`, une des `urls` de `remote.status`)
/// avec le jeton dans l'ancre, que le navigateur n'envoie jamais au serveur.
export function pageLink(url, token) {
  return `${url.replace(/\/+$/, "")}/#token=${token}`;
}

/// Une adresse de la page qui ne se joint que depuis cet ordinateur.
export function isLoopbackUrl(url) {
  let host;
  try {
    host = new URL(url).hostname.replace(/^\[|\]$/g, "").toLowerCase();
  } catch {
    return false;
  }
  return host === "localhost" || host === "::1" || /^127\./.test(host);
}

/// Adresses qu'aura la page une fois l'accès ouvert, d'après les réglages (même règle que
/// le moteur : HTTP sur la boucle locale, HTTPS ailleurs ; pour une écoute sur toutes les
/// adresses, celles des interfaces ; puis les noms de `remote.hosts`). Le QR code d'un
/// appareil ajouté avant l'ouverture sert ainsi quand elle viendra.
export function plannedUrls(settings, addresses) {
  const listen = splitListen(settings["remote.listen"]) ?? { host: "127.0.0.1", port: 7380 };
  const loopback = listen.host === "localhost" || listen.host === "::1" || /^127\./.test(listen.host);
  const scheme = loopback ? "http" : "https";
  const at = (host) => `${scheme}://${joinListen(host, listen.port)}`;
  let hosts;
  if (listen.host === "0.0.0.0" || listen.host === "::") {
    hosts = (addresses ?? []).map((a) => a.address).filter((ip) => listen.host === "::" || !ip.includes(":"));
  } else {
    hosts = [listen.host];
  }
  const urls = hosts.map(at);
  for (const name of settings["remote.hosts"] ?? []) {
    const url = name.includes(":") ? at(name) : `${scheme}://${name}:${listen.port}`;
    if (!urls.includes(url)) urls.push(url);
  }
  return urls;
}

/// Adresses de la page à proposer pour le QR code : celles du réseau d'abord.
export function qrUrls(urls) {
  return [...(urls ?? [])].sort((a, b) => Number(isLoopbackUrl(a)) - Number(isLoopbackUrl(b)));
}

/// Tracé SVG des modules foncés d'un QR code (`{width, modules}` de l'hôte), décalé de la
/// marge de 4 modules qu'exige la norme.
export function qrPath(width, modules) {
  let d = "";
  for (let y = 0; y < width; y++) {
    for (let x = 0; x < width; x++) {
      if (modules[y * width + x] === "1") d += `M${x + 4} ${y + 4}h1v1h-1z`;
    }
  }
  return d;
}

/// L'écoute suit-elle les réglages enregistrés ? (Le moteur l'ajuste un instant après
/// `settings.set` ; même règle que `auro remote`.)
export function remoteSettled(status, settings) {
  if (!status) return false;
  const wanted = settings["remote.enabled"] === true;
  const applied = status.enabled === wanted
    && (status.listen ?? null) === (settings["remote.listen"] ?? null)
    && JSON.stringify(status.hosts ?? []) === JSON.stringify(settings["remote.hosts"] ?? [])
    && (status.tlsCert ?? null) === (settings["remote.tlsCert"] ?? null);
  const settled = wanted ? status.listening === true || typeof status.error === "string" : !status.listening;
  return applied && settled;
}

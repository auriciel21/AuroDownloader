// État des tâches tenu à jour par `sync.snapshot` et les notifications du moteur
// (SPEC §6.5, §6.10). Sans lien avec la page : testé seul (ui/tests).
//
// Règles de la SPEC §6.10 :
// - s'abonner, puis charger l'instantané ; ignorer les notifications de révision
//   inférieure ou égale à la sienne ;
// - `rev` augmente de 1 à chaque changement (tâches et files, les deux sujets suivis) :
//   un trou signale une notification perdue, il faut recharger l'instantané ;
// - l'avancement porte la révision courante sans l'augmenter.

/// Groupes d'états des filtres de la liste.
export const GROUPS = {
  queued: "active",
  active: "active",
  retry_wait: "active",
  verifying: "active",
  post_processing: "active",
  seeding: "active",
  paused: "stopped",
  failed: "stopped",
  completed: "completed",
  cancelled: "cancelled",
};

export const FILTERS = ["all", "active", "stopped", "completed", "cancelled"];

/// La tâche apparaît sous ce filtre ; « toutes » omet les annulées.
export function inFilter(task, filter) {
  if (filter === "all") return task.state !== "cancelled";
  return GROUPS[task.state] === filter;
}

// Actions permises par la table des états (SPEC §5.5).
export const canPause = (state) => ["queued", "active", "retry_wait"].includes(state);
export const canResume = (state) => ["paused", "failed"].includes(state);
export const canCancel = (state) => ["queued", "active", "paused", "retry_wait", "seeding"].includes(state);
// Retrait de la liste (`task.remove`) : tâches finies seulement.
export const canRemove = (state) => ["completed", "failed", "cancelled"].includes(state);

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
  };
}

function merge(into, other) {
  for (const id of other.changed) into.changed.add(id);
  into.list ||= other.list;
  into.resync ||= other.resync;
  into.limits ||= other.limits;
  into.finished.push(...other.finished);
}

/// Tâche telle que la liste la garde : l'avancement manque à l'instantané.
function fresh(task) {
  return { bps: 0, etaS: null, connections: 0, ...task };
}

export class TaskStore {
  constructor() {
    this.tasks = new Map();
    this.queues = new Map();
    /// Limites globales (`limits.get`) ; `null` avant le premier instantané.
    this.limits = null;
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
    // Un moteur antérieur au 2026-11-07 ne les donne pas.
    this.limits = snapshot.limits ?? null;
    this.rev = snapshot.rev;
    const result = outcome();
    result.list = true;
    result.limits = true;
    for (const id of this.tasks.keys()) result.changed.add(id);
    const waiting = this.waiting;
    this.waiting = [];
    for (const { method, params } of waiting) merge(result, this.apply(method, params));
    return result;
  }

  /// Applique une notification du moteur.
  apply(method, params) {
    const result = outcome();
    if (this.rev === null) {
      this.waiting.push({ method, params });
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
        if (params.to === "completed" && task.totalBytes > 0) task.doneBytes = task.totalBytes;
        result.changed.add(params.taskId);
        result.list = GROUPS[params.from] !== GROUPS[params.to];
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
        const { rev: _rev, taskId: _id, ...fields } = params;
        Object.assign(task, fields);
        result.changed.add(params.taskId);
        break;
      }
      case "task.removed":
        // Déjà absente : rien à redessiner, rien de perdu.
        if (this.tasks.delete(params.taskId)) result.list = true;
        break;
      case "queue.updated":
        this.queues.set(params.queue.queueId, params.queue);
        break;
      case "limits.updated": {
        const { rev: _rev, ...limits } = params;
        this.limits = limits;
        result.limits = true;
        break;
      }
    }
    return result;
  }

  /// Tâches du filtre, les plus récentes d'abord (identifiants UUID v7, ordonnés dans le
  /// temps).
  list(filter) {
    return [...this.tasks.values()]
      .filter((t) => inFilter(t, filter))
      .sort((a, b) => (a.taskId < b.taskId ? 1 : a.taskId > b.taskId ? -1 : 0));
  }

  /// Nombre de tâches de chaque filtre.
  counts() {
    const counts = Object.fromEntries(FILTERS.map((f) => [f, 0]));
    for (const task of this.tasks.values()) {
      for (const f of FILTERS) if (inFilter(task, f)) counts[f]++;
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

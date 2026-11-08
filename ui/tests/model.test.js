// Révisions et état des tâches (SPEC §6.10).
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  TaskStore, inView, category, sortTasks, canCancel, canPause, canRemove, canResume, canRestart,
  canRelocate,
} from "../src/model.js";

const task = (id, state = "queued", extra = {}) => ({
  taskId: id, state, fileName: `${id}.bin`, doneBytes: 0, totalBytes: 100, pauseReason: null, errorCode: null, ...extra,
});
const snapshot = (rev, tasks) => ({ rev, tasks, queues: [{ queueId: "q", name: "default" }] });

test("les notifications reçues avant l'instantané attendent, les plus anciennes sont écartées", () => {
  const store = new TaskStore();
  store.reset();
  // Arrivent avant la réponse de sync.snapshot.
  store.apply("task.stateChanged", { rev: 5, taskId: "a", from: "queued", to: "active" });
  store.apply("task.added", { rev: 6, task: task("b") });
  store.apply("task.progress", { rev: 6, items: [{ taskId: "a", doneBytes: 40, bps: 10, etaS: 6, connections: 2 }] });
  assert.equal(store.tasks.size, 0, "rien n'est appliqué sans instantané");

  const result = store.load(snapshot(5, [task("a", "active", { doneBytes: 30 })]));
  assert.equal(store.rev, 6);
  assert.deepEqual([...store.tasks.keys()].sort(), ["a", "b"]);
  assert.equal(store.tasks.get("a").doneBytes, 40);
  assert.equal(store.tasks.get("a").connections, 2);
  assert.ok(result.list && !result.resync);
});

test("une révision manquante demande de recharger l'instantané", () => {
  const store = new TaskStore();
  store.load(snapshot(10, [task("a")]));
  assert.ok(!store.apply("task.stateChanged", { rev: 11, taskId: "a", from: "queued", to: "active" }).resync);
  const gap = store.apply("task.stateChanged", { rev: 13, taskId: "a", from: "active", to: "paused", pauseReason: "user" });
  assert.ok(gap.resync);
  assert.equal(store.tasks.get("a").state, "paused", "appliquée quand même");
  assert.equal(store.rev, 13);
});

test("les révisions déjà chargées et l'avancement périmé sont ignorés", () => {
  const store = new TaskStore();
  store.load(snapshot(10, [task("a", "active", { doneBytes: 50 })]));
  const old = store.apply("task.stateChanged", { rev: 10, taskId: "a", from: "queued", to: "active" });
  assert.equal(old.changed.size, 0);
  store.apply("task.progress", { rev: 9, items: [{ taskId: "a", doneBytes: 20 }] });
  assert.equal(store.tasks.get("a").doneBytes, 50, "avancement antérieur à l'instantané");
  store.apply("task.progress", { rev: 10, items: [{ taskId: "a", doneBytes: 60 }, { taskId: "inconnue", doneBytes: 1 }] });
  assert.equal(store.tasks.get("a").doneBytes, 60);
  assert.ok(!store.tasks.has("inconnue"));
});

test("un changement d'état remet à zéro l'avancement et annonce la fin", () => {
  const store = new TaskStore();
  store.load(snapshot(1, [task("a", "active")]));
  store.apply("task.progress", { rev: 1, items: [{ taskId: "a", doneBytes: 90, bps: 5, etaS: 2, connections: 4 }] });
  const done = store.apply("task.stateChanged", { rev: 2, taskId: "a", from: "active", to: "completed" });
  const a = store.tasks.get("a");
  assert.equal(a.state, "completed");
  assert.equal(a.doneBytes, 100);
  assert.equal(a.bps, 0);
  assert.equal(a.etaS, null);
  assert.ok(done.list, "passe d'un filtre à l'autre");
  assert.deepEqual(done.finished, [{ taskId: "a", to: "completed" }]);

  store.load(snapshot(3, [task("b", "active")]));
  const failed = store.apply("task.stateChanged", { rev: 4, taskId: "b", from: "active", to: "failed", errorCode: "NET_TIMEOUT" });
  assert.equal(store.tasks.get("b").errorCode, "NET_TIMEOUT");
  assert.deepEqual(failed.finished, [{ taskId: "b", to: "failed" }]);
  store.apply("task.stateChanged", { rev: 5, taskId: "b", from: "failed", to: "queued" });
  assert.equal(store.tasks.get("b").errorCode, null, "relancée : plus d'erreur");
});

test("une tâche inconnue, ou des événements perdus, demandent de recharger", () => {
  const store = new TaskStore();
  store.load(snapshot(1, []));
  assert.ok(store.apply("task.stateChanged", { rev: 2, taskId: "x", from: "queued", to: "active" }).resync);
  assert.ok(store.apply("task.updated", { rev: 3, taskId: "x", speedLimitBps: 5 }).resync);
  assert.ok(store.apply("engine.warning", { rev: 3, code: "EVENTS_LOST", message: "" }).resync);
  assert.ok(!store.apply("engine.warning", { rev: 3, code: "AUTRE", message: "" }).resync);
});

test("task.updated ne change que les champs annoncés ; queue.updated suit la file", () => {
  const store = new TaskStore();
  store.load(snapshot(1, [task("a", "active", { speedLimitBps: null })]));
  store.apply("task.updated", { rev: 2, taskId: "a", speedLimitBps: 1024 });
  assert.equal(store.tasks.get("a").speedLimitBps, 1024);
  assert.equal(store.tasks.get("a").state, "active");
  assert.ok(!("rev" in store.tasks.get("a")));
  store.apply("queue.updated", { rev: 3, queue: { queueId: "q", name: "default", maxActive: 5 } });
  assert.equal(store.queues.get("q").maxActive, 5);
  assert.equal(store.rev, 3);
});

test("un nouvel instantané après reconnexion repart de sa révision, même plus petite", () => {
  const store = new TaskStore();
  store.load(snapshot(40, [task("a")]));
  // Le moteur a redémarré : sa révision repart de 0.
  store.reset();
  store.load(snapshot(2, [task("a", "paused")]));
  assert.equal(store.rev, 2);
  assert.ok(!store.apply("task.stateChanged", { rev: 3, taskId: "a", from: "paused", to: "queued" }).resync);
});

test("une tâche retirée quitte la liste, dans l'ordre des révisions", () => {
  const store = new TaskStore();
  store.load(snapshot(1, [task("a", "completed"), task("b", "failed")]));
  const removed = store.apply("task.removed", { rev: 2, taskId: "a" });
  assert.ok(removed.list && !removed.resync);
  assert.deepEqual([...store.tasks.keys()], ["b"]);
  // Déjà absente (instantané plus récent) : sans effet, sans rechargement.
  const again = store.apply("task.removed", { rev: 3, taskId: "a" });
  assert.ok(!again.list && !again.resync);
  assert.equal(store.rev, 3);
  // Une révision sautée reste un trou.
  assert.ok(store.apply("task.removed", { rev: 5, taskId: "b" }).resync);
  assert.equal(store.tasks.size, 0);
});

test("les limites globales viennent de l'instantané, puis de limits.updated", () => {
  const store = new TaskStore();
  store.load({ ...snapshot(4, []), limits: { downloadBps: null, uploadBps: null } });
  assert.deepEqual(store.limits, { downloadBps: null, uploadBps: null });
  const changed = store.apply("limits.updated", { rev: 5, downloadBps: 2048, uploadBps: null });
  assert.ok(changed.limits && !changed.resync);
  assert.deepEqual(store.limits, { downloadBps: 2048, uploadBps: null });
  // Déjà comprise dans un instantané plus récent : ignorée.
  store.load({ ...snapshot(9, []), limits: { downloadBps: 4096, uploadBps: null } });
  assert.ok(!store.apply("limits.updated", { rev: 9, downloadBps: 1, uploadBps: null }).limits);
  assert.equal(store.limits.downloadBps, 4096);
  // Moteur plus ancien : pas de limites dans l'instantané.
  store.load(snapshot(1, []));
  assert.equal(store.limits, null);
});

test("vues de l'arborescence, compteurs et débit", () => {
  const store = new TaskStore();
  store.load(snapshot(1, [
    task("0001", "completed", { fileName: "film.mkv" }), task("0002", "active"), task("0003", "cancelled"),
    task("0004", "failed", { fileName: null, displayName: "notes.PDF" }), task("0005", "paused"),
    task("0006", "retry_wait", { queueId: "q" }),
  ]));
  store.apply("task.progress", { rev: 1, items: [{ taskId: "0002", bps: 300 }] });
  const ids = (view) => store.list(view).map((t) => t.taskId);
  assert.deepEqual(ids("all"), ["0006", "0005", "0004", "0003", "0002", "0001"]);
  assert.deepEqual(ids("incomplete"), ["0006", "0005", "0004", "0002"], "ni terminée ni annulée");
  assert.deepEqual(ids("completed/video"), ["0001"]);
  assert.deepEqual(ids("incomplete/documents"), ["0004"], "nom provisoire avant préparation");
  assert.deepEqual(ids("queue:q"), ["0006"]);
  const counts = store.counts();
  assert.equal(counts.all, 6);
  assert.equal(counts.incomplete, 4);
  assert.equal(counts["completed/video"], 1);
  assert.equal(counts["all/other"], 4);
  assert.equal(counts["queue:q"], 1);
  assert.equal(counts["completed/music"], undefined);
  assert.deepEqual(store.activity(), { active: 1, bps: 300 });
  assert.ok(inView(task("x", "cancelled"), "all") && !inView(task("x", "cancelled"), "incomplete"));
});

test("type de fichier d'après l'extension", () => {
  assert.equal(category("a.ZIP"), "compressed");
  assert.equal(category("a.part1.rar"), "compressed");
  assert.equal(category("a.r01"), "compressed");
  assert.equal(category("a.7z.001"), "compressed");
  assert.equal(category("Blender-3.2.msi"), "programs");
  assert.equal(category("son.flac"), "music");
  assert.equal(category("film.webm"), "video");
  assert.equal(category("lisez-moi"), "other");
  assert.equal(category(".bashrc"), "other");
  assert.equal(category(""), "other");
});

test("tri par colonne, les plus récentes d'abord à égalité", () => {
  const tasks = [
    task("0001", "completed", { fileName: "b10.bin", totalBytes: 10, createdAt: 3 }),
    task("0002", "active", { fileName: "b9.bin", totalBytes: 500, doneBytes: 250, bps: 7, etaS: 30, createdAt: 1 }),
    task("0003", "paused", { fileName: "A.bin", totalBytes: null, createdAt: 2 }),
    task("0004", "active", { fileName: "c.bin", totalBytes: 100, doneBytes: 50, bps: 7, etaS: 5, createdAt: 2 }),
  ];
  const ids = (...args) => sortTasks(tasks, ...args).map((t) => t.taskId);
  assert.deepEqual(ids("name", false), ["0003", "0002", "0001", "0004"], "sans casse, nombres dans l'ordre");
  assert.deepEqual(ids("size", true), ["0002", "0004", "0001", "0003"], "taille inconnue en dernier");
  assert.deepEqual(ids("progress", true), ["0001", "0004", "0002", "0003"]);
  assert.deepEqual(ids("rate", true), ["0004", "0002", "0003", "0001"]);
  assert.deepEqual(ids("eta", false), ["0004", "0002", "0003", "0001"], "sans estimation à la fin");
  assert.deepEqual(ids("state", false), ["0004", "0002", "0003", "0001"]);
  assert.deepEqual(ids("added", true), ["0001", "0004", "0003", "0002"]);
  assert.deepEqual(ids(), ids("added", true), "ordre par défaut");
});

test("la fin d'une tâche est datée à l'arrivée de la notification", () => {
  const store = new TaskStore();
  store.load(snapshot(1, [task("a", "active", { createdAt: 5 })]));
  const result = store.apply("task.updated", { rev: 2, taskId: "a", fileName: "a.mp4", displayName: "a.mp4", totalBytes: 9 });
  assert.ok(result.list, "le nom change la vue");
  store.apply("task.stateChanged", { rev: 3, taskId: "a", from: "active", to: "completed" }, 1234);
  assert.equal(store.tasks.get("a").finishedAt, 1234);
  store.apply("task.stateChanged", { rev: 4, taskId: "a", from: "completed", to: "queued" }, 2000);
  assert.equal(store.tasks.get("a").finishedAt, null);
  // Moteur antérieur : ni date d'ajout ni date de fin dans l'instantané.
  store.load(snapshot(1, [{ taskId: "b", state: "queued" }]));
  assert.equal(store.tasks.get("b").createdAt, null);
});

test("actions permises par la table des états (SPEC §5.5)", () => {
  assert.ok(canPause("active") && canPause("queued") && canPause("retry_wait") && !canPause("paused"));
  assert.ok(canResume("paused") && canResume("failed") && !canResume("completed"));
  // Une tâche en échec ne peut pas être annulée, seulement relancée.
  assert.ok(!canCancel("failed") && !canCancel("completed") && canCancel("paused"));
  // Seules les tâches finies se retirent ; les autres s'annulent d'abord.
  assert.ok(canRemove("completed") && canRemove("failed") && canRemove("cancelled"));
  assert.ok(!canRemove("paused") && !canRemove("active") && !canRemove("queued"));
  // Retélécharger : tout sauf ce qui tourne ou attend son tour.
  for (const s of ["completed", "failed", "cancelled", "paused", "retry_wait"]) assert.ok(canRestart(s), s);
  assert.ok(!canRestart("active") && !canRestart("queued"));
});

test("retéléchargement : nom, taille et octets effacés, puis en file", () => {
  const store = new TaskStore();
  store.load(snapshot(1, [task("a", "completed", { totalBytes: 9, doneBytes: 9 })]));
  store.apply("task.updated", { rev: 2, taskId: "a", fileName: null, totalBytes: null, doneBytes: 0, errorCode: null, errorDetail: null });
  const out = store.apply("task.stateChanged", { rev: 3, taskId: "a", from: "completed", to: "queued" });
  const again = store.tasks.get("a");
  assert.equal(again.state, "queued");
  assert.equal(again.fileName, null);
  assert.equal(again.doneBytes, 0);
  assert.equal(again.finishedAt, null);
  assert.ok(out.list);
});

test("planifications, ouverture des files et action de fin (F-16)", () => {
  const store = new TaskStore();
  const night = { scheduleId: "s", name: "Nuit", enabled: true, open: false, nextChangeAt: 5, rules: {} };
  store.load({
    rev: 1,
    tasks: [task("a", "queued", { queueId: "q" }), task("b", "queued", { queueId: "autre" })],
    queues: [{ queueId: "q", name: "default", scheduleId: "s" }, { queueId: "autre", name: "x" }],
    schedules: [night],
    endAction: null,
  });
  assert.ok(store.waitsForWindow("q"));
  assert.ok(!store.waitsForWindow("autre"), "sans planification");

  // La plage s'ouvre : les tâches de la file sont à redessiner.
  const opened = store.apply("schedule.updated", { rev: 2, schedule: { ...night, open: true } });
  assert.ok(opened.planning && !opened.resync);
  assert.deepEqual([...opened.changed], ["a"]);
  assert.ok(!store.waitsForWindow("q"));
  // Désactivée : jamais en attente, même fermée.
  store.apply("schedule.updated", { rev: 3, schedule: { ...night, enabled: false } });
  assert.ok(!store.waitsForWindow("q"));

  // Action de fin annoncée, puis refusée par le système.
  store.apply("engine.endAction", { rev: 4, pending: { queueId: "q", action: "shutdown", at: 9 }, error: null });
  assert.deepEqual(store.endAction, { queueId: "q", action: "shutdown", at: 9 });
  const refused = store.apply("engine.endAction", { rev: 5, pending: null, error: "refusée" });
  assert.equal(store.endAction, null);
  assert.equal(refused.endActionError, "refusée");

  // Supprimée, puis la file la perd.
  store.apply("schedule.removed", { rev: 6, scheduleId: "s" });
  store.apply("queue.updated", { rev: 7, queue: { queueId: "q", name: "default", scheduleId: null } });
  assert.equal(store.scheduleOf("q"), null);
  assert.equal(store.rev, 7, "aucune révision perdue");

  // Moteur antérieur : ni planifications ni action de fin dans l'instantané.
  store.load(snapshot(1, []));
  assert.equal(store.schedules.size, 0);
  assert.equal(store.endAction, null);
});

test("le dossier des téléchargements suit l'instantané puis settings.updated", () => {
  const store = new TaskStore();
  store.load({ ...snapshot(1, []), settings: { "downloads.dir": "/a" } });
  assert.equal(store.settings["downloads.dir"], "/a");
  const result = store.apply("settings.updated", { rev: 2, values: { "downloads.dir": "/b" } });
  assert.ok(result.settings);
  assert.ok(!result.resync);
  assert.equal(store.settings["downloads.dir"], "/b");
  // Une révision déjà connue ne change rien.
  assert.ok(!store.apply("settings.updated", { rev: 2, values: { "downloads.dir": "/c" } }).settings);
  assert.equal(store.settings["downloads.dir"], "/b");
});

test("déplacer et actualiser l'adresse : une tâche qui ne tourne pas, dont le fichier ne bouge pas", () => {
  for (const s of ["queued", "paused", "retry_wait", "failed", "cancelled", "completed"]) {
    assert.ok(canRelocate({ state: s }), s);
  }
  assert.ok(!canRelocate({ state: "active" }));
  assert.ok(!canRelocate({ state: "completed", moving: true }));
});

test("déplacement : nouvel emplacement suivi, échec d'une copie annoncé sans rester dans la tâche", () => {
  const store = new TaskStore();
  store.load(snapshot(1, [task("a", "completed", { destDir: "/dl" })]));
  store.apply("task.updated", { rev: 2, taskId: "a", destDir: "/dl", fileName: "a.bin", displayName: "a.bin", moving: true });
  assert.equal(store.tasks.get("a").moving, true);
  const out = store.apply("task.updated", {
    rev: 3, taskId: "a", destDir: "/dl", fileName: "a.bin", displayName: "a.bin", moving: false, moveError: "disque plein",
  });
  assert.deepEqual(out.moveErrors, [{ taskId: "a", error: "disque plein" }]);
  assert.equal(store.tasks.get("a").moving, false);
  assert.ok(!("moveError" in store.tasks.get("a")));
  const moved = store.apply("task.updated", { rev: 4, taskId: "a", destDir: "/autre", fileName: "b.bin", displayName: "b.bin", moving: false });
  assert.deepEqual(moved.moveErrors, []);
  assert.equal(store.tasks.get("a").destDir, "/autre");
  assert.ok(moved.list, "le nom change l'ordre et les vues");
});

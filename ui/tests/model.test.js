// Révisions et état des tâches (SPEC §6.10).
import { test } from "node:test";
import assert from "node:assert/strict";
import { TaskStore, inFilter, canCancel, canPause, canResume } from "../src/model.js";

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

test("filtres, ordre, compteurs et débit", () => {
  const store = new TaskStore();
  store.load(snapshot(1, [
    task("0001", "completed"), task("0002", "active"), task("0003", "cancelled"),
    task("0004", "failed"), task("0005", "paused"), task("0006", "retry_wait"),
  ]));
  store.apply("task.progress", { rev: 1, items: [{ taskId: "0002", bps: 300 }] });
  assert.deepEqual(store.list("all").map((t) => t.taskId), ["0006", "0005", "0004", "0002", "0001"]);
  assert.deepEqual(store.list("active").map((t) => t.taskId), ["0006", "0002"]);
  assert.deepEqual(store.list("stopped").map((t) => t.taskId), ["0005", "0004"]);
  assert.deepEqual(store.counts(), { all: 5, active: 2, stopped: 2, completed: 1, cancelled: 1 });
  assert.deepEqual(store.activity(), { active: 1, bps: 300 });
  assert.ok(inFilter(task("x", "cancelled"), "cancelled"));
});

test("actions permises par la table des états (SPEC §5.5)", () => {
  assert.ok(canPause("active") && canPause("queued") && canPause("retry_wait") && !canPause("paused"));
  assert.ok(canResume("paused") && canResume("failed") && !canResume("completed"));
  // Une tâche en échec ne peut pas être annulée, seulement relancée.
  assert.ok(!canCancel("failed") && !canCancel("completed") && canCancel("paused"));
});

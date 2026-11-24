// Révisions et état des tâches (SPEC §6.10).
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  TaskStore, inView, category, sortTasks, canCancel, canPause, canRemove, canResume, canRestart,
  canRelocate, canSetChecksum, parseChecksum, detailsOutdated, canChooseFiles, isFinishedDownload,
  isMagnet, shareRatio, qualityChoices, exactFormat, formatContent, parseMirrors, canAddMirror, sourceName,
  SOURCE_STATES, MAX_SOURCES, isMetalinkName, isMetalinkUrl, scanRequest, parseSizeText, linkFilter,
  reclassifyLink,
  takeHashToken, sessionError, subDirParts, scopeAllows, pastedToken, isToken, pluginCandidate, pluginLinkName,
  splitListen, joinListen, pageLink, isLoopbackUrl, qrUrls, qrPath, remoteSettled, plannedUrls,
  splitList, splitArgs, ruleFromFields, ruleForSet, ruleSummary, ruleError, moveRule,
  isFileTransferUrl, sshServer, pendingKeyFor, blockedByHostKey,
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
});

test("une commande de fin en échec laisse l'avancement complet", () => {
  const store = new TaskStore();
  store.load(snapshot(1, [task("a", "active")]));
  // Dernier avancement envoyé avant la fin du transfert.
  store.apply("task.progress", { rev: 1, items: [{ taskId: "a", doneBytes: 64 }] });
  store.apply("task.stateChanged", { rev: 2, taskId: "a", from: "active", to: "post_processing" });
  assert.equal(store.tasks.get("a").doneBytes, 100);
  store.apply("task.stateChanged", {
    rev: 3, taskId: "a", from: "post_processing", to: "failed", errorCode: "POSTPROCESS_FAILED",
  });
  const a = store.tasks.get("a");
  assert.equal(a.doneBytes, 100);
  assert.equal(a.errorCode, "POSTPROCESS_FAILED");

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

// Audit W13 : l'avancement porte la révision courante (SPEC 6.10) ; plus récent que l'état
// chargé, il révèle des changements d'état manqués : rechargement, sans l'appliquer.
test("un avancement en avance sur la révision est un trou", () => {
  const store = new TaskStore();
  store.load(snapshot(4, [task("a", "active")]));
  const same = store.apply("task.progress", { rev: 4, items: [{ taskId: "a", doneBytes: 10 }] });
  assert.ok(same.changed.has("a") && !same.resync);
  const ahead = store.apply("task.progress", { rev: 6, items: [{ taskId: "a", doneBytes: 99 }] });
  assert.ok(ahead.resync);
  assert.equal(store.tasks.get("a").doneBytes, 10, "non appliqué");
  const old = store.apply("task.progress", { rev: 3, items: [{ taskId: "a", doneBytes: 1 }] });
  assert.ok(!old.resync && !old.changed.size);
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

test("empreinte (F-11) : lue comme auro --checksum, modifiable tant que le fichier n'est pas fini", () => {
  const sha256 = "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855";
  const sha1 = "da39a3ee5e6b4b0d3255bfef95601890afd80709";
  const md5 = "d41d8cd98f00b204e9800998ecf8427e";
  assert.deepEqual(parseChecksum(` ${sha256} `), { checksum: { algo: "sha256", value: sha256.toLowerCase() } });
  assert.deepEqual(parseChecksum(sha1), { checksum: { algo: "sha1", value: sha1 } });
  assert.deepEqual(parseChecksum(md5).checksum.algo, "md5");
  assert.deepEqual(parseChecksum(`SHA-1:${sha1}`).checksum, { algo: "sha1", value: sha1 });
  assert.deepEqual(parseChecksum(""), { checksum: null });
  assert.deepEqual(parseChecksum("   "), { checksum: null });
  assert.equal(parseChecksum("abcdef").error, "length");
  assert.equal(parseChecksum(`crc32:${md5}`).error, "algo");
  assert.equal(parseChecksum(`sha256:${sha1}`).error, "value");
  assert.equal(parseChecksum(`${md5.slice(1)}g`).error, "value");

  for (const s of ["queued", "active", "paused", "retry_wait", "failed"]) assert.ok(canSetChecksum({ state: s }), s);
  for (const s of ["verifying", "completed", "cancelled"]) assert.ok(!canSetChecksum({ state: s }), s);
  // La vérification s'interrompt et s'annule comme un téléchargement.
  assert.ok(canPause("verifying") && canCancel("verifying") && !canResume("verifying"));
});

test("l'avancement de la vérification de l'empreinte ne survit pas à la vérification", () => {
  const store = new TaskStore();
  store.load(snapshot(1, [task("a", "active")]));
  store.apply("task.stateChanged", { rev: 2, taskId: "a", from: "active", to: "verifying" });
  store.apply("task.progress", { rev: 2, items: [{ taskId: "a", doneBytes: 100, verifiedBytes: 40 }] });
  assert.equal(store.tasks.get("a").verifiedBytes, 40);
  store.apply("task.stateChanged", { rev: 3, taskId: "a", from: "verifying", to: "failed", errorCode: "CHECKSUM_MISMATCH" });
  assert.equal(store.tasks.get("a").verifiedBytes, null);
  const changed = store.apply("task.updated", { rev: 4, taskId: "a", checksum: { algo: "md5", value: "0".repeat(32) } });
  assert.ok(changed.changed.has("a"));
  assert.equal(store.tasks.get("a").checksum.algo, "md5");
});

test("historique en CSV (F-35) : champs protégés, dates locales, octets", async () => {
  const { historyCsv, csvField, localStamp } = await import("../src/model.js");
  assert.equal(csvField("a;b", ";"), '"a;b"');
  assert.equal(csvField("a,b", ";"), "a,b");
  assert.equal(csvField('dit "oui"', ","), '"dit ""oui"""');
  assert.equal(csvField(null, ";"), "");
  const at = new Date(2026, 10, 8, 9, 5, 7).getTime();
  assert.equal(localStamp(at), "2026-11-08 09:05:07");
  const csv = historyCsv([
    { finishedAt: at, finalState: "completed", totalBytes: 1024, displayName: "a;b.iso",
      sourceHost: "exemple.org", finalPath: "C:\dl\a;b.iso", sha256: "ab" },
    { finishedAt: at, finalState: "cancelled", totalBytes: null, displayName: "c.bin",
      sourceHost: null, finalPath: null, sha256: null },
  ], { headers: ["Date", "État", "Taille", "Nom", "Site", "Chemin", "SHA-256"],
       stateLabel: (s) => ({ completed: "Terminé", cancelled: "Annulé" })[s] });
  assert.ok(csv.startsWith("\ufeffDate;État;"), "marque UTF-8 et en-têtes");
  const lines = csv.slice(1).split("\r\n");
  assert.equal(lines[1], '2026-11-08 09:05:07;Terminé;1024;"a;b.iso";exemple.org;"C:\dl\a;b.iso";ab');
  assert.equal(lines[2], "2026-11-08 09:05:07;Annulé;;c.bin;;;");
  assert.equal(lines[3], "", "fin de ligne finale");
});

test("le détail d'une tâche lu pendant le téléchargement se relit une fois la tâche finie", () => {
  assert.ok(detailsOutdated(null, "a"), "rien de lu");
  assert.ok(detailsOutdated({ taskId: "b", state: "completed" }, "a"), "autre tâche");
  // Lu pendant le téléchargement, avec des segments en réception : la fin ne doit pas
  // figer cet affichage (segments « Réception des données » sur une tâche terminée).
  assert.ok(detailsOutdated({ taskId: "a", state: "active" }, "a"));
  assert.ok(detailsOutdated({ taskId: "a", state: "verifying" }, "a"));
  for (const state of ["completed", "failed", "cancelled"]) {
    assert.ok(!detailsOutdated({ taskId: "a", state }, "a"), state);
  }
  // Relancée puis de nouveau en échec entre deux lectures (F-12) : l'état est le même,
  // mais il a changé depuis la lecture.
  const store = new TaskStore();
  store.load(snapshot(1, [task("a", "failed")]));
  const read = { taskId: "a", state: "failed", stateRev: store.tasks.get("a").stateRev ?? null };
  assert.ok(!detailsOutdated(read, "a", store.tasks.get("a").stateRev));
  store.apply("task.stateChanged", { rev: 2, taskId: "a", from: "failed", to: "queued" });
  store.apply("task.stateChanged", { rev: 3, taskId: "a", from: "verifying", to: "failed" });
  assert.ok(detailsOutdated(read, "a", store.tasks.get("a").stateRev), "relu après la relance");
  assert.ok(!detailsOutdated({ ...read, stateRev: 3 }, "a", store.tasks.get("a").stateRev));
});

test("l'annonce du réseau (Android) avance la révision sans recharger", () => {
  const store = new TaskStore();
  store.load(snapshot(10, [task("a")]));
  assert.ok(!store.apply("engine.network", { rev: 11, usable: false }).resync);
  assert.equal(store.rev, 11);
  const next = store.apply("task.stateChanged", { rev: 12, taskId: "a", from: "queued", to: "active" });
  assert.ok(!next.resync, "pas de trou dans les révisions");
});

test("torrents : partage arrêtable, ni adresse ni empreinte, fichiers à choisir (SPEC §7.16)", () => {
  const torrent = (state) => task("t", state, { kind: "torrent" });
  assert.ok(canPause("seeding"), "arrêter le partage");
  assert.ok(!canRelocate(torrent("paused")) && canRelocate(task("h", "paused")));
  assert.ok(!canSetChecksum(torrent("active")) && canSetChecksum(task("h", "active")));
  assert.ok(!canRelocate(task("v", "paused", { kind: "media" })), "vidéo");
  assert.ok(!canSetChecksum(task("v", "active", { kind: "media" })), "vidéo");
  assert.ok(canChooseFiles(torrent("active")) && canChooseFiles(torrent("paused")));
  assert.ok(!canChooseFiles(torrent("seeding")) && !canChooseFiles(torrent("completed")));
  assert.ok(!canChooseFiles(task("h", "active")), "pas un torrent");
  assert.ok(isFinishedDownload(torrent("seeding")) && isFinishedDownload(task("h", "completed")));
  assert.ok(!isFinishedDownload(torrent("active")));
  assert.ok(isMagnet("magnet:?xt=urn:btih:abc") && isMagnet(" MAGNET:?xt=urn:btih:abc "));
  assert.ok(!isMagnet("https://exemple.org/a.torrent") && !isMagnet("magnet:"));
  assert.equal(shareRatio(torrent("seeding")), null, "envoi inconnu");
  assert.equal(shareRatio({ ...torrent("seeding"), uploadedBytes: 50 }), 0.5);
});

test("annonces numérotées : interface des torrents, outils de la vidéo, avertissements", () => {
  const store = new TaskStore();
  store.load({ ...snapshot(1, []), torrentInterfaceUp: false });
  assert.equal(store.torrentInterfaceUp, false);
  let r = store.apply("engine.torrentInterface", { rev: 2, up: true });
  assert.ok(r.torrentInterface && !r.resync);
  assert.equal(store.torrentInterfaceUp, true);
  r = store.apply("media.tools", { rev: 3, ready: true, installing: false, ytdlp: { version: "1" } });
  assert.ok(r.tools && !r.resync);
  assert.equal(store.tools.ready, true);
  assert.ok(!("rev" in store.tools));
  r = store.apply("engine.warning", { rev: 4, code: "SUBTITLES_MISSING", message: "m", taskId: "a" });
  assert.ok(!r.resync, "un avertissement numéroté ne crée pas de trou");
  assert.deepEqual(r.warnings, [{ code: "SUBTITLES_MISSING", message: "m", taskId: "a" }]);
  // La notification suivante ne voit aucun trou.
  r = store.apply("engine.network", { rev: 5, usable: true });
  assert.ok(!r.resync);
  // Notifications perdues : propre à la connexion, sans révision.
  assert.ok(store.apply("engine.warning", { code: "EVENTS_LOST" }).resync);
  assert.equal(store.rev, 5);
});

test("vidéo : qualités proposées, tailles estimées, DRM écarté, format précis (SPEC §7.17)", () => {
  const probe = {
    formats: [
      { formatId: "a1", vcodec: null, acodec: "mp4a", filesize: 1000 },
      { formatId: "a2", vcodec: null, acodec: "opus", filesize: 1500, filesizeApprox: true },
      { formatId: "v720", vcodec: "avc1", acodec: null, height: 720, filesize: 9000 },
      { formatId: "v1080", vcodec: "avc1", acodec: null, height: 1080, filesize: 20000 },
      { formatId: "hls360", vcodec: "avc1", acodec: "mp4a", height: 360 },
      { formatId: "drm4k", vcodec: "hev1", acodec: null, height: 2160, filesize: 1, drm: true },
    ],
  };
  const choices = qualityChoices(probe);
  assert.deepEqual(choices.map((c) => c.key), ["best", "h1080", "h720", "h360", "audio"]);
  assert.equal(choices[0].format, null, "le sélecteur du moteur");
  assert.deepEqual(choices[0].size, { bytes: 21500, approx: true }, "image et meilleur son");
  assert.equal(choices[2].format, "bv*[height<=720]+ba/b[height<=720]");
  assert.deepEqual(choices[2].size, { bytes: 10500, approx: true });
  assert.equal(choices[3].size, null, "HLS sans taille");
  assert.deepEqual(choices[4], { key: "audio", format: "ba", height: null, size: { bytes: 1500, approx: true } });
  // Une vidéo à format unique, codecs inconnus (PeerTube) : une hauteur, pas de son seul.
  const single = qualityChoices({ formats: [{ formatId: "400p", height: 400, filesize: 50 }] });
  assert.deepEqual(single.map((c) => c.key), ["best", "h400"]);
  assert.deepEqual(single[0].size, { bytes: 50, approx: false });
  assert.deepEqual(qualityChoices({ formats: [] }).map((c) => c.key), ["best"]);
  // Format précis : une image seule reçoit le meilleur son.
  assert.equal(exactFormat(probe.formats[2], probe), "v720+ba");
  assert.equal(exactFormat(probe.formats[4], probe), "hls360");
  assert.equal(exactFormat(probe.formats[0], probe), "a1");
  assert.deepEqual(probe.formats.map(formatContent), ["a", "a", "v", "v", "av", "v"]);
  assert.equal(formatContent({ height: 400 }), "?");
});

test("miroirs (SPEC §7.7) : une seule adresse HTTP, adresses lues ligne par ligne, 16 sources au plus", () => {
  const ref = ["https://a.example/f.iso"];
  assert.deepEqual(parseMirrors("", ref), { mirrors: [] });
  assert.deepEqual(parseMirrors("  \n ", ["https://a/x", "https://b/y"], 1), { mirrors: [] }, "sans miroir, rien à vérifier");
  assert.deepEqual(
    parseMirrors(" https://b.example/f.iso\n\nhttp://c.example/pub/f.iso ", ref),
    { mirrors: ["https://b.example/f.iso", "http://c.example/pub/f.iso"] },
  );
  assert.deepEqual(parseMirrors("https://b/f", ["https://a/f", "https://c/f"]), { error: "many" });
  assert.deepEqual(parseMirrors("https://b/f", ref, 1), { error: "many" });
  assert.deepEqual(parseMirrors("https://b/f", [], 1), { error: "torrent" });
  assert.deepEqual(parseMirrors("https://b/f", ["magnet:?xt=urn:btih:00"]), { error: "torrent" });
  assert.deepEqual(parseMirrors("ftp://b/f", ref), { error: "scheme", url: "ftp://b/f" });
  assert.deepEqual(parseMirrors("magnet:?xt=urn:btih:00", ref), { error: "scheme", url: "magnet:?xt=urn:btih:00" });
  assert.deepEqual(parseMirrors(ref[0], ref), { error: "twice", url: ref[0] });
  assert.deepEqual(parseMirrors("https://b/f https://b/f", ref), { error: "twice", url: "https://b/f" });
  const many = (n) => Array.from({ length: n }, (_, i) => `https://m${i}.example/f.iso`).join("\n");
  assert.equal(parseMirrors(many(MAX_SOURCES - 1), ref).mirrors.length, MAX_SOURCES - 1);
  assert.deepEqual(parseMirrors(many(MAX_SOURCES), ref), { error: "count" });
});

test("miroirs : ajout dans les Propriétés pour un fichier HTTP pas fini, nom de chaque source", () => {
  assert.ok(canAddMirror(task("a", "active")));
  assert.ok(canAddMirror(task("a", "failed", { kind: "http" })));
  for (const state of ["queued", "paused", "retry_wait"]) assert.ok(canAddMirror(task("a", state)), state);
  for (const state of ["completed", "cancelled", "verifying"]) assert.ok(!canAddMirror(task("a", state)), state);
  assert.ok(!canAddMirror(task("a", "paused", { kind: "torrent" })));
  assert.ok(!canAddMirror(task("a", "paused", { kind: "media" })));
  assert.deepEqual(sourceName({ rank: 0 }), ["Principale"]);
  assert.deepEqual(sourceName({ rank: 3 }), ["Miroir {0}", 3]);
  assert.deepEqual(Object.keys(SOURCE_STATES).sort(), ["failed", "ok", "slow", "unknown"]);
});

test("Metalink (F-27) : reconnu par son nom ou son adresse ; ni miroir ni empreinte à côté", () => {
  for (const name of ["lot.meta4", "LOT.METALINK", " a.b.meta4 "]) assert.ok(isMetalinkName(name), name);
  for (const name of ["lot.meta4.txt", "meta4", "lot.torrent", "", null]) assert.ok(!isMetalinkName(name), String(name));
  for (const url of [
    "https://download.example/f.iso.meta4",
    "http://a.example/x/f.metalink?mirrorlist=1",
    "https://a.example/f.META4#top",
  ]) assert.ok(isMetalinkUrl(url), url);
  for (const url of [
    "https://a.example/f.iso",
    "https://a.example/get?file=f.meta4",
    "ftp://a.example/f.meta4",
    "/home/f.meta4",
  ]) assert.ok(!isMetalinkUrl(url), url);
  assert.deepEqual(parseMirrors("https://b/f", [], 0, 1), { error: "metalink" });
  assert.deepEqual(parseMirrors("https://b/f", ["https://a/f.meta4"]), { error: "metalink" });
  assert.deepEqual(parseMirrors("https://b/f", ["https://a/f"], 0, 1), { error: "many" });
  assert.deepEqual(parseMirrors("", [], 0, 2), { mirrors: [] });
});

test("analyse de liens (F-29) : page ou texte, tailles écrites, filtres comme auro links", () => {
  assert.equal(scanRequest("  "), null);
  for (const page of ["https://a.example/", "https://a.example/liste", "http://a.example/dl/index.php?p=2", "https://a.example/x.html#haut"]) {
    assert.deepEqual(scanRequest(` ${page} `), { pageUrl: page }, page);
  }
  // Une adresse de fichier seule, ou plusieurs adresses : un texte dont on relève les liens.
  assert.deepEqual(scanRequest("https://a.example/f.iso"), { text: "https://a.example/f.iso" });
  assert.deepEqual(scanRequest("https://a.example/ https://b.example/"), { text: "https://a.example/ https://b.example/" });
  assert.deepEqual(scanRequest("voir https://a.example/"), { text: "voir https://a.example/" });

  assert.equal(parseSizeText(""), null);
  assert.equal(parseSizeText("800000"), 800000);
  assert.equal(parseSizeText("500K"), 512000);
  assert.equal(parseSizeText("2 Mio"), 2 * 1024 ** 2);
  assert.equal(parseSizeText("1,5G"), 1.5 * 1024 ** 3);
  assert.ok(Number.isNaN(parseSizeText("beaucoup")));

  const iso = { url: "https://a.example/debian-13.iso", ext: "iso", kind: "file" };
  const page = { url: "https://a.example/index.html", ext: "html", kind: "page" };
  const magnet = { url: "magnet:?xt=urn:btih:abc", ext: null, kind: "magnet" };
  const zip = { url: "https://b.example/src.zip", ext: "zip", kind: "file" };
  const plain = linkFilter();
  assert.ok(plain.keep(iso) && plain.keep(magnet) && !plain.keep(page) && !plain.sized);
  assert.ok(linkFilter({ all: true }).keep(page));
  const ext = linkFilter({ extensions: ".ISO, 7z" });
  assert.ok(ext.keep(iso) && !ext.keep(zip) && !ext.keep(magnet));
  const regex = linkFilter({ regex: String.raw`debian-\d+` });
  assert.ok(regex.keep(iso) && !regex.keep(zip));
  assert.ok(!linkFilter({ regex: "DEBIAN" }).keep(iso), "casse comprise, comme la CLI");
  const sized = linkFilter({ min: "10", max: "20" });
  assert.ok(sized.sized && sized.keep(iso, 10) && sized.keep(iso, 20));
  assert.ok(!sized.keep(iso, 9) && !sized.keep(iso, 21) && !sized.keep(iso, null));
  assert.deepEqual(linkFilter({ min: "dix" }), { error: "min" });
  assert.deepEqual(linkFilter({ max: "?" }), { error: "max" });
  assert.equal(linkFilter({ regex: "(" }).error, "regex");
});

test("un lien sans extension que le serveur décrit comme un fichier en devient un", () => {
  const sums = { url: "https://a.example/SHA256SUMS", ext: null, kind: "page" };
  assert.equal(reclassifyLink(sums, "file").kind, "file");
  assert.equal(reclassifyLink(sums, "page").kind, "page");
  assert.equal(reclassifyLink(sums, null).kind, "page");
  const zip = { url: "https://a.example/a.zip", ext: "zip", kind: "file" };
  assert.equal(reclassifyLink(zip, "page"), zip, "un fichier le reste");
});

test("interface distante : jeton de l'ancre, sous-dossier, portées", () => {
  const token = "auro_" + "A".repeat(43);
  assert.equal(takeHashToken(`#token=${token}`), token);
  assert.equal(takeHashToken(`#x=1&token=${token}`), token);
  for (const bad of ["", "#", "#token=", "#token=auro_court", `#token=${token}x`, `#jeton=${token}`]) {
    assert.equal(takeHashToken(bad), null, bad);
  }
  // Ce qu'on colle dans la connexion : le jeton seul, entouré d'espaces, l'adresse, la ligne.
  assert.equal(pastedToken(token), token);
  assert.equal(pastedToken(`  ${token}\n`), token);
  assert.equal(pastedToken(`http://192.168.1.20:7380/#token=${token}`), token);
  assert.equal(pastedToken(`control : http://127.0.0.1:7391/#token=${token}`), token);
  assert.equal(pastedToken("  pas un jeton "), "pas un jeton");
  assert.equal(pastedToken(`${token}x`), `${token}x`);
  assert.ok(isToken(token) && !isToken(`${token}x`) && !isToken("auro_…") && !isToken(""));
  assert.equal(sessionError(401).code, "UNAUTHORIZED");
  assert.equal(sessionError(429).code, "TOO_MANY_ATTEMPTS");
  assert.equal(sessionError(500).code, "ENGINE_DOWN");

  assert.equal(subDirParts(""), "");
  assert.equal(subDirParts("  "), "");
  assert.equal(subDirParts("ISO/Debian"), "ISO/Debian");
  assert.equal(subDirParts("/ISO//Debian/ "), "ISO/Debian");
  assert.equal(subDirParts("ISO\\Debian"), "ISO/Debian");
  assert.equal(subDirParts("../x"), null);
  assert.equal(subDirParts("a/./b"), null);

  assert.ok(scopeAllows("admin", "admin") && scopeAllows("control", "add") && scopeAllows("control", "read"));
  assert.ok(!scopeAllows("control", "admin") && !scopeAllows("read", "add") && !scopeAllows("add", "read"));
  assert.ok(scopeAllows("read", "read") && scopeAllows("add", "add"));
});

test("accès à distance : adresse d'écoute lue et écrite, IPv6 entre crochets", () => {
  assert.deepEqual(splitListen("127.0.0.1:7380"), { host: "127.0.0.1", port: 7380 });
  assert.deepEqual(splitListen("[fd00::20]:443"), { host: "fd00::20", port: 443 });
  assert.equal(splitListen("fd00::20:443"), null);
  assert.equal(splitListen("127.0.0.1:70000"), null);
  assert.equal(splitListen(""), null);
  assert.equal(joinListen("192.168.1.20", 7380), "192.168.1.20:7380");
  assert.equal(joinListen("fd00::20", 7380), "[fd00::20]:7380");
  for (const text of ["10.0.0.5:8080", "[::1]:7380"]) {
    const { host, port } = splitListen(text);
    assert.equal(joinListen(host, port), text);
  }
});

test("accès à distance : lien du QR code, jeton dans l'ancre, adresses du réseau d'abord", () => {
  const token = "auro_" + "a".repeat(43);
  const link = pageLink("https://192.168.1.20:7380", token);
  assert.equal(link, `https://192.168.1.20:7380/#token=${token}`);
  // La page le relit tel quel.
  assert.equal(takeHashToken(new URL(link).hash), token);
  assert.equal(pageLink("https://nas.maison:7380/", token), `https://nas.maison:7380/#token=${token}`);
  assert.ok(isLoopbackUrl("http://127.0.0.1:7380"));
  assert.ok(isLoopbackUrl("https://[::1]:7380"));
  assert.ok(isLoopbackUrl("https://localhost:7380"));
  assert.ok(!isLoopbackUrl("https://192.168.1.20:7380"));
  assert.ok(!isLoopbackUrl("https://[fd00::20]:7380"));
  assert.deepEqual(
    qrUrls(["http://127.0.0.1:7380", "https://192.168.1.20:7380", "https://nas.maison:7380"]),
    ["https://192.168.1.20:7380", "https://nas.maison:7380", "http://127.0.0.1:7380"],
  );
});

test("accès à distance : tracé du QR code, marge de 4 modules", () => {
  assert.equal(qrPath(2, "1001"), "M4 4h1v1h-1zM5 5h1v1h-1z");
  assert.equal(qrPath(1, "0"), "");
});

test("accès à distance : l'écoute suit-elle les réglages ?", () => {
  const settings = { "remote.enabled": true, "remote.listen": "192.168.1.20:7380", "remote.hosts": [], "remote.tlsCert": null };
  const status = { enabled: true, listen: "192.168.1.20:7380", listening: true, hosts: [], tlsCert: null };
  assert.ok(remoteSettled(status, settings));
  // Pas encore déplacée.
  assert.ok(!remoteSettled({ ...status, listen: "127.0.0.1:7380" }, settings));
  // Activée mais fermée, avec la raison : c'est fini aussi.
  assert.ok(remoteSettled({ ...status, listening: false, error: "port occupé" }, settings));
  assert.ok(!remoteSettled({ ...status, listening: false }, settings));
  // Coupée : fini quand l'écoute est fermée.
  const off = { ...settings, "remote.enabled": false };
  assert.ok(remoteSettled({ ...status, enabled: false, listening: false }, off));
  assert.ok(!remoteSettled({ ...status, enabled: false, listening: true }, off));
  assert.ok(!remoteSettled(null, settings));
});

test("accès à distance : adresse de la page avant l'ouverture, d'après les réglages", () => {
  const settings = (listen, hosts = []) => ({ "remote.listen": listen, "remote.hosts": hosts });
  assert.deepEqual(plannedUrls(settings("127.0.0.1:7380"), []), ["http://127.0.0.1:7380"]);
  assert.deepEqual(plannedUrls({}, []), ["http://127.0.0.1:7380"]);
  assert.deepEqual(plannedUrls(settings("192.168.1.20:7391", ["nas.maison"]), []),
    ["https://192.168.1.20:7391", "https://nas.maison:7391"]);
  assert.deepEqual(plannedUrls(settings("[fd00::20]:7380"), []), ["https://[fd00::20]:7380"]);
  const addresses = [{ interface: "Wi-Fi", address: "192.168.1.20" }, { interface: "Wi-Fi", address: "fd00::20" }];
  assert.deepEqual(plannedUrls(settings("0.0.0.0:7380"), addresses), ["https://192.168.1.20:7380"]);
  assert.deepEqual(plannedUrls(settings("[::]:7380"), addresses),
    ["https://192.168.1.20:7380", "https://[fd00::20]:7380"]);
});

test("plugins : pages candidates et noms des liens rendus", () => {
  assert.ok(pluginCandidate("https://fichiers.exemple/f/abc"));
  assert.ok(pluginCandidate("HTTP://fichiers.exemple/"));
  for (const url of ["magnet:?xt=urn:btih:abc", "ftp://a.exemple/f", "https://a.exemple/lot.meta4", "fichiers.exemple/f", "", undefined]) {
    assert.ok(!pluginCandidate(url), String(url));
  }
  assert.equal(pluginLinkName({ url: "https://cdn.exemple/x/f.iso?jeton=1", name: "Debian.iso" }), "Debian.iso");
  assert.equal(pluginLinkName({ url: "https://cdn.exemple/x/f.iso?jeton=1#a", name: null }), "f.iso");
  assert.equal(pluginLinkName({ url: "https://cdn.exemple/", name: "" }), "https://cdn.exemple/");
});

test("règles : champs de la boîte, résumé, ordre (SPEC §7.21)", () => {
  assert.deepEqual(splitList(" .ISO, iso;pdf  zip ", { extensions: true }), ["iso", "pdf", "zip"]);
  assert.deepEqual(splitList("Debian.org,\nubuntu.com"), ["debian.org", "ubuntu.com"]);
  assert.deepEqual(splitList(""), []);
  assert.deepEqual(splitArgs("--scan\n\n{chemin}\r\nun argument\n"), ["--scan", "{chemin}", "un argument"]);

  const rule = ruleFromFields({
    name: " Images ", enabled: true, categories: ["images"], extensions: ".iso", domains: "",
    urlRegex: "", nameRegex: " ^photo", folder: " Rangées/Images ", rename: "",
    program: "", args: "ignoré", timeout: "",
  });
  assert.deepEqual(rule, {
    name: "Images", enabled: true,
    match: { categories: ["images"], extensions: ["iso"], domains: [], urlRegex: null, nameRegex: " ^photo" },
    actions: { folder: "Rangées/Images", rename: null, command: null },
  });
  const withCommand = ruleFromFields({ ruleId: "r1", enabled: false, program: "/usr/bin/clamscan", args: "{chemin}", timeout: "30" });
  assert.equal(withCommand.ruleId, "r1");
  assert.equal(withCommand.enabled, false);
  assert.deepEqual(withCommand.actions.command, { program: "/usr/bin/clamscan", args: ["{chemin}"], timeoutS: 30 });
  assert.equal(ruleFromFields({ program: "/bin/x", timeout: "" }).actions.command.timeoutS, undefined);

  const listed = { ruleId: "r1", name: "N", enabled: true, match: rule.match, actions: { folder: "F", rename: null, hasCommand: true } };
  assert.deepEqual(ruleForSet(listed).actions, { folder: "F", rename: null });
  assert.deepEqual(ruleSummary(listed), {
    when: [["type", ["images", "iso"]], ["name", " ^photo"]],
    then: [["folder", "F"], ["hasCommand", true]],
  });
  assert.deepEqual(ruleSummary({ match: {}, actions: {} }), { when: [], then: [] });

  const list = ["a", "b", "c"];
  assert.deepEqual(moveRule(list, 1, -1), ["b", "a", "c"]);
  assert.deepEqual(moveRule(list, 1, 1), ["a", "c", "b"]);
  assert.equal(moveRule(list, 0, -1), list);
  assert.equal(moveRule(list, 2, 1), list);
});

test("une erreur du moteur sur une règle nomme son champ sans chemin technique", () => {
  assert.deepEqual(
    ruleError("rules[0] : actions.folder : un chemin absolu, ou des noms (\"..\" refusé)"),
    { field: "rule-folder", text: "un chemin absolu, ou des noms (\"..\" refusé)" },
  );
  assert.deepEqual(
    ruleError("rules[2] : actions.command.args : trop d'arguments"),
    { field: "rule-args", text: "trop d'arguments" },
  );
  // Champ sans case dans la boîte : le message seul.
  assert.deepEqual(
    ruleError("rules[1] : match.categories : type inconnu \"x\""),
    { field: null, text: "type inconnu \"x\"" },
  );
  // Message d'une autre forme : tel quel.
  assert.deepEqual(ruleError("moteur injoignable"), { field: null, text: "moteur injoignable" });
});

test("FTP et SFTP : une source, et le serveur SSH d'une adresse comme le moteur le nomme", () => {
  assert.ok(isFileTransferUrl("ftp://h/f") && isFileTransferUrl("FTPS://h/f") && isFileTransferUrl(" sftp://h/f"));
  assert.ok(!isFileTransferUrl("https://h/f") && !isFileTransferUrl(null));
  assert.deepEqual(parseMirrors("https://m/f", ["ftp://h/f"]), { error: "transfer" });
  assert.ok(!canAddMirror(task("a", "failed"), "sftp://h/f"));
  assert.ok(canAddMirror(task("a", "failed"), "https://h/f"));

  assert.deepEqual(sshServer("sftp://Moi:x@SSH.Exemple.org/f.iso"), { host: "ssh.exemple.org", port: 22 });
  assert.deepEqual(sshServer("sftp://h:2222/~/f"), { host: "h", port: 2222 });
  assert.deepEqual(sshServer("sftp://[::1]:2222/f"), { host: "::1", port: 2222 });
  assert.equal(sshServer("ftp://h/f"), null);
  assert.equal(sshServer("pas une adresse"), null);

  const keys = { items: [], pending: [{ host: "h", port: 2222, algorithm: "ssh-ed25519", fingerprint: "SHA256:a", changed: false }] };
  assert.equal(pendingKeyFor(keys, "sftp://h:2222/f").fingerprint, "SHA256:a");
  assert.equal(pendingKeyFor(keys, "sftp://h/f"), null);
  assert.equal(pendingKeyFor(null, "sftp://h:2222/f"), null);

  assert.ok(blockedByHostKey(task("a", "failed", { errorCode: "HOST_KEY_CHANGED" })));
  assert.ok(!blockedByHostKey(task("a", "failed", { errorCode: "AUTH_FAILED" })));
  assert.ok(!blockedByHostKey(task("a", "queued", { errorCode: "HOST_KEY_UNKNOWN" })));
});

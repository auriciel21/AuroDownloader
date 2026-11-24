// Arrière-plan de l'extension (`background.js`) avec un faux `chrome` qui note les appels :
// téléchargement capté, confié au relais, puis annulé et effacé du navigateur.
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const read = (name) => fs.readFileSync(path.join(__dirname, "..", name), "utf8");

/// Un arrière-plan chargé ; `cancelFails` : le navigateur refuse l'annulation (téléchargement
/// déjà fini).
function background({ cancelFails = false, stored = {} } = {}) {
  const calls = [];
  let onCreated;
  let onMessage;
  const note = (name, value) => (...args) => {
    calls.push([name, ...args]);
    return Promise.resolve(value);
  };
  const chrome = {
    runtime: {
      id: "auro",
      onMessage: { addListener: (fn) => { onMessage = fn; } },
      sendNativeMessage: note("relay", { ok: true, taskId: "t1" }),
    },
    i18n: { getMessage: () => "" },
    action: { setBadgeBackgroundColor: note("badge"), setBadgeText: note("badge") },
    storage: { sync: { get: () => Promise.resolve(stored) }, local: { set: note("store") } },
    cookies: { getAll: () => Promise.resolve([]) },
    contextMenus: { removeAll() {}, create() {}, onClicked: { addListener() {} } },
    downloads: {
      onCreated: { addListener: (fn) => { onCreated = fn; } },
      pause: note("pause"),
      resume: note("resume"),
      cancel: (...args) => {
        calls.push(["cancel", ...args]);
        return cancelFails ? Promise.reject(new Error("déjà fini")) : Promise.resolve();
      },
      erase: note("erase"),
    },
  };
  const context = vm.createContext({
    chrome,
    navigator: { userAgent: "essai" },
    setTimeout: () => 0,
    console,
    URL,
  });
  context.globalThis = context;
  vm.runInContext(read("decide.js"), context);
  vm.runInContext(read("background.js"), context);
  /// Message d'un script de contenu ; rend la réponse.
  const message = (msg, sender) =>
    new Promise((resolve) => {
      if (!onMessage(msg, sender, resolve)) resolve(undefined);
    });
  return { calls, created: (item) => onCreated(item), message };
}

const ITEM = {
  id: 7,
  url: "https://exemple.org/gros.iso",
  state: "in_progress",
  totalBytes: 500 * 1024 * 1024,
  referrer: "https://exemple.org/",
};

test("un téléchargement confié est annulé puis effacé du navigateur", async () => {
  const bg = background();
  await bg.created(ITEM);
  const names = bg.calls.map((c) => c[0]).filter((n) => ["relay", "cancel", "erase"].includes(n));
  assert.deepEqual(names, ["relay", "cancel", "erase"]);
});

// Audit W8 : une annulation refusée (le navigateur a déjà fini) laisse son entrée, au lieu
// d'un fichier en double que l'utilisateur ne voit plus dans le navigateur.
test("une annulation refusée laisse l'entrée du navigateur", async () => {
  const bg = background({ cancelFails: true });
  await bg.created(ITEM);
  const names = bg.calls.map((c) => c[0]);
  assert.ok(names.includes("cancel"), names.join(" "));
  assert.ok(!names.includes("erase"), names.join(" "));
});

// Audit W9 (décision 8) : un lien magnet cliqué dans une fenêtre privée suit
// « captureIncognito », comme un téléchargement ; sinon il est rendu au navigateur.
test("lien magnet d'une fenêtre privée : selon captureIncognito", async () => {
  const magnet = { type: "magnet", url: "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567" };
  const relayed = (bg) => bg.calls.filter((c) => c[0] === "relay").length;
  // Objets du contexte de l'arrière-plan : comparés par leur contenu.
  const plain = (v) => JSON.parse(JSON.stringify(v));

  const off = background();
  assert.deepEqual(plain(await off.message(magnet, { tab: { incognito: true } })), { ok: false, giveBack: true });
  assert.equal(relayed(off), 0, "rien n'est confié au moteur");

  const normal = background();
  assert.deepEqual(plain(await normal.message(magnet, { tab: { incognito: false } })), { ok: true, giveBack: false });
  assert.equal(relayed(normal), 1);

  const on = background({ stored: { captureIncognito: true } });
  assert.deepEqual(plain(await on.message(magnet, { tab: { incognito: true } })), { ok: true, giveBack: false });
  assert.equal(relayed(on), 1);
});

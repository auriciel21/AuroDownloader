// Hôte de la page distante (`ui/src/web.js`) avec un faux WebSocket : ouverture, annonce,
// abonnement selon la portée (audit W2, W25).
import { test } from "node:test";
import assert from "node:assert/strict";

/** Faux WebSocket : note ses envois ; le test ouvre et répond à la place du moteur. */
class FakeSocket {
  static OPEN = 1;
  static made = [];
  constructor(url) {
    this.url = url;
    this.readyState = 0;
    this.sent = [];
    this.listeners = {};
    FakeSocket.made.push(this);
  }
  addEventListener(kind, fn) {
    (this.listeners[kind] ??= []).push(fn);
  }
  send(text) {
    this.sent.push(JSON.parse(text));
  }
  close() {
    this.readyState = 3;
    for (const fn of this.listeners.close ?? []) fn({});
  }
  open() {
    this.readyState = FakeSocket.OPEN;
    for (const fn of this.listeners.open ?? []) fn({});
  }
  reply(id, result) {
    for (const fn of this.listeners.message ?? []) fn({ data: JSON.stringify({ jsonrpc: "2.0", id, result }) });
  }
}

const define = (name, value) => Object.defineProperty(globalThis, name, { value, configurable: true, writable: true });
define("WebSocket", FakeSocket);
define("location", { protocol: "http:", host: "nas.maison:7380", reload() {} });
define("document", { hidden: false, addEventListener() {} });
define("localStorage", { getItem: () => null, setItem() {} });
define("navigator", { language: "fr-FR" });
define("fetch", async () => ({ status: 200, ok: true, json: async () => ({ name: "tél", scope: "add" }) }));

const { webHost } = await import("../src/web.js");
const tick = () => new Promise((r) => setTimeout(r, 0));

async function connected(scope) {
  FakeSocket.made.length = 0;
  const host = webHost();
  const states = [];
  await host.listen((event) => { if (event.kind === "status") states.push(event.state); });
  const ws = FakeSocket.made[0];
  assert.equal(ws.url, "ws://nas.maison:7380/ws");
  ws.open();
  await tick();
  const hello = ws.sent[0];
  assert.equal(hello.method, "engine.hello");
  ws.reply(hello.id, { protocol: "1.0", rev: 0, scope, capabilities: [] });
  await tick();
  return { ws, states };
}

test("un jeton « Ajouter seulement » se connecte sans s'abonner aux événements", async () => {
  const { ws, states } = await connected("add");
  assert.deepEqual(ws.sent.map((m) => m.method), ["engine.hello"]);
  assert.equal(states.at(-1), "up");
});

test("une portée qui lit s'abonne, puis la connexion est prête", async () => {
  const { ws, states } = await connected("control");
  const subscribe = ws.sent[1];
  assert.equal(subscribe?.method, "events.subscribe");
  assert.notEqual(states.at(-1), "up", "prête seulement après l'abonnement");
  ws.reply(subscribe.id, {});
  await tick();
  assert.equal(states.at(-1), "up");
});

// Audit W12 : stockage refusé (navigation privée), la langue choisie vaut pour la page.
test("langue choisie gardée quand le navigateur refuse le stockage", async () => {
  const refuse = () => { throw new Error("SecurityError"); };
  define("localStorage", { getItem: refuse, setItem: refuse });
  try {
    const host = webHost();
    assert.equal((await host.info()).lang, "fr", "langue du navigateur d'abord");
    const saved = await host.savePrefs({ lang: "en" });
    assert.equal(saved.lang, "en");
    assert.equal(saved.prefs.lang, "en");
    assert.equal((await host.info()).lang, "en");
  } finally {
    define("localStorage", { getItem: () => null, setItem() {} });
  }
});

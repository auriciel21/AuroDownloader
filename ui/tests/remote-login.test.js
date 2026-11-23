// Page distante (`ui/src/web.js`, étape 3c) : la boîte « Connexion à AuroDownloader » dans
// jsdom, sans faux Tauri, avec un faux serveur (`/session`, `/login`) et un faux WebSocket.
// Audit W5 : une erreur se montre la boîte ouverte, liée au champ (`aria-invalid`,
// `aria-describedby`), le focus revenu au champ ; jamais fermée puis rouverte (leçon A15).
import { test, before } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const { JSDOM } = require("jsdom");

const html = readFileSync(new URL("../src/index.html", import.meta.url), "utf8");
const GOOD = `auro_${"A".repeat(43)}`;
const KNOWN = `auro_${"B".repeat(43)}`;
let window;
let reopened = 0;
const logins = [];

class FakeSocket {
  static OPEN = 1;
  constructor() { this.readyState = 0; }
  addEventListener() {}
  send() {}
  close() {}
}

before(async () => {
  const dom = new JSDOM(html, { runScripts: "outside-only", pretendToBeVisual: true, url: "http://nas.maison:7380/" });
  window = dom.window;
  const { document } = window;
  const proto = window.HTMLDialogElement.prototype;
  proto.showModal = function () {
    if (this.id === "remote-login" && this.hasAttribute("data-shown")) reopened += 1;
    this.setAttribute("data-shown", "");
    this.setAttribute("open", "");
  };
  proto.show = proto.showModal;
  proto.close = function (value) {
    if (value !== undefined) this.returnValue = value;
    this.removeAttribute("open");
    this.dispatchEvent(new window.Event("close"));
  };
  window.matchMedia ??= () => ({ matches: false, addEventListener() {}, removeEventListener() {} });
  window.HTMLElement.prototype.scrollIntoView ??= () => {};
  window.WebSocket = FakeSocket;
  let signedIn = false;
  globalThis.fetch = async (url, init = {}) => {
    const path = new URL(url, "http://nas.maison:7380/").pathname;
    if (path === "/session") {
      return signedIn
        ? { ok: true, status: 200, json: async () => ({ name: "tél", scope: "control" }) }
        : { ok: false, status: 401, json: async () => ({}) };
    }
    if (path === "/login") {
      const { token } = JSON.parse(init.body);
      logins.push(token);
      if (token === KNOWN) {
        signedIn = true;
        return { ok: true, status: 200, json: async () => ({ name: "tél", scope: "control" }) };
      }
      return { ok: false, status: 401, json: async () => ({}) };
    }
    try {
      const text = readFileSync(new URL(`../src${path}`, import.meta.url), "utf8");
      return { ok: true, status: 200, text: async () => text };
    } catch {
      return { ok: false, status: 404, text: async () => "" };
    }
  };
  for (const key of Object.getOwnPropertyNames(window)) {
    if (!(key in globalThis)) {
      try { globalThis[key] = window[key]; } catch { /* propriété en lecture seule */ }
    }
  }
  for (const key of ["Option", "Image", "Audio"]) globalThis[key] ??= window[key];
  globalThis.CSS ??= { escape: (text) => String(text).replace(/[^\w-]/g, (c) => `\\${c}`) };
  globalThis.window = window;
  globalThis.document = document;
  globalThis.WebSocket = FakeSocket;
  Object.defineProperty(globalThis, "location", { value: window.location, configurable: true });
  globalThis.history = window.history;
  globalThis.localStorage = window.localStorage;

  import("../src/main.js");
  // La page démarre de façon asynchrone : sa boîte de connexion s'ouvre quand elle a vu
  // qu'il n'y a pas de session. Attendue, pas supposée au bout d'un délai fixe.
  await until(() => window.document.getElementById("remote-login").open);
});

/// Attend que `pred` soit vrai, cinq secondes au plus (machine chargée).
async function until(pred) {
  for (let i = 0; i < 500 && !pred(); i++) await new Promise((r) => setTimeout(r, 10));
}

/// Envoie `text` ; attend que la page l'ait traité (erreur montrée, ou boîte fermée).
async function submit(text) {
  const { document } = window;
  const field = document.getElementById("rl-token");
  const error = document.getElementById("rl-error");
  const before = `${error.textContent}|${logins.length}`;
  field.value = text;
  field.focus();
  document.getElementById("rl-form").requestSubmit();
  await until(() => !document.getElementById("remote-login").open
    || (`${error.textContent}|${logins.length}` !== before && !error.hidden));
  await new Promise((r) => setTimeout(r, 20));
}

function described(field) {
  return (field.getAttribute("aria-describedby") ?? "").split(/\s+/).filter(Boolean);
}

test("la boîte reste ouverte, l'erreur liée au champ, puis la connexion la ferme", async () => {
  const { document } = window;
  const dialog = document.getElementById("remote-login");
  const field = document.getElementById("rl-token");
  const error = document.getElementById("rl-error");
  assert.ok(dialog.open, `boîte ouverte sans session : ${document.getElementById("error").textContent}`);

  // Un texte sans jeton : jamais envoyé, l'erreur dite sans le montrer.
  await submit("pas un jeton");
  assert.ok(dialog.open, "toujours ouverte");
  assert.deepEqual(logins, []);
  assert.equal(field.getAttribute("aria-invalid"), "true");
  assert.ok(described(field).includes(error.id), described(field).join(" "));
  assert.equal(error.hidden, false);
  assert.match(error.textContent, /\(12 /, "le nombre de caractères, pas le texte");
  assert.doesNotMatch(error.textContent, /pas un jeton/);
  assert.equal(document.activeElement, field, "focus au champ");

  // Un jeton inconnu du moteur.
  await submit(GOOD);
  assert.ok(dialog.open);
  assert.deepEqual(logins, [GOOD]);
  assert.match(error.textContent, /inconnu|unknown/i);
  assert.equal(field.getAttribute("aria-invalid"), "true");
  assert.equal(field.value, "", "le jeton ne reste pas dans le champ");

  // Le bon jeton : la boîte se ferme, l'erreur part.
  await submit(KNOWN);
  assert.ok(!dialog.open, "fermée après la connexion");
  assert.equal(field.getAttribute("aria-invalid"), null);
  assert.equal(reopened, 0, "jamais fermée puis rouverte");
});

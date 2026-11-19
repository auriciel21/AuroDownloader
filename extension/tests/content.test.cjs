// Script de contenu (`content.js`) dans jsdom, avec un faux `chrome.runtime` qui note ses
// messages. Audit W1 et W10 : un clic fabriqué par la page (`a.click()`, `dispatchEvent`,
// sans `isTrusted`) ne confie aucun lien magnet à AuroDownloader et ne passe pas pour un
// Alt+clic. jsdom ne sait pas fabriquer un vrai geste de l'utilisateur : la capture des
// clics réels reste essayée dans les navigateurs (docs/ESSAI-EXTENSION.md).
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { createRequire } = require("node:module");

const fromUi = createRequire(path.join(__dirname, "..", "..", "ui", "package.json"));
const { JSDOM } = fromUi("jsdom");

const script = fs.readFileSync(path.join(__dirname, "..", "content.js"), "utf8");

function page() {
  const dom = new JSDOM(
    `<!doctype html><a id="m" href="magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567">t</a>`,
    { runScripts: "outside-only", url: "https://exemple.org/page" },
  );
  const sent = [];
  dom.window.chrome = {
    runtime: { sendMessage: (m) => { sent.push(m); return Promise.resolve({ ok: true }); } },
    storage: {
      sync: { get: () => Promise.resolve({}) },
      onChanged: { addListener: () => {} },
    },
  };
  dom.window.eval(script);
  return { dom, sent };
}

test("un lien magnet cliqué par un script de la page n'est pas confié à AuroDownloader", async () => {
  const { dom, sent } = page();
  const a = dom.window.document.getElementById("m");
  const click = new dom.window.MouseEvent("click", { bubbles: true, cancelable: true, button: 0 });
  assert.equal(click.isTrusted, false);
  a.dispatchEvent(click);
  a.click();
  await new Promise((r) => setTimeout(r, 0));
  assert.deepEqual(sent, [], "aucun message");
  assert.equal(click.defaultPrevented, false, "le lien suit son chemin habituel");
});

test("un Alt+clic fabriqué par la page n'est pas transmis", async () => {
  const { dom, sent } = page();
  const a = dom.window.document.getElementById("m");
  for (const kind of ["mousedown", "click", "auxclick"]) {
    a.dispatchEvent(new dom.window.MouseEvent(kind, { bubbles: true, altKey: true, button: 0 }));
  }
  await new Promise((r) => setTimeout(r, 0));
  assert.deepEqual(sent, []);
});

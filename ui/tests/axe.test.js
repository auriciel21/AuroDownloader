// Accessibilité du balisage de la page (WCAG 2.2 AA, N-09) : axe-core dans jsdom, sur la
// fenêtre principale puis sur chaque boîte ouverte, tout son contenu montré (messages
// d'erreur et sections cachées compris : ils s'affichent selon l'état). jsdom ne calcule
// pas la mise en page : les contrastes sont vérifiés à part (contrast.test.js).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import axe from "axe-core";
import { JSDOM } from "jsdom";

const html = readFileSync(new URL("../src/index.html", import.meta.url), "utf8");

// Règles WCAG A et AA (2.0 à 2.2) et bonnes pratiques ; sans contraste (mise en page absente).
const OPTIONS = {
  runOnly: { type: "tag", values: ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa", "best-practice"] },
  rules: { "color-contrast": { enabled: false } },
};

/** Une page neuve, sans ses scripts, avec axe chargé. */
function page() {
  const dom = new JSDOM(html, { runScripts: "outside-only", pretendToBeVisual: true });
  // Sans mise en page, jsdom n'a pas `elementFromPoint` : axe s'en sert pour trouver une boîte
  // modale ouverte, et sauterait sinon toutes ses règles. Rien sous un point : pas de modale.
  dom.window.document.elementFromPoint = () => null;
  dom.window.document.elementsFromPoint = () => [];
  dom.window.eval(axe.source);
  return dom.window;
}

function describe(violations) {
  return violations.flatMap((v) =>
    v.nodes.map((n) => `${v.id} (${v.impact}) : ${n.target.join(" ")} — ${n.failureSummary.split("\n").slice(1).join(" ").trim()}`),
  );
}

async function audit(window, context) {
  const result = await window.axe.run(context, { ...OPTIONS, resultTypes: ["violations", "incomplete"] });
  // Une règle qu'axe n'a pas pu exécuter (fonction absente de jsdom) est un échec : sinon
  // l'audit passerait sans rien vérifier.
  const skipped = Array.from(result.incomplete)
    .filter((r) => r.error)
    .map((r) => `${r.id} sautée : ${r.error.message}`);
  // Tableaux de la page (autre royaume) : recopiés pour la comparaison.
  return [...describe(Array.from(result.violations)), ...skipped];
}

test("fenêtre principale", async () => {
  const window = page();
  assert.deepEqual(await audit(window, window.document), []);
});

test("l'audit voit un défaut glissé dans une boîte", async () => {
  const window = page();
  const dialog = window.document.getElementById("add-dialog");
  dialog.setAttribute("open", "");
  dialog.insertAdjacentHTML("beforeend", '<button type="button"><svg></svg></button><input type="text">');
  const found = await audit(window, dialog);
  assert.ok(found.some((v) => v.startsWith("button-name")), found.join("\n"));
  assert.ok(found.some((v) => v.startsWith("label")), found.join("\n"));
});

const dialogs = [...new JSDOM(html).window.document.querySelectorAll("dialog")].map((d) => d.id);

test("la page a ses boîtes", () => {
  assert.ok(dialogs.length >= 20, dialogs.join(", "));
  assert.ok(dialogs.every(Boolean), "chaque boîte a un identifiant");
});

for (const id of dialogs) {
  test(`boîte ${id}`, async () => {
    const window = page();
    const dialog = window.document.getElementById(id);
    dialog.setAttribute("open", "");
    for (const hidden of dialog.querySelectorAll("[hidden]")) hidden.hidden = false;
    assert.deepEqual(await audit(window, dialog), []);
  });
}

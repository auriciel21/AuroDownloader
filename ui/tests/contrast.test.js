// Contrastes des couleurs de la page (WCAG 2.2 AA, critères 1.4.3 et 1.4.11), thème clair
// et sombre : lus dans style.css, comparés deux à deux selon leur usage réel.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const css = readFileSync(new URL("../src/style.css", import.meta.url), "utf8");

/** Les variables `--nom: #rrggbb` d'un bloc qui commence à `start`. */
function tokens(start) {
  const open = css.indexOf("{", start);
  let depth = 0;
  let end = open;
  for (; end < css.length; end++) {
    if (css[end] === "{") depth++;
    if (css[end] === "}" && --depth === 0) break;
  }
  const block = css.slice(open, end);
  return Object.fromEntries([...block.matchAll(/--([a-z-]+):\s*(#[0-9a-f]{6})\s*;/gi)].map((m) => [m[1], m[2]]));
}

const light = tokens(css.indexOf(":root"));
const darkAt = css.indexOf("@media (prefers-color-scheme: dark)");
const dark = { ...light, ...tokens(css.indexOf(":root", darkAt)) };

function luminance(hex) {
  const [r, g, b] = [1, 3, 5].map((i) => {
    const c = parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

export function ratio(a, b) {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

// Texte : 4,5:1 (1.4.3). Couleur du texte, fond, et où on les trouve ensemble.
const TEXT = [
  ["text", "bg", "fenêtre"],
  ["text", "panel", "panneaux, boîtes, champs"],
  ["text", "hover", "ligne survolée"],
  ["text", "error-bg", "bandeau d'erreur"],
  ["muted", "panel", "notes, résumé"],
  ["muted", "bg", "arborescence, liste vide"],
  ["muted", "hover", "compteurs d'une vue survolée"],
  ["select-text", "select-bg", "ligne, vue et entrée de menu choisies"],
  ["accent-text", "accent", "bouton principal"],
  ["panel", "fail", "bouton de suppression"],
  ["done", "panel", "état « Terminé »"],
  ["done", "select-bg", "état « Terminé » d'une ligne choisie"],
  ["done", "hover", "état « Terminé » d'une ligne survolée"],
  ["fail", "panel", "état « Échec », erreurs des formulaires"],
  ["fail", "select-bg", "état « Échec » d'une ligne choisie"],
  ["fail", "hover", "état « Échec » d'une ligne survolée"],
  ["warn-text", "warn-bg", "bandeau d'avertissement"],
  ["hit-text", "hit-bg", "résultat de la recherche du guide"],
  ["hit-current-text", "hit-current-bg", "résultat montré de la recherche du guide"],
];

// Composants et graphiques : 3:1 (1.4.11). Contour du focus, icônes, bords des champs.
const GRAPHIC = [
  ["focus", "panel", "contour du focus"],
  ["focus", "bg", "contour du focus"],
  ["focus", "select-bg", "contour du focus d'une ligne choisie"],
  ["accent", "panel", "icônes (Ajouter, types de fichier)"],
  ["accent", "bg", "icônes de l'arborescence"],
  ["muted", "panel", "icônes de la barre d'outils"],
  ["done", "panel", "icône Reprendre"],
  ["amber", "panel", "icône Pause"],
  ["fail", "panel", "icône Supprimer"],
  ["control-border", "panel", "bord des champs, listes et boutons"],
  ["control-border", "bg", "bord des champs, listes et boutons"],
  ["hit-current-bg", "panel", "résultat montré, parmi le texte du guide"],
];

for (const [theme, colors] of [["clair", light], ["sombre", dark]]) {
  test(`thème ${theme} : texte lisible (4,5:1)`, () => {
    const bad = TEXT.filter(([fg, bg]) => ratio(colors[fg], colors[bg]) < 4.5)
      .map(([fg, bg, where]) => `${fg} sur ${bg} (${where}) : ${ratio(colors[fg], colors[bg]).toFixed(2)}`);
    assert.deepEqual(bad, []);
  });
  test(`thème ${theme} : focus, icônes et bords visibles (3:1)`, () => {
    const bad = GRAPHIC.filter(([fg, bg]) => ratio(colors[fg], colors[bg]) < 3)
      .map(([fg, bg, where]) => `${fg} sur ${bg} (${where}) : ${ratio(colors[fg], colors[bg]).toFixed(2)}`);
    assert.deepEqual(bad, []);
  });
}

test("chaque couleur comparée existe dans les deux thèmes", () => {
  for (const [fg, bg] of [...TEXT, ...GRAPHIC]) {
    for (const colors of [light, dark]) {
      assert.ok(colors[fg] && colors[bg], `--${fg} ou --${bg} manque`);
    }
  }
});

test("le calcul suit la formule du WCAG", () => {
  assert.equal(ratio("#000000", "#ffffff").toFixed(1), "21.0");
  assert.equal(ratio("#767676", "#ffffff").toFixed(2), "4.54");
});

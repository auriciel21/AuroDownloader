// Guide de l'utilisateur (étapes 9c et 9d) : node --test scripts/tests/
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";
import { check, render, output, slug, inline, LANGS, files, headings } from "../guide.mjs";

test("copie de l'application : une capture devient son texte alternatif, le texte est échappé", () => {
  const ctx = { lang: "fr", file: "02-premiers-pas.md", prefix: (f) => `guide-${f.replace(/\.md$/, "")}` };
  assert.equal(inline("![La fenêtre <principale>](../images/fr/01-fenetre.png)", ctx),
    '<em class="guide-image">La fenêtre &lt;principale&gt;</em>');
  assert.equal(inline("voir [Options](08-options.md#général)", ctx),
    'voir <a href="#guide-08-options-général">Options</a>');
  assert.equal(inline("<script>x</script> **gras** `a<b`", ctx),
    "&lt;script&gt;x&lt;/script&gt; <strong>gras</strong> <code>a&lt;b</code>");
});

test("liens internes et ancres du guide justes, langues de même structure", () => {
  assert.deepEqual(check(), []);
  for (const lang of LANGS) assert.ok(files(lang).length >= 10, lang);
});

test("ancres comme GitHub, accents gardés", () => {
  assert.equal(slug("Réglages (Options, BitTorrent)"), "réglages-options-bittorrent");
  assert.equal(slug("Règles de rangement"), "règles-de-rangement");
  assert.equal(slug("FTP, FTPS et SFTP"), "ftp-ftps-et-sftp");
  assert.equal(slug("Settings (Options, BitTorrent)"), "settings-options-bittorrent");
});

test("copies de l'application à jour (node scripts/guide.mjs)", () => {
  for (const lang of LANGS) {
    assert.ok(existsSync(output(lang)), `${output(lang)} absent`);
    assert.equal(readFileSync(output(lang), "utf8"), render(lang), `ui/src/guide/${lang}.html pas à jour`);
  }
});

test("copie de l'application : sans script ni lien qui sorte, chaque ancre interne existe", () => {
  for (const lang of LANGS) {
    const html = render(lang);
    assert.doesNotMatch(html, /<script|on\w+=|javascript:/i);
    const hrefs = [...html.matchAll(/href="([^"]*)"/g)].map((m) => m[1]);
    assert.ok(hrefs.length > 10);
    for (const href of hrefs) {
      assert.match(href, /^#guide-/, `lien qui sortirait de l'application : ${href}`);
      assert.ok(html.includes(`id="${href.slice(1)}"`), `${lang} : ancre ${href} absente`);
    }
    // Une section par fichier, un titre de niveau 2 en tête de chacune.
    assert.equal((html.match(/<section /g) ?? []).length, files(lang).length);
    const titles = files(lang).map((f) => headings(readFileSync(new URL(`../../docs/guide/${lang}/${f}`, import.meta.url), "utf8"))[0]);
    assert.ok(titles.every((h) => h && h.level === 1), "chaque fichier commence par un titre #");
  }
});

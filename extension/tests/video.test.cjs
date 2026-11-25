// Vidéo de la page (extension/video.js), sans navigateur : node --test extension/tests/
const test = require("node:test");
const assert = require("node:assert/strict");
const path = require("node:path");
const { pathToFileURL } = require("node:url");
require("../video.js");

const { qualityChoices, formatSize, formatDuration, languageName } = globalThis.AuroVideo;

const pages = [
  {
    formats: [
      { formatId: "a1", vcodec: null, acodec: "mp4a", filesize: 1000 },
      { formatId: "a2", vcodec: null, acodec: "opus", filesize: 1500, filesizeApprox: true },
      { formatId: "v720", vcodec: "avc1", acodec: null, height: 720, filesize: 9000 },
      { formatId: "v1080", vcodec: "avc1", acodec: null, height: 1080, filesize: 20000 },
      { formatId: "hls360", vcodec: "avc1", acodec: "mp4a", height: 360 },
      { formatId: "drm4k", vcodec: "hev1", acodec: null, height: 2160, filesize: 1, drm: true },
    ],
  },
  { formats: [{ formatId: "400p", height: 400, filesize: 50 }] },
  { formats: [{ formatId: "son", vcodec: null, acodec: "opus" }] },
  { formats: [] },
];

test("qualités identiques à celles de l'application de bureau", async () => {
  const model = await import(pathToFileURL(path.join(__dirname, "../../ui/src/model.js")).href);
  for (const page of pages) assert.deepEqual(qualityChoices(page), model.qualityChoices(page));
  const choices = qualityChoices(pages[0]);
  assert.deepEqual(choices.map((c) => c.key), ["best", "h1080", "h720", "h360", "audio"]);
  assert.deepEqual(choices[0].size, { bytes: 21500, approx: true });
});

test("tailles et langues identiques à celles de l'application de bureau", async () => {
  const i18n = await import(pathToFileURL(path.join(__dirname, "../../ui/src/i18n.js")).href);
  const sizes = [0, 1, 512, 1023, 1024, 1536 * 1024, 99.96 * 1024 * 1024, 250 * 2 ** 30, 3 * 2 ** 40, 5 * 2 ** 50, null, -1, NaN];
  for (const lang of ["fr", "en"]) {
    const t = i18n.translator(lang);
    for (const n of sizes) assert.equal(formatSize(n, lang), i18n.bytes(n, lang, t), `${n} en ${lang}`);
    for (const code of ["fr", "en", "pt-BR", "zh-Hans", "qqq-invalide!", "xx"]) {
      assert.equal(languageName(code, lang), i18n.languageName(code, lang, t), `${code} en ${lang}`);
    }
  }
});

test("tailles, durées et langues lisibles", () => {
  assert.equal(formatSize(1536 * 1024, "fr"), "1,5 Mio");
  assert.equal(formatSize(1536 * 1024, "en"), "1.5 MiB");
  assert.equal(formatSize(512, "en"), "512 B");
  assert.equal(formatSize(250 * 1024 * 1024 * 1024, "en"), "250.0 GiB");
  assert.equal(formatSize(null, "en"), "?");
  assert.equal(formatDuration(65), "1:05");
  assert.equal(formatDuration(3725.4), "1:02:05");
  assert.equal(formatDuration(null), "");
  assert.equal(languageName("fr", "fr"), "français (fr)");
  assert.equal(languageName("qqq-invalide!", "fr"), "qqq-invalide!");
});

// Vidéo d'une page dans la fenêtre de l'extension (étape 5d, SPEC 4.5) : qualités
// proposées après `media.probe`, tailles et noms de langues, sans dépendre du navigateur
// (testé par extension/tests). Script classique, rangé dans `globalThis.AuroVideo`.
// Les règles des qualités sont celles de l'application de bureau (`ui/src/model.js`,
// `qualityChoices`) : les deux doivent rester identiques.
(() => {
  /// Un format a de l'image (codec connu, ou codecs inconnus mais une hauteur : yt-dlp le
  /// prend pour complet).
  const hasVideo = (f) => Boolean(f.vcodec) || (!f.vcodec && !f.acodec && f.height > 0);
  /// Un format n'a que du son.
  const audioOnly = (f) => !f.vcodec && Boolean(f.acodec);

  /// Taille d'un format : `{bytes, approx}`, ou `null` si inconnue.
  const sizeOf = (f) => (f.filesize > 0 ? { bytes: f.filesize, approx: Boolean(f.filesizeApprox) } : null);

  function addSizes(a, b) {
    if (!a) return null;
    if (!b) return a;
    return { bytes: a.bytes + b.bytes, approx: a.approx || b.approx };
  }

  /// Qualités proposées : « meilleure », chaque hauteur d'image (de la plus grande à la
  /// plus petite), puis « son seul » s'il existe. Chacune porte son sélecteur de yt-dlp
  /// (`null` : celui du moteur) et une taille estimée (image et meilleur son), ou `null`.
  /// Les formats protégés (DRM) sont écartés.
  function qualityChoices(probe) {
    const formats = (probe?.formats ?? []).filter((f) => !f.drm);
    const audios = formats.filter(audioOnly);
    const bestAudio = audios.map(sizeOf).filter(Boolean).sort((a, b) => b.bytes - a.bytes)[0] ?? null;
    const heights = [...new Set(formats.filter((f) => hasVideo(f) && f.height > 0).map((f) => f.height))]
      .sort((a, b) => b - a);
    const sizeAt = (height) => {
      const sizes = formats
        .filter((f) => hasVideo(f) && f.height === height)
        .map((f) => {
          const own = sizeOf(f);
          // Image seule : le son s'y ajoute.
          return f.vcodec && !f.acodec ? addSizes(own, bestAudio) : own;
        })
        .filter(Boolean)
        .sort((a, b) => b.bytes - a.bytes);
      return sizes[0] ?? null;
    };
    const choices = [{ key: "best", format: null, height: null, size: heights.length ? sizeAt(heights[0]) : null }];
    for (const height of heights) {
      choices.push({
        key: `h${height}`,
        format: `bv*[height<=${height}]+ba/b[height<=${height}]`,
        height,
        size: sizeAt(height),
      });
    }
    if (audios.length) choices.push({ key: "audio", format: "ba", height: null, size: bestAudio });
    return choices;
  }

  /// Taille lisible : unités binaires, notées à la française (« Mio ») en français, une
  /// décimale au-delà de l'octet ; « ? » si inconnue. Recopie de `bytes` (`ui/src/i18n.js`),
  /// comparée par les tests.
  function formatSize(bytes, lang = "en") {
    const fr = String(lang).toLowerCase().startsWith("fr");
    const units = fr ? ["o", "Kio", "Mio", "Gio", "Tio"] : ["B", "KiB", "MiB", "GiB", "TiB"];
    let value = bytes;
    if (value == null || value < 0 || !Number.isFinite(value)) return "?";
    let unit = 0;
    while (value >= 1024 && unit < units.length - 1) {
      value /= 1024;
      unit += 1;
    }
    const digits = unit === 0 ? 0 : 1;
    const number = value.toLocaleString(lang, { minimumFractionDigits: digits, maximumFractionDigits: digits });
    return `${number} ${units[unit]}`;
  }

  /// Durée lisible : `m:ss` ou `h:mm:ss` ; `""` si inconnue.
  function formatDuration(seconds) {
    const s = Math.round(Number(seconds));
    if (!Number.isFinite(s) || s <= 0) return "";
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    const pad = (n) => String(n).padStart(2, "0");
    return h ? `${h}:${pad(m)}:${pad(s % 60)}` : `${m}:${pad(s % 60)}`;
  }

  /// Nom d'une langue dans celle de l'interface, sinon son code. Recopie de `languageName`
  /// (`ui/src/i18n.js`), comparée par les tests.
  function languageName(code, lang = "en") {
    try {
      const name = new Intl.DisplayNames([lang], { type: "language" }).of(code);
      return name && name !== code ? `${name} (${code})` : code;
    } catch {
      return code;
    }
  }

  globalThis.AuroVideo = { qualityChoices, formatSize, formatDuration, languageName };
})();

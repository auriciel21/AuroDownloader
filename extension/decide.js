// Décision de capture d'un téléchargement (SPEC 4.5, F-37), sans dépendre du navigateur :
// testée seule (extension/tests). Script classique : chargé par `importScripts` (Chrome,
// Edge) ou par la liste `background.scripts` (Firefox), il se range dans
// `globalThis.AuroDecide`.
(() => {
  /// Réglages par défaut (décision du 2026-11-07) : tout au-dessus de 1 Mio est capturé.
  const DEFAULTS = Object.freeze({
    capture: true,
    minSizeMiB: 1,
    excludedSites: [],
    captureIncognito: false,
    // Cookies, page d'origine et navigateur transmis avec l'adresse (SPEC 4.5) : un
    // téléchargement qui demande d'être connecté réussit aussi dans AuroDownloader.
    sendCookies: true,
  });

  /// Délai pendant lequel un clic avec Alt laisse le téléchargement au navigateur.
  const ALT_WINDOW_MS = 3000;

  /// Réglages lus, complétés et nettoyés.
  function settings(stored) {
    const s = { ...DEFAULTS, ...(stored ?? {}) };
    const size = Number(s.minSizeMiB);
    return {
      capture: s.capture !== false,
      minSizeMiB: Number.isFinite(size) && size >= 0 ? size : DEFAULTS.minSizeMiB,
      excludedSites: (Array.isArray(s.excludedSites) ? s.excludedSites : [])
        .map((site) => String(site).trim().toLowerCase())
        .filter(Boolean),
      captureIncognito: s.captureIncognito === true,
      sendCookies: s.sendCookies !== false,
    };
  }

  /// Hôte d'une adresse, en minuscules ; `null` si elle n'en a pas.
  function host(url) {
    try {
      return new URL(url).hostname.toLowerCase() || null;
    } catch {
      return null;
    }
  }

  /// Le site exclu couvre cet hôte : lui-même ou un de ses sous-domaines.
  function covers(site, hostname) {
    const bare = site.replace(/^\*\./, "").replace(/^https?:\/\//, "").replace(/\/.*$/, "");
    return hostname === bare || hostname.endsWith(`.${bare}`);
  }

  /// Faut-il prendre ce téléchargement au navigateur ? Renvoie `{capture, reason}`.
  /// `item` : ce que l'API `downloads` décrit (`url`, `finalUrl`, `totalBytes`,
  /// `fileSize`, `incognito`, `byExtensionId`, `state`) ; `altClickAt` : instant du
  /// dernier clic avec Alt dans une page, en millisecondes.
  function decide(item, stored, { now = Date.now(), altClickAt = 0, selfId = null } = {}) {
    const s = settings(stored);
    const url = item.finalUrl || item.url || "";
    if (!s.capture) return { capture: false, reason: "capture désactivée" };
    if (selfId && item.byExtensionId === selfId) return { capture: false, reason: "rendu au navigateur" };
    if (item.state && item.state !== "in_progress") return { capture: false, reason: "déjà fini" };
    if (!/^https?:/i.test(url)) return { capture: false, reason: "adresse non HTTP" };
    if (item.incognito && !s.captureIncognito) return { capture: false, reason: "navigation privée" };
    if (now - altClickAt >= 0 && now - altClickAt < ALT_WINDOW_MS) {
      return { capture: false, reason: "clic avec Alt" };
    }
    const hostname = host(url);
    if (hostname && s.excludedSites.some((site) => covers(site, hostname))) {
      return { capture: false, reason: "site exclu" };
    }
    // Taille inconnue (0 ou -1) : capturé, le moteur la découvrira.
    const size = Math.max(Number(item.totalBytes) || 0, Number(item.fileSize) || 0);
    if (size > 0 && size < s.minSizeMiB * 1024 * 1024) return { capture: false, reason: "trop petit" };
    return { capture: true, reason: "" };
  }

  /// Clé d'idempotence d'une demande : un nouvel essai pour le même téléchargement du
  /// navigateur ne crée pas de doublon dans le moteur.
  function requestId(item) {
    return `browser-${item.id}-${item.startTime ?? ""}`;
  }

  /// `Referer` que le navigateur lui-même enverrait (politique par défaut,
  /// `strict-origin-when-cross-origin`) : l'adresse entière vers la même origine,
  /// l'origine seule vers une autre, rien de HTTPS vers HTTP ; `null` sans page.
  function refererFor(page, target) {
    let from;
    let to;
    try {
      from = new URL(page);
      to = new URL(target);
    } catch {
      return null;
    }
    if (!/^https?:$/.test(from.protocol)) return null;
    if (from.protocol === "https:" && to.protocol === "http:") return null;
    if (from.origin === to.origin) {
      from.hash = "";
      from.username = "";
      from.password = "";
      return from.href;
    }
    return `${from.origin}/`;
  }

  /// En-tête `Cookie` tel que le navigateur le formerait : les chemins les plus longs
  /// d'abord (RFC 6265, 5.4) ; `""` sans cookie.
  function cookieHeader(cookies) {
    return [...(cookies ?? [])]
      .sort((a, b) => (b.path?.length ?? 0) - (a.path?.length ?? 0))
      .map((c) => (c.name ? `${c.name}=${c.value}` : c.value))
      .join("; ");
  }

  /// En-têtes transmis au moteur avec l'adresse `url` : `Referer` (page d'origine),
  /// `User-Agent` et `Cookie` ; rien de tout cela si l'option est coupée.
  function pageHeaders(stored, { url, page, userAgent, cookies }) {
    if (!settings(stored).sendCookies) return {};
    const headers = {};
    const referer = page ? refererFor(page, url) : null;
    if (referer) headers.Referer = referer;
    if (userAgent) headers["User-Agent"] = userAgent;
    const cookie = cookieHeader(cookies);
    if (cookie) headers.Cookie = cookie;
    return headers;
  }

  /// Magasin de cookies d'un téléchargement : celui que Firefox indique (conteneur,
  /// navigation privée), sinon celui de la navigation privée de Chrome et Edge (« 1 »),
  /// sinon le magasin par défaut (`undefined`).
  function cookieStore(item) {
    if (item.cookieStoreId) return item.cookieStoreId;
    return item.incognito ? "1" : undefined;
  }

  globalThis.AuroDecide = {
    DEFAULTS,
    ALT_WINDOW_MS,
    settings,
    decide,
    requestId,
    host,
    refererFor,
    cookieHeader,
    pageHeaders,
    cookieStore,
  };

})();

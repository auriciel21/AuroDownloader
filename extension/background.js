// Arrière-plan de l'extension (SPEC 4.5, F-37) : service worker sous Chrome et Edge,
// page d'arrière-plan sous Firefox. Capte les téléchargements selon les réglages
// (`decide.js`) et les confie à AuroDownloader par le relais de messagerie native
// (`auro-relay`). Le téléchargement du navigateur n'est annulé qu'une fois l'ajout
// accepté : si le relais manque ou refuse, le navigateur continue seul.
// Étape 5d : liens magnet (`content.js`), fichiers `.torrent` lus par l'extension, vidéo
// d'une page (fenêtre `popup.html`, menu contextuel).
if (typeof importScripts === "function" && !globalThis.AuroDecide) importScripts("decide.js");

const {
  decide,
  settings,
  requestId,
  pageHeaders,
  cookieStore,
  isMagnet,
  givesBackMagnet,
  torrentProblem,
  toBase64,
  MAX_TORRENT_BYTES,
} = globalThis.AuroDecide;
const HOST = "org.aurodownloader.relay";
const t = (key, ...subs) => chrome.i18n.getMessage(key, subs) || key;
const freshKey = (kind) => `${kind}-${Date.now()}-${Math.random().toString(36).slice(2)}`;

/// Dernier clic avec Alt dans une page (`content.js`).
let altClickAt = 0;

/// Badge de l'icône, quelques secondes : « ✓ » envoyé, « ! » refusé.
function flash(text, color) {
  chrome.action.setBadgeBackgroundColor({ color }).catch(() => {});
  chrome.action.setBadgeText({ text }).catch(() => {});
  setTimeout(() => chrome.action.setBadgeText({ text: "" }).catch(() => {}), 4000);
}

/// Envoie au relais ; un relais absent ou en panne donne une réponse de refus.
async function relay(message) {
  try {
    return await chrome.runtime.sendNativeMessage(HOST, message);
  } catch (e) {
    return { ok: false, code: "RELAY_MISSING", message: String(e?.message ?? e) };
  }
}

/// Garde le résultat du dernier envoi, que les options affichent.
function remember(url, reply) {
  const last = { at: Date.now(), url, ok: reply?.ok === true, code: reply?.code ?? "", message: reply?.message ?? "" };
  chrome.storage.local.set({ last }).catch(() => {});
}

/// Envoie une demande d'ajout au relais, note le résultat et le montre sur l'icône ;
/// rend la réponse.
async function submit(message, shown) {
  const reply = await relay(message);
  remember(shown, reply);
  if (reply?.ok) flash("✓", "#2b6cb0");
  else flash("!", "#b45309");
  return reply;
}

/// Cookies que le navigateur enverrait à `url`, dans le magasin `storeId` ; aucun si
/// l'accès aux sites n'est pas accordé (Firefox le demande à part).
async function cookiesFor(url, storeId) {
  if (!chrome.cookies) return [];
  const query = storeId ? { url, storeId } : { url };
  try {
    return await chrome.cookies.getAll(query);
  } catch {
    // Firefox, isolation par site activée : le domaine de premier niveau est exigé.
    try {
      return await chrome.cookies.getAll({ ...query, firstPartyDomain: null });
    } catch {
      return [];
    }
  }
}

/// Confie une adresse à AuroDownloader, avec les en-têtes de sa page ; vrai si le moteur
/// l'a acceptée. Un lien magnet part seul : ni cookies ni page pour un torrent.
async function hand(url, key, { page, storeId } = {}) {
  if (isMagnet(url)) return (await submit({ type: "add", url, requestId: key }, url))?.ok === true;
  const stored = await chrome.storage.sync.get(null).catch(() => ({}));
  const headers = pageHeaders(stored, {
    url,
    page,
    userAgent: navigator.userAgent,
    cookies: await cookiesFor(url, storeId),
  });
  return (await submit({ type: "add", url, requestId: key, headers }, url))?.ok === true;
}

/// Lit un fichier `.torrent` comme le navigateur (ses cookies pour ce site) ; rend ses
/// octets, ou lève une erreur.
async function readTorrent(url) {
  const response = await fetch(url, { credentials: "include", cache: "no-store" });
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  const announced = Number(response.headers.get("content-length"));
  if (announced > MAX_TORRENT_BYTES) throw new Error("fichier .torrent de plus de 16 Mio");
  const bytes = new Uint8Array(await response.arrayBuffer());
  const problem = torrentProblem(bytes);
  if (problem) throw new Error(problem);
  return bytes;
}

/// Confie un `.torrent` du navigateur : lu ici, envoyé en base64 ; vrai si le moteur en
/// a fait un torrent.
async function handTorrent(url, key) {
  let bytes;
  try {
    bytes = await readTorrent(url);
  } catch (e) {
    const reply = { ok: false, code: "TORRENT_UNREADABLE", message: String(e?.message ?? e) };
    remember(url, reply);
    flash("!", "#b45309");
    return false;
  }
  return (await submit({ type: "add", torrent: toBase64(bytes), requestId: key }, url))?.ok === true;
}

/// Ajoute la vidéo d'une page ; rend la réponse du relais.
function handVideo({ pageUrl, format, subtitles, title }) {
  const message = { type: "video", pageUrl, requestId: freshKey("video") };
  if (format) message.format = format;
  if (subtitles?.length) message.subtitles = subtitles;
  if (title) message.title = title;
  return submit(message, pageUrl);
}

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  switch (message?.type) {
    case "altClick":
      altClickAt = Date.now();
      return false;
    // Clic sur un lien magnet (`content.js`) : la page attend la réponse, pour rendre le
    // lien au navigateur si AuroDownloader est injoignable (`givesBackMagnet`).
    case "magnet": {
      if (!isMagnet(message.url)) {
        sendResponse({ ok: false, giveBack: true });
        return false;
      }
      (async () => {
        // Fenêtre privée : capté seulement avec « captureIncognito », comme un
        // téléchargement ; sinon rendu au navigateur (audit W9, décision 8).
        if (sender.tab?.incognito) {
          const stored = await chrome.storage.sync.get(null).catch(() => ({}));
          if (!settings(stored).captureIncognito) return { ok: false, giveBack: true };
        }
        const reply = await submit({ type: "add", url: message.url, requestId: freshKey("magnet") }, message.url);
        return { ok: reply?.ok === true, giveBack: givesBackMagnet(reply) };
      })().then(sendResponse);
      return true;
    }
    // Fenêtre de l'extension (`popup.js`).
    case "video": {
      if (sender.tab) return false;
      handVideo(message).then(sendResponse);
      return true;
    }
    default:
      return false;
  }
});

chrome.downloads.onCreated.addListener(async (item) => {
  const stored = await chrome.storage.sync.get(null).catch(() => ({}));
  const verdict = decide(item, stored, { altClickAt, selfId: chrome.runtime.id });
  if (!verdict.capture) return;
  const url = item.finalUrl || item.url;
  if (verdict.torrent) {
    // Le navigateur attend pendant la lecture ; il reprend si l'ajout échoue. Un
    // `.torrent` déjà fini reste où le navigateur l'a mis.
    await chrome.downloads.pause(item.id).catch(() => {});
    if (await handTorrent(url, requestId(item))) {
      const cancelled = await chrome.downloads.cancel(item.id).then(() => true, () => false);
      if (cancelled) await chrome.downloads.erase({ id: item.id }).catch(() => {});
    } else {
      await chrome.downloads.resume(item.id).catch(() => {});
    }
    return;
  }
  const page = item.referrer || undefined;
  if (await hand(url, requestId(item), { page, storeId: cookieStore(item) })) {
    // Effacée seulement si l'annulation a pris : un téléchargement que le navigateur a
    // déjà fini reste dans sa liste, au lieu d'un fichier en double invisible (audit W8).
    const cancelled = await chrome.downloads.cancel(item.id).then(() => true, () => false);
    if (cancelled) await chrome.downloads.erase({ id: item.id }).catch(() => {});
  }
});

// Menus contextuels : un lien, une image, une vidéo, un son (lien magnet compris) ; la
// vidéo de la page, en meilleure qualité. Recréés à chaque démarrage de l'arrière-plan, et
// pas seulement à l'installation : l'essai sous Edge du 2026-11-10 a montré une extension
// mise à jour sans son entrée vidéo, revenue seulement après un rechargement.
function createMenus() {
  chrome.contextMenus.removeAll(() => {
    chrome.contextMenus.create({
      id: "auro-download",
      title: t("menuDownload"),
      contexts: ["link", "image", "video", "audio"],
    });
    chrome.contextMenus.create({
      id: "auro-video",
      title: t("menuVideo"),
      contexts: ["page", "video"],
      documentUrlPatterns: ["http://*/*", "https://*/*"],
    });
  });
}
createMenus();

chrome.contextMenus.onClicked.addListener((info, tab) => {
  if (info.menuItemId === "auro-video") {
    const pageUrl = info.pageUrl || tab?.url;
    if (pageUrl && /^https?:/i.test(pageUrl)) handVideo({ pageUrl });
    return;
  }
  if (info.menuItemId !== "auro-download") return;
  const url = info.linkUrl || info.srcUrl;
  if (!url || !(/^https?:/i.test(url) || isMagnet(url))) return;
  const storeId = tab?.cookieStoreId ?? (tab?.incognito ? "1" : undefined);
  hand(url, freshKey("menu"), { page: info.frameUrl || info.pageUrl, storeId });
});

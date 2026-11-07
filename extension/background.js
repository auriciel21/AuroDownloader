// Arrière-plan de l'extension (SPEC 4.5, F-37) : service worker sous Chrome et Edge,
// page d'arrière-plan sous Firefox. Capte les téléchargements selon les réglages
// (`decide.js`) et les confie à AuroDownloader par le relais de messagerie native
// (`auro-relay`). Le téléchargement du navigateur n'est annulé qu'une fois l'ajout
// accepté : si le relais manque ou refuse, le navigateur continue seul.
if (typeof importScripts === "function" && !globalThis.AuroDecide) importScripts("decide.js");

const { decide, requestId, pageHeaders, cookieStore } = globalThis.AuroDecide;
const HOST = "org.aurodownloader.relay";
const t = (key, ...subs) => chrome.i18n.getMessage(key, subs) || key;

/// Dernier clic avec Alt dans une page (`content.js`).
let altClickAt = 0;

chrome.runtime.onMessage.addListener((message) => {
  if (message?.type === "altClick") altClickAt = Date.now();
});

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
/// l'a acceptée.
async function hand(url, key, { page, storeId } = {}) {
  const stored = await chrome.storage.sync.get(null).catch(() => ({}));
  const headers = pageHeaders(stored, {
    url,
    page,
    userAgent: navigator.userAgent,
    cookies: await cookiesFor(url, storeId),
  });
  const reply = await relay({ type: "add", url, requestId: key, headers });
  remember(url, reply);
  if (reply?.ok) flash("✓", "#2b6cb0");
  else flash("!", "#b45309");
  return reply?.ok === true;
}

chrome.downloads.onCreated.addListener(async (item) => {
  const stored = await chrome.storage.sync.get(null).catch(() => ({}));
  const verdict = decide(item, stored, { altClickAt, selfId: chrome.runtime.id });
  if (!verdict.capture) return;
  const page = item.referrer || undefined;
  if (await hand(item.finalUrl || item.url, requestId(item), { page, storeId: cookieStore(item) })) {
    await chrome.downloads.cancel(item.id).catch(() => {});
    await chrome.downloads.erase({ id: item.id }).catch(() => {});
  }
});

// Menu contextuel des liens et des médias.
chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.removeAll(() => {
    chrome.contextMenus.create({
      id: "auro-download",
      title: t("menuDownload"),
      contexts: ["link", "image", "video", "audio"],
    });
  });
});

chrome.contextMenus.onClicked.addListener((info, tab) => {
  if (info.menuItemId !== "auro-download") return;
  const url = info.linkUrl || info.srcUrl;
  if (!url || !/^https?:/i.test(url)) return;
  const key = `menu-${Date.now()}-${Math.random().toString(36).slice(2)}`;
  const storeId = tab?.cookieStoreId ?? (tab?.incognito ? "1" : undefined);
  hand(url, key, { page: info.frameUrl || info.pageUrl, storeId });
});

// L'icône ouvre les réglages.
chrome.action.onClicked.addListener(() => chrome.runtime.openOptionsPage());

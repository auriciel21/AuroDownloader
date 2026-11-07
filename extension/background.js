// Arrière-plan de l'extension (SPEC 4.5, F-37) : service worker sous Chrome et Edge,
// page d'arrière-plan sous Firefox. Capte les téléchargements selon les réglages
// (`decide.js`) et les confie à AuroDownloader par le relais de messagerie native
// (`auro-relay`). Le téléchargement du navigateur n'est annulé qu'une fois l'ajout
// accepté : si le relais manque ou refuse, le navigateur continue seul.
if (typeof importScripts === "function" && !globalThis.AuroDecide) importScripts("decide.js");

const { decide, requestId } = globalThis.AuroDecide;
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

/// Confie une adresse à AuroDownloader ; vrai si le moteur l'a acceptée.
async function hand(url, key) {
  const reply = await relay({ type: "add", url, requestId: key });
  remember(url, reply);
  if (reply?.ok) flash("✓", "#2b6cb0");
  else flash("!", "#b45309");
  return reply?.ok === true;
}

chrome.downloads.onCreated.addListener(async (item) => {
  const stored = await chrome.storage.sync.get(null).catch(() => ({}));
  const verdict = decide(item, stored, { altClickAt, selfId: chrome.runtime.id });
  if (!verdict.capture) return;
  if (await hand(item.finalUrl || item.url, requestId(item))) {
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

chrome.contextMenus.onClicked.addListener((info) => {
  if (info.menuItemId !== "auro-download") return;
  const url = info.linkUrl || info.srcUrl;
  if (url && /^https?:/i.test(url)) hand(url, `menu-${Date.now()}-${Math.random().toString(36).slice(2)}`);
});

// L'icône ouvre les réglages.
chrome.action.onClicked.addListener(() => chrome.runtime.openOptionsPage());

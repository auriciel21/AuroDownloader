// Réglages de l'extension : capture, taille minimale, sites exclus, navigation privée,
// cookies de la page ; état de la liaison avec AuroDownloader (relais de messagerie
// native) ; accès aux sites, pour les cookies.
const { DEFAULTS, settings } = globalThis.AuroDecide;
const HOST = "org.aurodownloader.relay";
const t = (key, ...subs) => chrome.i18n.getMessage(key, subs) || key;
const $ = (id) => document.getElementById(id);

for (const el of document.querySelectorAll("[data-i18n]")) el.textContent = t(el.dataset.i18n);
for (const el of document.querySelectorAll("[data-i18n-placeholder]")) el.placeholder = t(el.dataset.i18nPlaceholder);
document.documentElement.lang = chrome.i18n.getUILanguage();

async function load() {
  const s = settings(await chrome.storage.sync.get(null));
  $("capture").checked = s.capture;
  $("minSizeMiB").value = String(s.minSizeMiB);
  $("excludedSites").value = s.excludedSites.join("\n");
  $("captureIncognito").checked = s.captureIncognito;
  $("sendCookies").checked = s.sendCookies;
}

let savedTimer = 0;
async function save() {
  const value = Number($("minSizeMiB").value);
  await chrome.storage.sync.set({
    capture: $("capture").checked,
    minSizeMiB: Number.isFinite(value) && value >= 0 ? value : DEFAULTS.minSizeMiB,
    excludedSites: $("excludedSites").value.split(/[\s,]+/).filter(Boolean),
    captureIncognito: $("captureIncognito").checked,
    sendCookies: $("sendCookies").checked,
  });
  $("saved").textContent = t("saved");
  clearTimeout(savedTimer);
  savedTimer = setTimeout(() => { $("saved").textContent = ""; }, 2000);
}

async function check() {
  $("link").textContent = "…";
  try {
    const reply = await chrome.runtime.sendNativeMessage(HOST, { type: "ping" });
    $("link").textContent = reply?.ok ? t("linkOk", reply.version ?? "?") : t("linkBad", reply?.message ?? "");
  } catch (e) {
    $("link").textContent = t("linkMissing", String(e?.message ?? e));
  }
}

async function showLast() {
  const { last } = await chrome.storage.local.get("last");
  if (!last) return;
  const when = new Date(last.at).toLocaleString(chrome.i18n.getUILanguage());
  $("last").textContent = last.ok
    ? t("lastOk", when, last.url)
    : t("lastBad", when, last.url, `${last.message} [${last.code}]`);
}

$("form").addEventListener("change", save);
$("form").addEventListener("submit", (e) => e.preventDefault());
$("check").addEventListener("click", check);
load();
showLast();
check();

/// Accès aux cookies des sites : accordé à l'installation sous Chrome et Edge ; Firefox
/// peut le laisser à accorder ici.
const ALL_SITES = { origins: ["<all_urls>"] };
async function showAccess() {
  const granted = await chrome.permissions.contains(ALL_SITES).catch(() => true);
  $("access").hidden = granted;
}
$("grant").addEventListener("click", async () => {
  await chrome.permissions.request(ALL_SITES).catch(() => false);
  showAccess();
});
showAccess();

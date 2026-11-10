// Fenêtre de l'extension (étape 5d, SPEC 4.5) : la vidéo de la page ouverte. À
// l'ouverture, la page est décrite par le moteur (`probe`, par le relais) ; l'utilisateur
// choisit la qualité et les sous-titres, puis l'ajout passe par l'arrière-plan, qui le
// montre sur l'icône et le note pour les options.
const { qualityChoices, formatSize, formatDuration, languageName } = globalThis.AuroVideo;
const HOST = "org.aurodownloader.relay";
const t = (key, ...subs) => chrome.i18n.getMessage(key, subs) || key;
const $ = (id) => document.getElementById(id);
const lang = chrome.i18n.getUILanguage();

for (const el of document.querySelectorAll("[data-i18n]")) el.textContent = t(el.dataset.i18n);
document.documentElement.lang = lang;

/// Page décrite, et adresse de la page.
let probe = null;
let pageUrl = "";

/// Texte d'un refus du relais ou du moteur.
function refusalText(reply) {
  switch (reply?.code) {
    case "RELAY_MISSING":
      return t("popupRelayMissing");
    case "TOOLS_MISSING":
      return t("popupToolsMissing");
    case "CAPABILITY_MISSING":
      return t("popupUnsupported");
    case "EXTRACTOR_FAILED":
      return t("popupNoVideo", reply.message ?? "");
    default:
      return t("popupFailed", `${reply?.message ?? ""} [${reply?.code ?? "?"}]`);
  }
}

function status(text) {
  $("status").textContent = text;
}

function failed(text) {
  status(text);
  $("choice").hidden = true;
  $("retry").hidden = false;
}

function qualityLabel(choice) {
  if (choice.key === "best") return t("popupBest");
  if (choice.key === "audio") return t("popupAudio");
  return `${choice.height}p`;
}

function sizeLabel(size) {
  if (!size) return "";
  const text = formatSize(size.bytes, lang);
  return size.approx ? `≈ ${text}` : text;
}

function show() {
  $("title").textContent = probe.title || pageUrl;
  $("duration").textContent = formatDuration(probe.durationS);
  const qualities = $("qualities");
  qualities.replaceChildren();
  qualityChoices(probe).forEach((choice, i) => {
    const label = document.createElement("label");
    const input = document.createElement("input");
    input.type = "radio";
    input.name = "quality";
    input.value = choice.format ?? "";
    input.checked = i === 0;
    const name = document.createElement("span");
    name.textContent = qualityLabel(choice);
    const size = document.createElement("span");
    size.className = "size";
    size.textContent = sizeLabel(choice.size);
    label.append(input, name, size);
    qualities.append(label);
  });
  const subs = probe.subtitles ?? [];
  $("subs").replaceChildren(
    ...subs.map((code) => {
      const label = document.createElement("label");
      const input = document.createElement("input");
      input.type = "checkbox";
      input.value = code;
      const name = document.createElement("span");
      name.textContent = languageName(code, lang);
      label.append(input, name);
      return label;
    }),
  );
  $("subs-box").hidden = subs.length === 0;
  $("choice").hidden = false;
  $("retry").hidden = true;
  status("");
  qualities.querySelector("input")?.focus();
}

async function read() {
  $("retry").hidden = true;
  $("choice").hidden = true;
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true }).catch(() => []);
  pageUrl = tab?.url ?? "";
  if (!/^https?:/i.test(pageUrl)) {
    status(t("popupNoPage"));
    return;
  }
  status(t("popupReading"));
  let reply;
  try {
    reply = await chrome.runtime.sendNativeMessage(HOST, { type: "probe", pageUrl });
  } catch (e) {
    reply = { ok: false, code: "RELAY_MISSING", message: String(e?.message ?? e) };
  }
  if (!reply?.ok) {
    failed(refusalText(reply));
    return;
  }
  const formats = reply.formats ?? [];
  if (!formats.length) {
    failed(t("popupNoVideo", ""));
    return;
  }
  if (!formats.some((f) => !f.drm)) {
    failed(t("popupProtected"));
    return;
  }
  probe = reply;
  show();
}

$("choice").addEventListener("submit", async (event) => {
  event.preventDefault();
  const format = document.querySelector('input[name="quality"]:checked')?.value || null;
  const subtitles = [...$("subs").querySelectorAll("input:checked")].map((b) => b.value);
  $("download").disabled = true;
  status(t("popupAdding"));
  let reply;
  try {
    reply = await chrome.runtime.sendMessage({ type: "video", pageUrl, format, subtitles, title: probe.title || null });
  } catch (e) {
    reply = { ok: false, code: "EXTENSION", message: String(e?.message ?? e) };
  }
  $("download").disabled = false;
  if (reply?.ok) {
    status(t("popupAdded"));
    $("choice").hidden = true;
  } else {
    status(refusalText(reply));
  }
});

$("retry").addEventListener("click", read);
$("options").addEventListener("click", (event) => {
  event.preventDefault();
  chrome.runtime.openOptionsPage();
  window.close();
});
read();

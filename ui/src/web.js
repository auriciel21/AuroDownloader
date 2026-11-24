// Hôte de la page servie par le moteur (interface distante, SPEC §7.19, étape 3c) : le
// WebSocket `/ws` de ce serveur à la place de la liaison Tauri. Même contrat que
// `tauriHost` (host.js), sans dossier, ouverture de fichier, zone de notification,
// démarrage avec la session, presse-papiers ni association ; les fichiers (.torrent,
// Metalink, listes) sont choisis sur l'appareil par le navigateur.
//
// En plus : `remote` (vrai), `session()`, `login(token)`, `logout()`, `tokenFromHash()`.

import { scopeAllows, sessionError, takeHashToken } from "./model.js";

// Intervalle de l'avancement demandé au moteur : un peu plus large que celui du bureau, le
// réseau est peut-être celui d'un téléphone.
const PROGRESS_INTERVAL_MS = 1000;
// Délais entre deux tentatives de connexion : doublés à chaque échec, jusqu'au plafond.
const RETRY_FIRST_MS = 1000;
const RETRY_MAX_MS = 15000;
// Préférences de la page, propres à ce navigateur (décision de l'utilisateur).
const PREFS_KEY = "aurodownloader.prefs";

const engineDown = (message) => ({ code: "ENGINE_DOWN", message });

export function webHost() {
  let listener = null;
  let socket = null;
  let status = { state: "connecting" };
  let nextId = 1;
  const waiting = new Map();
  let retry = RETRY_FIRST_MS;
  let started = false;
  // De l'avancement omis pendant que la page était cachée : recharger l'état au retour.
  let missed = false;
  // Fichiers choisis sur l'appareil, rendus par leur « chemin » (`web:<n>/<nom>`).
  const picked = new Map();
  let pickedCount = 0;

  const emit = (event) => listener?.(event);
  const setStatus = (next) => {
    status = next;
    emit({ kind: "status", ...next });
  };

  function send(message) {
    socket.send(JSON.stringify(message));
  }

  function request(method, params) {
    return new Promise((resolve, reject) => {
      if (!socket || socket.readyState !== WebSocket.OPEN) {
        reject(engineDown("connexion au moteur fermée"));
        return;
      }
      const id = nextId++;
      waiting.set(id, { resolve, reject });
      send({ jsonrpc: "2.0", id, method, params });
    });
  }

  function onMessage(event, early) {
    let message;
    try {
      message = JSON.parse(event.data);
    } catch {
      return;
    }
    if (message.id !== undefined && message.id !== null && !message.method) {
      const pending = waiting.get(message.id);
      if (!pending) return;
      waiting.delete(message.id);
      if (message.error) {
        pending.reject({
          code: message.error.data?.code ?? "INTERNAL",
          message: message.error.message ?? "",
        });
      } else {
        pending.resolve(message.result);
      }
      return;
    }
    if (early) {
      early.push(message);
      return;
    }
    relay(message);
  }

  function relay(message) {
    if (message.method === "task.progress" && document.hidden) {
      missed = true;
      return;
    }
    emit({ kind: "message", message });
  }

  function connect() {
    const scheme = location.protocol === "https:" ? "wss" : "ws";
    const ws = new WebSocket(`${scheme}://${location.host}/ws`);
    socket = ws;
    // Notifications arrivées pendant l'ouverture : relayées après l'annonce.
    let early = [];
    ws.addEventListener("message", (event) => onMessage(event, early));
    ws.addEventListener("open", async () => {
      try {
        const engine = await request("engine.hello", {
          client: { name: "auro-web", version: "1.0" },
          protocol: { min: "1.0", max: "1.0" },
        });
        // Un jeton « Ajouter seulement » n'a pas le droit de suivre les événements : sans
        // cette garde, le refus fermait la connexion et la page se reconnectait sans fin
        // (audit W2). La page fait alors sans état des tâches (`caps.read`).
        if (scopeAllows(engine.scope ?? "admin", "read")) {
          await request("events.subscribe", {
            topics: ["tasks", "queues", "progress"],
            progressIntervalMs: PROGRESS_INTERVAL_MS,
          });
        }
        retry = RETRY_FIRST_MS;
        setStatus({ state: "up", engine });
        const held = early;
        early = null;
        for (const message of held) relay(message);
      } catch (e) {
        ws.close();
      }
    });
    ws.addEventListener("close", () => {
      if (socket !== ws) return;
      socket = null;
      for (const pending of waiting.values()) pending.reject(engineDown("connexion perdue"));
      waiting.clear();
      setStatus({ state: "down", message: "connexion perdue" });
      setTimeout(reconnect, retry);
      retry = Math.min(retry * 2, RETRY_MAX_MS);
    });
  }

  async function reconnect() {
    // Session expirée ou fermée ailleurs : la page repart sur la connexion.
    const who = await session().catch(() => undefined);
    if (who === null) {
      location.reload();
      return;
    }
    setStatus({ state: "connecting" });
    connect();
  }

  document.addEventListener("visibilitychange", () => {
    if (!document.hidden && missed) {
      missed = false;
      emit({ kind: "resync" });
    }
  });

  /// La session de ce navigateur : `{name, scope}`, ou null s'il n'en a pas (ou plus).
  async function session() {
    const response = await fetch("/session", { credentials: "same-origin", cache: "no-store" });
    if (response.status === 401) return null;
    if (!response.ok) throw engineDown(`serveur : ${response.status}`);
    return response.json();
  }

  /// Préférences reçues depuis le chargement : elles valent même quand le navigateur refuse
  /// de les garder (navigation privée), sans quoi la langue choisie était ignorée (audit W12).
  let kept = null;

  function loadPrefs() {
    if (kept) return kept;
    try {
      return { lang: "", ...JSON.parse(localStorage.getItem(PREFS_KEY) ?? "{}") };
    } catch {
      return { lang: "" };
    }
  }

  function info() {
    const prefs = loadPrefs();
    const wanted = prefs.lang || navigator.language || "";
    return {
      version: "",
      platform: "web",
      prefs,
      lang: wanted.toLowerCase().startsWith("fr") ? "fr" : "en",
      tray: false,
      autostart: false,
      executableExtensions: [],
    };
  }

  function pickFiles(accept, multiple) {
    return new Promise((resolve) => {
      const input = document.createElement("input");
      input.type = "file";
      input.accept = accept;
      input.multiple = multiple;
      input.addEventListener("change", () => resolve([...input.files]), { once: true });
      input.addEventListener("cancel", () => resolve([]), { once: true });
      input.click();
    });
  }

  function remember(file) {
    const path = `web:${pickedCount++}/${file.name}`;
    picked.set(path, file);
    return path;
  }

  function pickedFile(path) {
    const file = picked.get(path);
    if (!file) throw { code: "INVALID_PARAMS", message: "fichier choisi introuvable" };
    picked.delete(path);
    return file;
  }

  async function base64(file) {
    const bytes = new Uint8Array(await file.arrayBuffer());
    let text = "";
    for (let i = 0; i < bytes.length; i += 0x8000) {
      text += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
    }
    return btoa(text);
  }

  return {
    remote: true,
    call: (method, params = {}) => request(method, params),
    listen: async (fn) => {
      listener = fn;
      if (!started) {
        started = true;
        connect();
      }
    },
    status: async () => status,
    info: async () => info(),
    savePrefs: async (prefs) => {
      kept = { lang: "", ...prefs };
      try {
        localStorage.setItem(PREFS_KEY, JSON.stringify(prefs));
      } catch {
        // Stockage refusé (navigation privée) : les préférences valent pour cette page.
      }
      return info();
    },
    saveText: async (name, text) => {
      const url = URL.createObjectURL(new Blob([text], { type: "text/csv;charset=utf-8" }));
      const link = document.createElement("a");
      link.href = url;
      link.download = name;
      link.click();
      setTimeout(() => URL.revokeObjectURL(url), 10000);
      return name;
    },
    pickTorrents: async () =>
      (await pickFiles(".torrent,.meta4,.metalink,application/x-bittorrent", true)).map(remember),
    readTorrent: async (path) => {
      const file = pickedFile(path);
      return { name: file.name, data: await base64(file) };
    },
    pickList: async () => {
      const [file] = await pickFiles(".txt,.csv,text/plain,text/csv", false);
      return file ? remember(file) : null;
    },
    readList: async (path) => {
      const file = pickedFile(path);
      return { name: file.name, text: await file.text() };
    },
    session,
    login: async (token) => {
      const response = await fetch("/login", {
        method: "POST",
        credentials: "same-origin",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ token }),
      });
      if (!response.ok) throw sessionError(response.status);
      return response.json();
    },
    logout: async () => {
      await fetch("/logout", { method: "POST", credentials: "same-origin" }).catch(() => {});
      location.reload();
    },
    /// Jeton apporté par l'ancre de l'adresse (QR code du bureau), effacé aussitôt de la
    /// barre d'adresse et de l'historique ; null sans lui.
    tokenFromHash: () => {
      const token = takeHashToken(location.hash);
      if (token !== null) history.replaceState(null, "", location.pathname + location.search);
      return token;
    },
  };
}

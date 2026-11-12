// Ce que la page attend de son hôte. L'application de bureau (Tauri) le fournit ici ;
// l'interface web distante fournira le sien (WebSocket vers le moteur, SPEC §6.2), sans
// dialogue de dossier ni ouverture de fichier.
//
// - call(method, params) : appel du moteur ; rejette `{code, message}` ;
// - listen(fn) : reçoit dans l'ordre `{kind: "status", state, …}`,
//   `{kind: "message", message}` et `{kind: "resync"}` ;
// - status() : état courant de la connexion ;
// - info(), savePrefs(prefs), setAutostart(on), pickFolder(current), openFile(dir, name),
//   revealFile(dir, name), saveText(name, text), quit(stopEngine) : services de
//   l'application. saveText enregistre un texte dans un fichier que l'utilisateur choisit,
//   et rend son chemin, ou null s'il renonce ;
// - torrents (étape 5b), facultatifs : un hôte sans eux n'offre ni choix de fichier
//   .torrent ni association. onOpened(fn) prévient quand des liens magnet ou des .torrent
//   ont été ouverts avec l'application, que takeOpened() rend ({kind: "magnet", link} ou
//   {kind: "torrent", path}) ; readTorrent(path) rend {name, data} (base64) ;
//   pickTorrents() rend des chemins ; linksStatus() et setLinks(on) rendent
//   {available, enabled}.
// - listes de liens (étape 2b), facultatives : pickList() rend le chemin d'un fichier texte
//   ou CSV choisi, ou null ; readList(path) rend {name, text}. Sans elles, pas d'import.
// - watchesClipboard (étape 2c) : l'hôte surveille le presse-papiers ; un lien choisi dans sa
//   notification arrive par onOpened, {kind: "link", link} ou {kind: "magnet", link}.
//   notificationsBlocked(), facultative : vrai quand le système n'affichera pas ces
//   notifications (Windows : coupées en général ou pour l'application).
// - accès à distance (étape 3d), facultatifs : qrCode(text) rend {width, modules} (matrice,
//   « 1 » pour un module foncé, ligne par ligne) ; pickPem() rend le chemin d'un fichier PEM
//   choisi, ou null. Sans qrCode, pas de rubrique « Accès à distance ».
// - plugins (étape 4c), facultatifs : pickPlugin() rend le chemin d'un fichier .wasm choisi,
//   ou null ; readPlugin(path) rend {name, data} (base64). Sans eux, pas de rubrique
//   « Plugins » (l'ajout par un plugin reste possible).

export function tauriHost() {
  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;
  return {
    call: (method, params = {}) => invoke("rpc", { method, params }),
    listen: (fn) => listen("engine", (event) => fn(event.payload)),
    status: () => invoke("engine_status"),
    info: () => invoke("app_info"),
    savePrefs: (prefs) => invoke("save_prefs", { prefs }),
    setAutostart: (enabled) => invoke("set_autostart", { enabled }),
    pickFolder: (current) => invoke("pick_folder", { current }),
    openFile: (dir, name) => invoke("open_file", { dir, name }),
    revealFile: (dir, name) => invoke("reveal_file", { dir, name }),
    saveText: (name, text) => invoke("save_text", { name, text }),
    quit: (stopEngine) => invoke("quit", { stopEngine }),
    onOpened: (fn) => listen("opened", () => fn()),
    takeOpened: () => invoke("take_opened"),
    readTorrent: (path) => invoke("read_torrent", { path }),
    pickTorrents: () => invoke("pick_torrents"),
    pickList: () => invoke("pick_list"),
    readList: (path) => invoke("read_list", { path }),
    // Surveillance du presse-papiers par l'application (F-30), réglée par la préférence
    // `watchClipboard`.
    watchesClipboard: true,
    notificationsBlocked: () => invoke("notifications_blocked"),
    // Accès à distance (étape 3d) : QR code d'un appareil, certificat fourni.
    qrCode: (text) => invoke("qr_code", { text }),
    pickPem: () => invoke("pick_pem"),
    // Plugins d'hébergeurs (étape 4c) : fichier choisi, lu pour le moteur.
    pickPlugin: () => invoke("pick_plugin"),
    readPlugin: (path) => invoke("read_plugin", { path }),
    linksStatus: () => invoke("links_status"),
    setLinks: (enabled) => invoke("set_links", { enabled }),
  };
}

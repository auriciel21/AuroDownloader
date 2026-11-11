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
    linksStatus: () => invoke("links_status"),
    setLinks: (enabled) => invoke("set_links", { enabled }),
  };
}

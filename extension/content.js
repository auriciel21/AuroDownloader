// Clics dans une page (F-37, étape 5d) :
// - un clic avec Alt laisse le téléchargement qui suit au navigateur ; seul le fait
//   qu'Alt était enfoncé est transmis ;
// - un clic sur un lien magnet le confie à AuroDownloader (option, active par défaut) ;
//   si AuroDownloader est injoignable, le lien suit son chemin habituel (programme
//   associé du système) ; un refus du moteur est montré sur l'icône. Rien d'autre de la
//   page n'est lu ni transmis.
(() => {
  const send = (message) => {
    try {
      return chrome.runtime.sendMessage(message);
    } catch {
      // Extension rechargée ou retirée : la page garde un script orphelin, sans effet.
      return Promise.reject(new Error("extension absente"));
    }
  };

  // Seuls les gestes de l'utilisateur comptent : un clic fabriqué par la page
  // (`a.click()`, `dispatchEvent`) n'a pas `isTrusted` (audit W1, W10).
  const note = (event) => {
    if (event.isTrusted && event.altKey) send({ type: "altClick" }).catch(() => {});
  };
  for (const kind of ["mousedown", "click", "auxclick"]) {
    addEventListener(kind, note, { capture: true, passive: true });
  }

  // Réglage lu une fois puis suivi : la décision doit être prise pendant le clic.
  let captureMagnet = true;
  try {
    chrome.storage.sync.get("captureMagnet").then(
      (s) => { captureMagnet = s?.captureMagnet !== false; },
      () => {},
    );
    chrome.storage.onChanged.addListener((changes, area) => {
      if (area === "sync" && changes.captureMagnet) captureMagnet = changes.captureMagnet.newValue !== false;
    });
  } catch {
    // Pas d'accès aux réglages : valeur par défaut.
  }

  /// Lien rendu au navigateur après un refus : le clic suivant sur lui passe.
  let passing = null;

  /// Lien `<a>` HTML cliqué, même dans un shadow DOM.
  const anchorOf = (event) =>
    event.composedPath().find((n) => n instanceof HTMLAnchorElement && typeof n.href === "string");

  addEventListener(
    "click",
    (event) => {
      if (event.button !== 0 || event.defaultPrevented) return;
      const a = anchorOf(event);
      if (!a || !/^magnet:\?/i.test(a.href)) return;
      if (passing === a) {
        passing = null;
        return;
      }
      // Un lien magnet « cliqué » par un script de la page (même d'une iframe publicitaire)
      // suit son chemin habituel, où le navigateur demande confirmation : sinon le torrent
      // démarrerait, puis partagerait, sans aucun geste de l'utilisateur.
      if (!event.isTrusted) return;
      if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey || !captureMagnet) return;
      event.preventDefault();
      const giveBack = () => {
        passing = a;
        a.click();
      };
      // Rendu au navigateur seulement si AuroDownloader est injoignable (ou l'extension
      // retirée) ; un refus du moteur est montré sur l'icône.
      send({ type: "magnet", url: a.href }).then(
        (reply) => { if (reply?.giveBack) giveBack(); },
        giveBack,
      );
    },
    { capture: true },
  );
})();

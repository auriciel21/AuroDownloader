// Clic avec Alt dans une page : le téléchargement qui suit reste au navigateur (F-37).
// Rien d'autre n'est lu ni transmis : seulement le fait qu'Alt était enfoncé.
(() => {
  const note = (event) => {
    if (!event.altKey) return;
    try {
      chrome.runtime.sendMessage({ type: "altClick" }).catch(() => {});
    } catch {
      // Extension rechargée ou retirée : la page garde un script orphelin, sans effet.
    }
  };
  for (const kind of ["mousedown", "click", "auxclick"]) {
    addEventListener(kind, note, { capture: true, passive: true });
  }
})();

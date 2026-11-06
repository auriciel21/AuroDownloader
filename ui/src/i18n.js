// Textes de l'interface (N-10) : le français est la langue source, l'anglais sa
// traduction. Chaque texte affiché passe par `t`, qui remplace aussi {0}, {1}…

export const EN = {
  // Page
  "AuroDownloader": "AuroDownloader",
  "Adresses à télécharger, une par ligne": "Addresses to download, one per line",
  "Adresses à télécharger": "Addresses to download",
  "Ajouter": "Add",
  "Ajouter les téléchargements": "Add the downloads",
  "Dossier :": "Folder:",
  "Changer…": "Change…",
  "Changer le dossier de destination": "Change the destination folder",
  "Ajouter en pause": "Add paused",
  "Filtres": "Filters",
  "Toutes ({0})": "All ({0})",
  "En cours ({0})": "In progress ({0})",
  "Arrêtées ({0})": "Stopped ({0})",
  "Terminées ({0})": "Completed ({0})",
  "Annulées ({0})": "Cancelled ({0})",
  "Téléchargements": "Downloads",
  "Aucun téléchargement ici.": "No downloads here.",
  "Limite de débit globale": "Global speed limit",
  "Aucune limite": "No limit",
  "Préférences": "Preferences",
  "Fermer": "Close",
  "Fermer le message": "Dismiss the message",
  // Tâches
  "Pause": "Pause",
  "Reprendre": "Resume",
  "Relancer": "Retry",
  "Annuler": "Cancel",
  "Ouvrir": "Open",
  "Afficher dans le dossier": "Show in folder",
  "Mettre en pause {0}": "Pause {0}",
  "Reprendre {0}": "Resume {0}",
  "Relancer {0}": "Retry {0}",
  "Annuler {0}": "Cancel {0}",
  "Ouvrir {0}": "Open {0}",
  "Afficher {0} dans son dossier": "Show {0} in its folder",
  "Avancement de {0}": "Progress of {0}",
  "{0}, {1}": "{0}, {1}",
  "{0} sur {1}": "{0} of {1}",
  "{0}/s": "{0}/s",
  "{0} connexion(s)": "{0} connection(s)",
  "reste {0}": "{0} left",
  "{0} terminé": "{0} completed",
  "{0} en échec : {1}": "{0} failed: {1}",
  "(sans nom)": "(no name)",
  // États (SPEC §5.5)
  "En attente": "Queued",
  "En cours": "Downloading",
  "En pause": "Paused",
  "Nouvel essai bientôt": "Retrying soon",
  "Vérification": "Verifying",
  "Finition": "Finishing",
  "Partage": "Seeding",
  "Terminé": "Completed",
  "Échec": "Failed",
  "Annulé": "Cancelled",
  "{0} ({1})": "{0} ({1})",
  "{0} : {1}": "{0}: {1}",
  // Causes de pause
  "plage horaire": "schedule",
  "disque plein": "disk full",
  "réseau": "network",
  "fichier distant modifié": "remote file changed",
  // Codes d'erreur (SPEC §6.8)
  "réseau ou serveur injoignable": "network or server unreachable",
  "délai dépassé": "timed out",
  "refusé par le serveur": "refused by the server",
  "certificat refusé": "certificate rejected",
  "HTTPS indisponible": "HTTPS unavailable",
  "reprise impossible": "cannot resume",
  "écriture refusée dans le dossier": "writing to the folder was refused",
  "erreur du disque": "disk error",
  "empreinte différente": "checksum mismatch",
  "erreur interne du moteur": "internal engine error",
  "page vidéo non reconnue": "video page not recognised",
  // Moteur
  "Connexion au moteur…": "Connecting to the engine…",
  "Moteur injoignable : {0}. Nouvel essai automatique.": "Engine unreachable: {0}. Retrying automatically.",
  "Moteur injoignable.": "Engine unreachable.",
  "{0} téléchargement(s) en cours, {1}": "{0} download(s) in progress, {1}",
  "Aucun téléchargement en cours": "No download in progress",
  "Action refusée : {0}": "Action refused: {0}",
  "{0} adresse(s) refusée(s) : {1}": "{0} address(es) refused: {1}",
  // Préférences
  "Langue": "Language",
  "Langue du système": "System language",
  "Dossier des nouveaux téléchargements": "Folder for new downloads",
  "Fermer la fenêtre la replie dans la zone de notification": "Closing the window keeps the app in the notification area",
  "Aucune zone de notification sur ce bureau : fermer la fenêtre quitte l'application, les téléchargements continuent.":
    "No notification area on this desktop: closing the window quits the app, downloads continue.",
  "Démarrer avec la session, dans la zone de notification": "Start with the session, in the notification area",
  "Quitter et arrêter les téléchargements": "Quit and stop downloads",
  "AuroDownloader {0}, moteur {1}": "AuroDownloader {0}, engine {1}",
  // Unités
  "o": "B", "Kio": "KiB", "Mio": "MiB", "Gio": "GiB", "Tio": "TiB",
  "{0} s": "{0} s",
  "{0} min {1} s": "{0} min {1} s",
  "{0} h {1} min": "{0} h {1} min",
  "{0} j {1} h": "{0} d {1} h",
};

/// Fonction de traduction pour `lang` (`fr` ou `en`).
export function translator(lang) {
  return (text, ...args) => {
    const s = lang === "en" ? (EN[text] ?? text) : text;
    return s.replace(/\{(\d)\}/g, (_, i) => String(args[Number(i)]));
  };
}

// États d'une tâche (SPEC §5.5), causes de pause et codes d'erreur (SPEC §6.8).
export const STATES = {
  queued: "En attente",
  active: "En cours",
  paused: "En pause",
  retry_wait: "Nouvel essai bientôt",
  verifying: "Vérification",
  post_processing: "Finition",
  seeding: "Partage",
  completed: "Terminé",
  failed: "Échec",
  cancelled: "Annulé",
};

export const PAUSE_REASONS = {
  schedule: "plage horaire",
  disk_full: "disque plein",
  network: "réseau",
  file_changed: "fichier distant modifié",
};

export const ERRORS = {
  NET_UNREACHABLE: "réseau ou serveur injoignable",
  NET_TIMEOUT: "délai dépassé",
  HTTP_STATUS: "refusé par le serveur",
  TLS_INVALID_CERT: "certificat refusé",
  TLS_UNAVAILABLE: "HTTPS indisponible",
  RANGE_UNSUPPORTED: "reprise impossible",
  FILE_CHANGED: "fichier distant modifié",
  DISK_FULL: "disque plein",
  DEST_PERMISSION: "écriture refusée dans le dossier",
  DISK_IO: "erreur du disque",
  CHECKSUM_MISMATCH: "empreinte différente",
  INTERNAL: "erreur interne du moteur",
  EXTRACTOR_FAILED: "page vidéo non reconnue",
};

/// Cause d'un échec, lisible ; le code lui-même s'il est inconnu.
export function errorText(code, t) {
  return ERRORS[code] ? t(ERRORS[code]) : code;
}

/// État d'une tâche, avec la cause d'une pause qui ne vient pas de l'utilisateur ou
/// celle d'un échec.
export function stateText(task, t) {
  const state = STATES[task.state] ? t(STATES[task.state]) : task.state;
  const reason = task.pauseReason;
  if (task.state === "paused" && reason && reason !== "user") {
    return t("{0} ({1})", state, PAUSE_REASONS[reason] ? t(PAUSE_REASONS[reason]) : reason);
  }
  if ((task.state === "failed" || task.state === "retry_wait") && task.errorCode) {
    return t("{0} : {1}", state, errorText(task.errorCode, t));
  }
  return state;
}

const UNITS = ["o", "Kio", "Mio", "Gio", "Tio"];

/// Taille en unités binaires, une décimale au-delà de l'octet ; « ? » si inconnue.
export function bytes(n, lang, t) {
  if (n == null || n < 0 || !Number.isFinite(n)) return "?";
  let i = 0;
  while (n >= 1024 && i < UNITS.length - 1) {
    n /= 1024;
    i++;
  }
  const digits = i === 0 ? 0 : 1;
  const number = n.toLocaleString(lang, { minimumFractionDigits: digits, maximumFractionDigits: digits });
  return `${number} ${t(UNITS[i])}`;
}

/// Durée en secondes, arrondie à l'unité utile ; vide si inconnue.
export function duration(s, t) {
  if (s == null || s < 0 || !Number.isFinite(s)) return "";
  s = Math.round(s);
  if (s < 60) return t("{0} s", s);
  if (s < 3600) return t("{0} min {1} s", Math.floor(s / 60), s % 60);
  if (s < 86400) return t("{0} h {1} min", Math.floor(s / 3600), Math.floor((s % 3600) / 60));
  return t("{0} j {1} h", Math.floor(s / 86400), Math.floor((s % 86400) / 3600));
}

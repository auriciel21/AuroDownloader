// Textes de l'interface (N-10) : le français est la langue source, l'anglais sa
// traduction. Chaque texte affiché passe par `t`, qui remplace aussi {0}, {1}…

export const EN = {
  // Fenêtre principale
  "AuroDownloader": "AuroDownloader",
  "Actions": "Actions",
  "Ajouter": "Add",
  "Reprendre": "Resume",
  "Arrêter": "Stop",
  "Tout reprendre": "Resume all",
  "Tout arrêter": "Stop all",
  "Supprimer": "Delete",
  "Retirer les finis": "Clear finished",
  "Retirer de la liste les téléchargements terminés et annulés ; les fichiers restent sur le disque":
    "Remove completed and cancelled downloads from the list; files stay on disk",
  "Propriétés": "Properties",
  "Options": "Options",
  "Fermer": "Close",
  "Fermer le message": "Dismiss the message",
  "Catégories": "Categories",
  "Tous les téléchargements": "All downloads",
  "Incomplets": "Unfinished",
  "Terminés": "Finished",
  "Archives": "Archives",
  "Documents": "Documents",
  "Images": "Images",
  "Musique": "Music",
  "Programmes": "Programs",
  "Vidéos": "Videos",
  "Autres": "Other",
  "Files d'attente": "Queues",
  "File principale": "Main queue",
  "({0})": "({0})",
  "Replier {0}": "Collapse {0}",
  "Déplier {0}": "Expand {0}",
  "Téléchargements": "Downloads",
  "Téléchargements : {0}": "Downloads: {0}",
  "Nom du fichier": "File name",
  "Taille": "Size",
  "Avancement": "Progress",
  "Statut": "Status",
  "Temps restant": "Time left",
  "Débit": "Speed",
  "Ajouté le": "Added",
  "Trier par {0}": "Sort by {0}",
  "Aucun téléchargement ici.": "No downloads here.",
  "{0} sélectionnés": "{0} selected",
  "Limite de débit globale": "Global speed limit",
  "Aucune limite": "No limit",
  // Menu contextuel
  "Ouvrir": "Open",
  "Afficher dans le dossier": "Show in folder",
  "Limiter le débit…": "Limit speed…",
  "Supprimer…": "Delete…",
  // Ajout
  "Ajouter des téléchargements": "Add downloads",
  "Adresses, une par ligne": "Addresses, one per line",
  "Enregistrer dans": "Save to",
  "Parcourir…": "Browse…",
  "Changer le dossier de destination": "Change the destination folder",
  "Annuler": "Cancel",
  "Télécharger plus tard": "Download later",
  "Démarrer": "Start",
  // Suppression
  "Supprimer « {0} » de la liste ?": "Delete “{0}” from the list?",
  "Supprimer {0} téléchargements de la liste ?": "Delete {0} downloads from the list?",
  "Supprimer aussi les fichiers terminés du disque": "Also delete finished files from the disk",
  "Les fichiers partiels des téléchargements inachevés sont toujours supprimés.":
    "Partial files of unfinished downloads are always deleted.",
  // Propriétés
  "Fichier": "File",
  "Dossier": "Folder",
  "Téléchargé": "Downloaded",
  "Connexions": "Connections",
  "Fini le": "Finished",
  "Adresse": "Address",
  "Reprise possible": "Can resume",
  "Oui": "Yes",
  "Non : ce serveur ne permet pas de reprendre ; un arrêt ou une coupure fait repartir de zéro":
    "No: this server does not allow resuming; stopping or a dropped connection starts over from zero",
  "Segments et connexions": "Segments and connections",
  "N°": "No.",
  "État": "State",
  "Réception des données": "Receiving data",
  "{0} sur {1}": "{0} of {1}",
  "{0} segments, {1} en cours de réception": "{0} segments, {1} being received",
  "inconnue": "unknown",
  "Pause": "Pause",
  "Relancer": "Retry",
  "Annuler le téléchargement": "Cancel the download",
  "Débit maximal de ce téléchargement": "Maximum speed of this download",
  "La limite globale s'applique aussi : la plus basse l'emporte.":
    "The global limit also applies: the lower one wins.",
  "Appliquer": "Apply",
  "Limite de {0} appliquée": "Limit of {0} applied",
  "Limite retirée": "Limit removed",
  "Avancement de {0}": "Progress of {0}",
  "{0} — {1}": "{0} — {1}",
  "{0} %": "{0}%",
  "{0}/s": "{0}/s",
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
  // Planification (F-16)
  "Planification": "Schedule",
  "File d'attente": "Queue",
  "Ne télécharger que pendant ces plages horaires": "Download only during these time slots",
  "Plages horaires": "Time slots",
  "Ajouter une plage": "Add a time slot",
  "Une plage dont la fin précède le début passe minuit : de 23:00 à 07:00, c'est la nuit.":
    "A slot that ends before it starts runs past midnight: from 23:00 to 07:00 is the night.",
  "Quand la file n'a plus rien à télécharger": "When the queue has nothing left to download",
  "Ne rien faire": "Do nothing",
  "Mettre l'ordinateur en veille": "Put the computer to sleep",
  "Éteindre l'ordinateur": "Shut down the computer",
  "Annoncé une minute avant, avec un bouton pour l'annuler.":
    "Announced one minute before, with a button to cancel it.",
  "Enregistrer": "Save",
  "Jours": "Days",
  "Retirer": "Remove",
  "Retirer cette plage": "Remove this time slot",
  "de": "from",
  "à": "to",
  "En ce moment : {0}.": "Right now: {0}.",
  "Chaque plage a besoin d'au moins un jour.": "Each time slot needs at least one day.",
  "Chaque plage a besoin d'une heure de début et de fin.": "Each time slot needs a start and an end time.",
  "Planification enregistrée": "Schedule saved",
  "Planification désactivée": "Schedule turned off",
  "En attente de la plage horaire": "Waiting for its time slot",
  "ouverte jusqu'à {0}": "open until {0}",
  "ouverte": "open",
  "fermée jusqu'à {0}": "closed until {0}",
  "fermée": "closed",
  "({0} s)": "({0} s)",
  "Mise en veille de l'ordinateur dans une minute : la file « {0} » n'a plus rien à télécharger.":
    "The computer will sleep in one minute: the queue “{0}” has nothing left to download.",
  "Extinction de l'ordinateur dans une minute : la file « {0} » n'a plus rien à télécharger.":
    "The computer will shut down in one minute: the queue “{0}” has nothing left to download.",
  "Action de fin annulée": "End action cancelled",
  "Action de fin refusée par le système : {0}": "End action refused by the system: {0}",
  // Options
  "Rubriques": "Sections",
  "Général": "General",
  "Enregistrement": "Saving",
  "Connexion": "Connection",
  "À propos": "About",
  "Les fichiers en cours s'écrivent sous leur nom suivi de .part, dans ce même dossier, puis prennent leur nom à la fin.":
    "Files being downloaded are written under their name followed by .part, in this same folder, and take their name at the end.",
  "Le nombre de connexions de chaque téléchargement s'ajuste seul : 4 au départ, jusqu'à 16 tant que le débit progresse, moins si le serveur résiste.":
    "Each download adjusts its number of connections by itself: 4 at first, up to 16 while the speed improves, fewer if the server resists.",
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

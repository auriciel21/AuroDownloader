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
  // Retéléchargement
  "Retélécharger…": "Download again…",
  "Retélécharger": "Download again",
  "Retélécharger « {0} » depuis le début ?": "Download “{0}” again from the start?",
  "Retélécharger {0} téléchargements depuis le début ?": "Download {0} downloads again from the start?",
  "Remplacer le fichier déjà téléchargé": "Replace the file already downloaded",
  "Sinon, il reste, et le nouveau prend un autre nom.":
    "Otherwise it stays, and the new one gets another name.",
  "Ce qui a déjà été reçu des téléchargements inachevés est supprimé.":
    "What unfinished downloads have already received is deleted.",
  // Déplacer/Renommer
  "Déplacer/Renommer…": "Move/Rename…",
  "Déplacer/Renommer": "Move/Rename",
  "Changer le dossier": "Change the folder",
  "Déplacer": "Move",
  "Le fichier partiel est déplacé maintenant ; le téléchargement se terminera à cet endroit.":
    "The partial file is moved now; the download will finish there.",
  "Le téléchargement n'a pas encore commencé : il prendra ce nom et ce dossier.":
    "The download has not started yet: it will take this name and folder.",
  "Copie de « {0} » vers l'autre disque en cours": "Copying “{0}” to the other disk",
  "Déplacement de « {0} » impossible : {1}": "Could not move “{0}”: {1}",
  "Déplacement du fichier…": "Moving the file…",
  // Réservation de la place (F-10).
  "Réserver la place de chaque fichier dès le départ": "Reserve each file's space from the start",
  "Un disque trop petit se voit avant le téléchargement, et le fichier se fragmente moins. Sans effet sur les clés et disques en FAT ou exFAT.":
    "A disk that is too small shows before the download, and the file is less fragmented. No effect on FAT or exFAT drives.",
  "Place réservée dès le départ": "Space reserved from the start",
  "Place prise au fil du téléchargement": "Space taken as the download goes",
  // Historique (F-35).
  "Historique": "History",
  "Rechercher (nom, dossier ou site)": "Search (name, folder or site)",
  "Nom": "Name",
  "Chemin": "Path",
  "Taille (octets)": "Size (bytes)",
  "Afficher plus": "Show more",
  "Exporter en CSV…": "Export as CSV…",
  "Effacer l'historique…": "Clear history…",
  "Effacer l'historique": "Clear history",
  "Effacer": "Clear",
  "Toutes les entrées de l'historique seront effacées. Les téléchargements de la liste et les fichiers sur le disque restent.":
    "Every history entry will be cleared. The downloads in the list and the files on disk stay.",
  "Aucune entrée": "No entries",
  "{0} entrée(s)": "{0} entries",
  "{0} entrées affichées, d'autres suivent": "{0} entries shown, more follow",
  "historique-{0}.csv": "history-{0}.csv",
  "Historique exporté : {0}": "History exported: {0}",
  "{0} entrée(s) effacée(s)": "{0} entries cleared",
  // Empreinte attendue (F-11).
  "Empreinte": "Checksum",
  "Empreinte attendue": "Expected checksum",
  "Empreinte attendue (facultatif)": "Expected checksum (optional)",
  "SHA-256, SHA-1 ou MD5, pour une seule adresse : le fichier ne prend son nom que s'il y correspond.":
    "SHA-256, SHA-1 or MD5, for a single address: the file only gets its name if it matches.",
  "SHA-256, SHA-1 ou MD5 ; vide pour n'en vérifier aucune. Après un échec, Relancer vérifie de nouveau le fichier reçu, sans le retélécharger.":
    "SHA-256, SHA-1 or MD5; empty to check none. After a failure, Retry checks the received file again, without downloading it again.",
  "{0} {1}": "{0} {1}",
  "{0} {1} (vérifiée)": "{0} {1} (verified)",
  "{0} {1} (lue dans {2})": "{0} {1} (read from {2})",
  "{0} {1} (lue dans {2}, vérifiée)": "{0} {1} (read from {2}, verified)",
  "Chercher l'empreinte à côté du fichier (SHA256SUMS, .sha256…)": "Look for the checksum next to the file (SHA256SUMS, .sha256…)",
  "Quand vous n'en donnez pas, AuroDownloader cherche l'empreinte publiée à côté du fichier, puis la vérifie à la fin. Quelques petites requêtes de plus au même serveur.":
    "When you give none, AuroDownloader looks for the checksum published next to the file, then checks it at the end. A few more small requests to the same server.",
  "Empreintes cherchées à côté des fichiers": "Checksums looked for next to files",
  "Empreintes non cherchées à côté des fichiers": "Checksums no longer looked for next to files",
  "Empreinte illisible : 64 (SHA-256), 40 (SHA-1) ou 32 (MD5) chiffres hexadécimaux, ou « sha256:… »":
    "Unreadable checksum: 64 (SHA-256), 40 (SHA-1) or 32 (MD5) hexadecimal digits, or “sha256:…”",
  "Empreinte enregistrée": "Checksum saved",
  "Empreinte retirée": "Checksum removed",
  "Une empreinte ne vaut que pour une seule adresse.": "A checksum applies to a single address only.",
  // Actualiser l'adresse
  "Actualiser l'adresse…": "Refresh address…",
  "Actualiser l'adresse": "Refresh address",
  "Adresse actuelle": "Current address",
  "Nouvelle adresse": "New address",
  "Actualiser": "Refresh",
  "Nouvelle adresse de « {0} », par exemple un lien régénéré sur le site d'origine.":
    "New address for “{0}”, for example a link generated again on the original site.",
  "AuroDownloader vérifie que la nouvelle adresse donne un fichier de même taille, puis reprend là où le téléchargement s'était arrêté.":
    "AuroDownloader checks that the new address gives a file of the same size, then resumes where the download stopped.",
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
  "Vérification de l'empreinte": "Verifying the checksum",
  "Vérification de l'empreinte ({0} %)": "Verifying the checksum ({0}%)",
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
  "empreinte différente de celle attendue": "the file does not match its expected checksum",
  "erreur interne du moteur": "internal engine error",
  "page vidéo non reconnue": "video page not recognised",
  "coffre du système indisponible": "system keychain unavailable",
  "identifiants demandés par le serveur": "the server asks for a user name and password",
  "identifiants refusés par le serveur": "the server refused the user name or password",
  // Identifiants de sites
  "Identifiants de sites": "Site logins",
  "Envoyés seulement quand un serveur du site les demande (authentification Basic ou Digest). Les mots de passe sont rangés dans le coffre du système et ne sont jamais affichés.":
    "Sent only when a server of the site asks for them (Basic or Digest authentication). Passwords are kept in the system keychain and never shown.",
  "Aucun identifiant enregistré.": "No login saved.",
  "Nouveau…": "New…",
  "Modifier…": "Edit…",
  "Identifiants d'un site": "Site login",
  "Site": "Site",
  "Nom d'utilisateur": "User name",
  "Mot de passe": "Password",
  "exemple.org vaut aussi pour ses sous-domaines ; https://exemple.org/dossier ne vaut que pour ce dossier.":
    "example.org also covers its subdomains; https://example.org/folder covers that folder only.",
  "Laisser vide pour garder le mot de passe enregistré.": "Leave empty to keep the saved password.",
  "Identifiants de {0} supprimés": "Login for {0} deleted",
  // Moteur
  "Connexion au moteur…": "Connecting to the engine…",
  "Moteur injoignable : {0}. Nouvel essai automatique.": "Engine unreachable: {0}. Retrying automatically.",
  "Moteur injoignable.": "Engine unreachable.",
  "{0} téléchargement(s) en cours, {1}": "{0} download(s) in progress, {1}",
  "Aucun téléchargement en cours": "No download in progress",
  "Action refusée : {0}": "Action refused: {0}",
  "{0} adresse(s) refusée(s) : {1}": "{0} address(es) refused: {1}",
  "(aucun)": "(none)",
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
  "Afficher la fenêtre au premier plan quand un téléchargement est ajouté (navigateur, ligne de commande)":
    "Bring the window to the front when a download is added (browser, command line)",
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
  verifying: "Vérification de l'empreinte",
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
  CHECKSUM_MISMATCH: "empreinte différente de celle attendue",
  INTERNAL: "erreur interne du moteur",
  EXTRACTOR_FAILED: "page vidéo non reconnue",
  SECRET_UNAVAILABLE: "coffre du système indisponible",
  AUTH_REQUIRED: "identifiants demandés par le serveur",
  AUTH_FAILED: "identifiants refusés par le serveur",
};

/// Cause d'un échec, lisible ; le code lui-même s'il est inconnu.
export function errorText(code, t) {
  return ERRORS[code] ? t(ERRORS[code]) : code;
}

/// État d'une tâche, avec la cause d'une pause qui ne vient pas de l'utilisateur ou
/// celle d'un échec.
export function stateText(task, t) {
  if (task.moving) return t("Déplacement du fichier…");
  // Calcul de l'empreinte (F-11) : part du fichier déjà lue.
  if (task.state === "verifying" && task.verifiedBytes != null && task.totalBytes > 0) {
    return t("Vérification de l'empreinte ({0} %)", Math.floor((Math.min(task.verifiedBytes, task.totalBytes) * 100) / task.totalBytes));
  }
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

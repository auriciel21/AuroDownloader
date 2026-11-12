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
  "Adresses ou liens magnet, un par ligne": "Addresses or magnet links, one per line",
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
  // Miroirs (SPEC §7.7, phase 6).
  "Miroirs (facultatif), un par ligne": "Mirrors (optional), one per line",
  "D'autres adresses du même fichier, pour une seule adresse HTTP : chacune est vérifiée (même taille, mêmes octets) avant de servir, et le téléchargement se répartit entre elles selon leur débit.":
    "Other addresses of the same file, for a single HTTP address: each one is checked (same size, same bytes) before use, and the download is shared between them according to their speed.",
  "Des miroirs ne valent que pour une seule adresse.": "Mirrors only apply to a single address.",
  "Un torrent trouve ses pairs lui-même : pas de miroir à donner.": "A torrent finds its own peers: no mirror to give.",
  "Un miroir est une adresse http:// ou https:// : {0}": "A mirror is an http:// or https:// address: {0}",
  "Adresse donnée deux fois : {0}": "Address given twice: {0}",
  "{0} sources au plus, la principale comprise.": "At most {0} sources, the main one included.",
  "Sources": "Sources",
  "Source": "Source",
  "Débit moyen": "Average speed",
  "Principale": "Main",
  "Miroir {0}": "Mirror {0}",
  "À vérifier": "To be checked",
  "Bonne": "Good",
  "Lente": "Slow",
  "Écartée": "Set aside",
  "Ajouter un miroir": "Add a mirror",
  "Une autre adresse du même fichier. Elle est vérifiée avant de servir ; un téléchargement en cours repart aussitôt avec elle.":
    "Another address of the same file. It is checked before use; a running download restarts with it at once.",
  "Vérification du miroir…": "Checking the mirror…",
  "Miroir ajouté": "Mirror added",
  // Empreinte attendue (F-11).
  "Empreinte": "Checksum",
  "Empreinte attendue": "Expected checksum",
  "Empreinte attendue (facultatif)": "Expected checksum (optional)",
  "SHA-256, SHA-1 ou MD5, pour une seule adresse HTTP : le fichier ne prend son nom que s'il y correspond. Un torrent vérifie ses morceaux lui-même.":
    "SHA-256, SHA-1 or MD5, for a single HTTP address: the file only gets its name if it matches. A torrent checks its own pieces.",
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
  // Accès à distance (étape 3d)
  "Accès à distance": "Remote access",
  "Piloter les téléchargements depuis un autre appareil": "Control downloads from another device",
  "Un téléphone ou un autre ordinateur ouvre la page d'AuroDownloader dans son navigateur. Chaque appareil entre avec son propre jeton, que vous pouvez lui retirer.":
    "A phone or another computer opens the AuroDownloader page in its browser. Each device signs in with its own token, which you can take back.",
  "Adresse d'écoute": "Listening address",
  "Port": "Port",
  "Hors de cet ordinateur, les connexions passent en HTTPS. Sous Windows, le pare-feu demande votre accord à la première ouverture.":
    "Beyond this computer, connections use HTTPS. On Windows, the firewall asks for your permission the first time.",
  "Accès coupé.": "Access is off.",
  "Ouvert. Adresses de la page : {0}": "Open. Page addresses: {0}",
  "Activé, mais fermé : {0}": "Turned on, but closed: {0}",
  "Ouverture…": "Opening…",
  "Cet ordinateur seulement ({0})": "This computer only ({0})",
  "Toutes les interfaces ({0})": "All interfaces ({0})",
  "{0} (absente)": "{0} (missing)",
  "Certificat": "Certificate",
  "Empreinte SHA-256": "SHA-256 fingerprint",
  "Valable jusqu'au": "Valid until",
  "fabriqué par AuroDownloader": "made by AuroDownloader",
  "fourni": "provided",
  "Le navigateur de l'appareil prévient que ce certificat n'est pas reconnu : avant de continuer, vérifiez que l'empreinte qu'il montre est bien celle-ci.":
    "The device's browser warns that this certificate is not recognised: before going on, check that the fingerprint it shows is this one.",
  "Utiliser mon certificat…": "Use my certificate…",
  "Revenir au certificat d'AuroDownloader": "Go back to AuroDownloader's certificate",
  "Certificat d'AuroDownloader rétabli": "AuroDownloader's certificate restored",
  "Appareils": "Devices",
  "Aucun appareil.": "No device.",
  "Ajouter un appareil…": "Add a device…",
  "Ajouter un appareil": "Add a device",
  "Tout piloter": "Full control",
  "utilisé le {0}": "used on {0}",
  "jamais utilisé": "never used",
  "Appareil « {0} » retiré : ses sessions sont fermées": "Device “{0}” removed: its sessions are closed",
  "Appareil « {0} » ajouté": "Device “{0}” added",
  "Accès à distance ouvert": "Remote access on",
  "Accès à distance coupé": "Remote access off",
  "Nom de l'appareil": "Device name",
  "Droits": "Permissions",
  "Tout piloter : ajouter, arrêter, supprimer": "Full control: add, stop, delete",
  "Ajouter seulement": "Add only",
  "Regarder seulement": "View only",
  "Les options, les identifiants de sites et cette rubrique restent réservés à cet ordinateur.":
    "Options, site logins and this section stay reserved to this computer.",
  "Créer le jeton": "Create the token",
  "Adresse de la page": "Page address",
  "QR code de l'adresse de la page, jeton compris": "QR code of the page address, token included",
  "Visez ce code avec l'appareil photo du téléphone : la page s'ouvre et entre d'elle-même.":
    "Point the phone's camera at this code: the page opens and signs in by itself.",
  "L'écoute est réservée à cet ordinateur : choisissez une adresse du réseau local pour qu'un téléphone puisse joindre la page.":
    "Listening is limited to this computer: choose a local network address so that a phone can reach the page.",
  "L'accès n'est pas encore ouvert : ce code servira dès que la case « Piloter les téléchargements depuis un autre appareil » sera cochée.":
    "Access is not open yet: this code will work as soon as “Control downloads from another device” is ticked.",
  "Jeton, à coller dans la page si besoin": "Token, to paste into the page if needed",
  "Copier": "Copy",
  "Jeton copié": "Token copied",
  "Ce code et ce jeton ne seront plus montrés : quiconque les photographie peut entrer avec les droits de cet appareil. Pour en refaire, retirez l'appareil puis ajoutez-le de nouveau.":
    "This code and this token will not be shown again: anyone who photographs them can sign in with this device's permissions. To make new ones, remove the device and add it again.",
  "Certificat fourni": "Provided certificate",
  "Un certificat et sa clé au format PEM, par exemple ceux d'un nom de domaine de votre réseau. AuroDownloader les relit lui-même ; il ne les copie pas.":
    "A certificate and its key in PEM format, for instance those of a domain name on your network. AuroDownloader reads them itself; it does not copy them.",
  "Certificat (chaîne comprise)": "Certificate (chain included)",
  "Choisir le certificat": "Choose the certificate",
  "Clé privée": "Private key",
  "Choisir la clé privée": "Choose the private key",
  "Utiliser": "Use",
  // Moteur
  "Connexion au moteur…": "Connecting to the engine…",
  "Moteur injoignable : {0}. Nouvel essai automatique.": "Engine unreachable: {0}. Retrying automatically.",
  "Moteur injoignable.": "Engine unreachable.",
  "{0} téléchargement(s) en cours, {1}": "{0} download(s) in progress, {1}",
  "Aucun téléchargement en cours": "No download in progress",
  "Action refusée : {0}": "Action refused: {0}",
  "{0} ajout(s) refusé(s) : {1}": "{0} addition(s) refused: {1}",
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
  // Presse-papiers (F-30, étape 2c).
  "Proposer d'ajouter les liens copiés": "Offer to add copied links",
  "L'adresse d'un fichier ou un lien magnet copié fait apparaître une notification ; un clic ouvre « Ajouter ». Le presse-papiers n'est lu que pour cela, et rien n'en est gardé.":
    "A copied file address or magnet link shows a notification; a click opens “Add”. The clipboard is read only for this, and nothing from it is kept.",
  "Windows n'affiche pas les notifications d'AuroDownloader (Paramètres, Système, Notifications) : cette option n'aura aucun effet tant qu'elles restent désactivées.":
    "Windows does not show AuroDownloader’s notifications (Settings, System, Notifications): this option has no effect while they stay off.",
  "Aucune zone de notification sur ce bureau : fermer la fenêtre quitte l'application, les téléchargements continuent.":
    "No notification area on this desktop: closing the window quits the app, downloads continue.",
  "Démarrer avec la session, dans la zone de notification": "Start with the session, in the notification area",
  "Quitter et arrêter les téléchargements": "Quit and stop downloads",
  "AuroDownloader {0}, moteur {1}": "AuroDownloader {0}, engine {1}",
  // Torrents (étape 5b)
  "En attente de l'interface réseau": "Waiting for the network interface",
  "Partage (ratio {0})": "Seeding (ratio {0})",
  "↑ {0}": "↑ {0}",
  "Envoi : {0}": "Upload: {0}",
  "Pairs": "Peers",
  "{0} (ratio {1})": "{0} (ratio {1})",
  "Arrêter le partage": "Stop seeding",
  "Télécharger {0}": "Download {0}",
  "Choisissez au moins un fichier.": "Choose at least one file.",
  "{0} fichier(s) choisi(s), {1} à télécharger": "{0} file(s) chosen, {1} to download",
  "Retirer {0}": "Remove {0}",
  "Un lien ou un fichier vous attend dans « Ajouter ».":
    "A link or file is waiting for you in “Add”.",
  "Un torrent vérifie ses morceaux lui-même : pas d'empreinte à donner.":
    "A torrent checks its own pieces: no checksum to give.",
  "Sans limite, jusqu'à ce que je l'arrête": "No limit, until I stop it",
  "Ne pas partager": "Do not seed",
  "Jusqu'à avoir envoyé autant que reçu (ratio 1)": "Until as much is sent as received (ratio 1)",
  "Jusqu'au ratio {0}": "Up to ratio {0}",
  "Aucune : toutes les interfaces": "None: all interfaces",
  "{0} est là : les torrents ne passent que par elle.": "{0} is up: torrents only go through it.",
  "{0} est absente : les torrents attendent son retour.": "{0} is missing: torrents wait for it to come back.",
  "AuroDownloader ouvre les liens magnet et les fichiers .torrent":
    "AuroDownloader opens magnet links and .torrent files",
  "Liens magnet et fichiers .torrent rendus au programme d'avant":
    "Magnet links and .torrent files given back to the previous program",
  "Partage des torrents réglé": "Torrent seeding set",
  "UPnP activé": "UPnP on",
  "UPnP désactivé": "UPnP off",
  "Interface des torrents réglée": "Torrent interface set",
  "Fichiers .torrent ou Metalink": ".torrent or Metalink files",
  "Choisir des fichiers .torrent ou Metalink…": "Choose .torrent or Metalink files…",
  "Ou glissez-les sur la fenêtre. Un Metalink (.meta4, .metalink) donne pour chaque fichier ses adresses et son empreinte ; son adresse peut aussi se coller ci-dessus.":
    "Or drop them on the window. A Metalink (.meta4, .metalink) gives the addresses and checksum of each file; its address can also be pasted above.",
  // Analyse de liens (F-29, F-31, SPEC §7.18).
  "Liens": "Links",
  "Relever les fichiers d'une page web, d'un texte ou d'une liste, et en télécharger plusieurs à la fois":
    "Find the files of a web page, a text or a list, and download several at once",
  "Analyser des liens": "Analyse links",
  "Adresse d'une page web, ou texte contenant des adresses": "Address of a web page, or text containing addresses",
  "Importer une liste (texte, CSV)…": "Import a list (text, CSV)…",
  "La page est lue telle quelle, sans JavaScript ; ses pages liées ne sont pas parcourues.":
    "The page is read as is, without JavaScript; its linked pages are not followed.",
  "Filtres": "Filters",
  "Extensions": "Extensions",
  "Taille minimale": "Minimum size",
  "Taille maximale": "Maximum size",
  "Expression régulière sur l'adresse": "Regular expression on the address",
  "Montrer aussi les pages et les liens sans extension": "Also show pages and links without an extension",
  "Tout cocher": "Check all",
  "Tout décocher": "Uncheck all",
  "Analyse en cours…": "Analysing…",
  "{0} lien(s) trouvé(s) dans {1}": "{0} link(s) found in {1}",
  "le texte collé": "the pasted text",
  "Collez l'adresse d'une page web, ou un texte qui contient des adresses.":
    "Paste the address of a web page, or a text that contains addresses.",
  "Taille minimale illisible (exemples : 500K, 10M, 1G).": "Unreadable minimum size (examples: 500K, 10M, 1G).",
  "Taille maximale illisible (exemples : 500K, 10M, 1G).": "Unreadable maximum size (examples: 500K, 10M, 1G).",
  "Expression régulière illisible : {0}": "Unreadable regular expression: {0}",
  "{0} coché(s) sur {1} montré(s), {2} trouvé(s)": "{0} checked of {1} shown, {2} found",
  "Seuls les {0} premiers liens sont affichés ; tous ceux qui sont cochés seront ajoutés.":
    "Only the first {0} links are displayed; all checked ones will be added.",
  "{0} téléchargement(s) ajouté(s)": "{0} download(s) added",
  "{0} ajouté(s), {1} refusé(s) : {2}": "{0} added, {1} refused: {2}",
  // Metalink (F-27, SPEC §7.7).
  "Écarté : {0} ({1})": "Set aside: {0} ({1})",
  "Un Metalink donne lui-même l'empreinte de chaque fichier.": "A Metalink gives the checksum of each file itself.",
  "Un Metalink donne lui-même les miroirs de chaque fichier.": "A Metalink gives the mirrors of each file itself.",
  "Choisissez les fichiers du Metalink, puis « Démarrer » ou « Télécharger plus tard ».":
    "Choose the files of the Metalink, then “Start” or “Download later”.",
  "aucun fichier choisi dans {0}": "no file chosen in {0}",
  "Envoyé aux autres pairs": "Sent to other peers",
  "Fichiers du torrent": "Torrent files",
  "La liste des fichiers arrive avec les métadonnées du torrent, une fois qu'il a démarré.":
    "The file list arrives with the torrent's metadata, once it has started.",
  "Télécharger": "Download",
  "Reçu": "Received",
  "Appliquer le choix": "Apply the choice",
  "Pour un torrent, une nouvelle limite vaut à sa prochaine reprise.":
    "For a torrent, a new limit applies when it next resumes.",
  "BitTorrent": "BitTorrent",
  "Après le téléchargement, partager": "After downloading, seed",
  "Le ratio compare ce qui est envoyé aux autres pairs à la taille reçue. Sur un réseau facturé au volume, rien n'est partagé.":
    "The ratio compares what is sent to other peers with the size received. On a metered network, nothing is seeded.",
  "Limite d'envoi": "Upload limit",
  "Interface réseau des torrents (VPN)": "Network interface for torrents (VPN)",
  "Actualiser la liste": "Refresh the list",
  "Ouvrir le port d'écoute sur le routeur (UPnP)": "Open the listening port on the router (UPnP)",
  "Plus de pairs joignables. Vaut à la prochaine ouverture des torrents.":
    "More reachable peers. Applies the next time torrents start.",
  "Ouvrir les liens magnet et les fichiers .torrent avec AuroDownloader":
    "Open magnet links and .torrent files with AuroDownloader",
  "Décochée, le programme d'avant est rétabli. Sous Windows, un choix fait dans les paramètres du système passe avant : Windows peut demander quel programme utiliser.":
    "Unchecked, the previous program is restored. On Windows, a choice made in the system settings comes first: Windows may ask which program to use.",
  // Vidéo (étape 5c)
  "≈ {0}": "≈ {0}",
  "absent": "missing",
  "{0}, celui du système": "{0}, from the system",
  "{0}, téléchargé": "{0}, downloaded",
  "Installation en cours…": "Installing…",
  "Dernière installation en échec : {0}": "Last installation failed: {0}",
  "Outils prêts.": "Tools ready.",
  "Outils absents : les vidéos ne peuvent pas être téléchargées.": "Tools missing: videos cannot be downloaded.",
  "Installer yt-dlp et ffmpeg": "Install yt-dlp and ffmpeg",
  "Donnez l'adresse d'une page web (http ou https).": "Give the address of a web page (http or https).",
  "Analyse de la page par yt-dlp…": "yt-dlp is reading the page…",
  "yt-dlp et ffmpeg sont nécessaires pour lire cette page.": "yt-dlp and ffmpeg are needed to read this page.",
  "Page non reconnue : {0}": "Page not recognised: {0}",
  "Cette vidéo est protégée (DRM) : AuroDownloader ne la télécharge pas.":
    "This video is protected (DRM): AuroDownloader does not download it.",
  "Meilleure qualité": "Best quality",
  "Son seul": "Audio only",
  "{0}p": "{0}p",
  "Format {0}": "Format {0}",
  "image et son": "picture and sound",
  "image": "picture",
  "son": "sound",
  "{0}, protégé (DRM)": "{0}, protected (DRM)",
  "Format choisi : {0}": "Chosen format: {0}",
  "Vidéo « {0} » ajoutée": "Video “{0}” added",
  "yt-dlp se mettra à jour de lui-même": "yt-dlp will update itself",
  "yt-dlp ne se mettra plus à jour de lui-même": "yt-dlp will no longer update itself",
  "Télécharger la vidéo d'une page web (sites sans DRM), par yt-dlp et ffmpeg":
    "Download the video of a web page (sites without DRM), with yt-dlp and ffmpeg",
  "Vidéo": "Video",
  "Ajouter une vidéo": "Add a video",
  "Adresse de la page de la vidéo": "Address of the video page",
  "Analyser": "Analyse",
  "Sites sans DRM : PeerTube, Wikimedia Commons, Internet Archive… La page est lue par yt-dlp.":
    "Sites without DRM: PeerTube, Wikimedia Commons, Internet Archive… The page is read by yt-dlp.",
  "Les vidéos demandent yt-dlp et ffmpeg, absents. AuroDownloader peut les télécharger depuis leurs publications officielles (yt-dlp sur GitHub, ffmpeg des builds BtbN), en vérifiant leur signature et leurs empreintes : près de 400 Mo une fois installés.":
    "Videos need yt-dlp and ffmpeg, which are missing. AuroDownloader can download them from their official releases (yt-dlp on GitHub, ffmpeg from the BtbN builds), checking their signature and checksums: nearly 400 MB once installed.",
  "Titre": "Title",
  "Durée": "Duration",
  "Qualité": "Quality",
  "Tous les formats": "All formats",
  "Choisir": "Choose",
  "Format": "Format",
  "Contenu": "Content",
  "Image": "Picture",
  "Protocole": "Protocol",
  "Conteneur": "Container",
  "Automatique": "Automatic",
  "Sous-titres, gardés à côté de la vidéo": "Subtitles, kept next to the video",
  "Aucun sous-titre proposé.": "No subtitles offered.",
  "Réinstaller": "Reinstall",
  "Téléchargés depuis leurs publications officielles (yt-dlp sur GitHub, ffmpeg des builds BtbN), signature et empreintes vérifiées, rangés à côté des données du moteur. Jamais sans votre accord.":
    "Downloaded from their official releases (yt-dlp on GitHub, ffmpeg from the BtbN builds), signature and checksums checked, stored next to the engine's data. Never without your consent.",
  "Mettre à jour yt-dlp de lui-même": "Let yt-dlp update itself",
  "Au plus une fois par jour, quand une page est analysée ; la signature de la nouvelle version est vérifiée. Les sites changent souvent : un yt-dlp ancien finit par ne plus les lire.":
    "At most once a day, when a page is analysed; the new version's signature is checked. Sites change often: an old yt-dlp ends up unable to read them.",
  "yt-dlp ou ffmpeg absent (Options, Vidéo)": "yt-dlp or ffmpeg missing (Options, Video)",
  "assemblage de la vidéo impossible": "the video could not be assembled",
  // Interface distante (étape 3c)
  "Se déconnecter": "Sign out",
  "Sous-dossier (facultatif)": "Subfolder (optional)",
  "Créé dans le dossier ci-dessus.": "Created in the folder above.",
  "Sous-dossier refusé : « . » et « .. » ne sont pas permis.":
    "Subfolder refused: “.” and “..” are not allowed.",
  "Connexion à AuroDownloader": "Sign in to AuroDownloader",
  "Collez un jeton créé sur la machine du moteur (Options du bureau, ou auro token create). Il n'est envoyé qu'une fois, puis ce navigateur garde une session de 30 jours.":
    "Paste a token created on the engine's machine (desktop Options, or auro token create). It is sent only once; this browser then keeps a 30-day session.",
  "Jeton": "Token",
  "Se connecter": "Sign in",
  "Jeton inconnu du moteur, ou révoqué.": "Token unknown to the engine, or revoked.",
  "Pas de jeton dans le texte collé ({0} caractères) : un jeton commence par auro_, suivi de 43 lettres, chiffres, - ou _.":
    "No token in the pasted text ({0} characters): a token starts with auro_, followed by 43 letters, digits, - or _.",
  "Trop d'essais refusés : réessayez dans une minute.": "Too many refused attempts: try again in a minute.",
  "Ce jeton permet seulement d'ajouter des téléchargements.": "This token only allows adding downloads.",
  "Arrêter {0}": "Stop {0}",
  "Reprendre {0}": "Resume {0}",
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
  TOOLS_MISSING: "yt-dlp ou ffmpeg absent (Options, Vidéo)",
  MERGE_FAILED: "assemblage de la vidéo impossible",
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

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
  "« {0} » termine sa fusion ou sa commande de fin : il pourra être supprimé ensuite.":
    "“{0}” is finishing its merge or end command: it can be deleted afterwards.",
  "{0} téléchargements terminent leur fusion ou leur commande de fin : ils pourront être supprimés ensuite.":
    "{0} downloads are finishing their merge or end command: they can be deleted afterwards.",
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
  // F-12 : empreintes de morceaux d'un Metalink.
  "Morceaux": "Pieces",
  "{0} morceaux de {1} ({2})": "{0} pieces of {1} ({2})",
  "{0}, 1 retéléchargé": "{0}, 1 downloaded again",
  "{0}, {1} retéléchargés": "{0}, {1} downloaded again",
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
  "commande de fin en échec": "end command failed",
  "session BitTorrent indisponible (port, réseau)": "BitTorrent session unavailable (port, network)",
  "torrent refusé ou invalide": "torrent refused or invalid",
  "adresse du réseau interne refusée": "address on the internal network refused",
  // Règles de rangement (SPEC 7.21)
  "Règles": "Rules",
  "Une règle range les fichiers ajoutés sans dossier choisi (extension, « Ajouter ») : dossier et nom donnés dès le départ. Elle peut aussi lancer une commande quand le téléchargement est terminé. La première règle activée qui correspond l'emporte.":
    "A rule files downloads added without a chosen folder (extension, “Add”): folder and name are set from the start. It can also run a command when the download is finished. The first enabled rule that matches wins.",
  "Aucune règle : chaque fichier va dans le dossier des téléchargements, sous son propre nom.":
    "No rules: every file goes to the downloads folder, under its own name.",
  "Nouvelle…": "New…",
  "Monter": "Move up",
  "Descendre": "Move down",
  "Les règles ne se modifient que dans l'application, sur l'ordinateur.":
    "Rules can only be changed in the application, on the computer.",
  "Règle de rangement": "Filing rule",
  "Activée": "Enabled",
  "Si (tous les critères remplis ; vides : toute tâche)": "If (all criteria met; none: every download)",
  "Types de fichier": "File types",
  "ou extensions": "or extensions",
  "Un type coché ou une extension listée suffit : iso, pdf, séparées par des virgules.":
    "One ticked type or one listed extension is enough: iso, pdf, separated by commas.",
  "Domaines du site": "Site domains",
  "debian.org vaut aussi pour cdimage.debian.org. Les torrents n'ont pas de domaine.":
    "debian.org also covers cdimage.debian.org. Torrents have no domain.",
  "Adresse (expression régulière)": "Address (regular expression)",
  "Nom du fichier (expression régulière)": "File name (regular expression)",
  "(?i) au début pour ignorer les majuscules.": "(?i) at the start to ignore case.",
  "Alors": "Then",
  "Choisir le dossier de la règle": "Choose the rule's folder",
  "Un chemin complet, ou un sous-dossier du dossier des téléchargements (Vidéos/Séries). Vide : le dossier des téléchargements.":
    "A full path, or a subfolder of the downloads folder (Videos/Series). Empty: the downloads folder.",
  "Vide : le nom d'origine. Sinon un modèle : {nom} (sans l'extension), {ext}, {hôte}, {date} ; par exemple {date} {nom}.{ext}. Pas pour un torrent.":
    "Empty: the original name. Otherwise a pattern: {name} (without the extension), {ext}, {host}, {date}; for example {date} {name}.{ext}. Not for a torrent.",
  "Commande à la fin du téléchargement": "Command when the download ends",
  "Programme (chemin complet)": "Program (full path)",
  "Arguments, un par ligne": "Arguments, one per line",
  "Lancé sans shell, une fois le fichier à son nom. Variables : {chemin}, {dossier}, {nom}, {taille}, {sha256}, {id}. Un échec ou un délai dépassé met le téléchargement en échec, fichier gardé ; le reprendre relance la commande.":
    "Run without a shell, once the file has its name. Variables: {path}, {dir}, {name}, {size}, {sha256}, {id}. A failure or a timeout fails the download, keeping the file; resuming it runs the command again.",
  "Délai (secondes)": "Time limit (seconds)",
  "type {0}": "type {0}",
  "site {0}": "site {0}",
  "adresse ~ {0}": "address ~ {0}",
  "nom ~ {0}": "name ~ {0}",
  "dossier {0}": "folder {0}",
  "nom {0}": "name {0}",
  "commande {0}": "command {0}",
  "une commande de fin": "an end command",
  "toute tâche": "every download",
  "rien": "nothing",
  "Si {0}, alors {1}.": "If {0}, then {1}.",
  "{0}. {1} (désactivée)": "{0}. {1} (disabled)",
  "Règle enregistrée": "Rule saved",
  "Règle déplacée au rang {0}": "Rule moved to position {0}",
  "Règle activée": "Rule enabled",
  "Règle désactivée": "Rule disabled",
  "Règle {0} supprimée": "Rule {0} deleted",
  // Identifiants de sites
  "Identifiants de sites": "Site logins",
  "Envoyés seulement quand un serveur du site les demande (HTTP Basic ou Digest, FTP, SFTP). Les mots de passe sont rangés dans le coffre du système et ne sont jamais affichés.":
    "Sent only when a server of the site asks for them (HTTP Basic or Digest, FTP, SFTP). Passwords are kept in the system keychain and never shown.",
  "Aucun identifiant enregistré.": "No login saved.",
  "Nouveau…": "New…",
  "Modifier…": "Edit…",
  "Identifiants d'un site": "Site login",
  "Site": "Site",
  "Nom d'utilisateur": "User name",
  "Mot de passe": "Password",
  "exemple.org vaut aussi pour ses sous-domaines, en HTTP, FTP et SFTP ; https://exemple.org/dossier ne vaut que pour ce dossier, sftp://exemple.org que pour SFTP.":
    "example.org also covers its subdomains, over HTTP, FTP and SFTP; https://example.org/folder covers that folder only, sftp://example.org SFTP only.",
  "Clé privée SSH (SFTP, facultative)": "SSH private key (SFTP, optional)",
  "Choisir le fichier de clé privée": "Choose the private key file",
  "Avec une clé, le mot de passe est sa phrase de passe (vide si elle n'en a pas). Le fichier reste à sa place : AuroDownloader le relit à chaque connexion.":
    "With a key, the password is its passphrase (empty if it has none). The file stays where it is: AuroDownloader reads it at each connection.",
  "FTP : autoriser sans chiffrement": "FTP: allow without encryption",
  "Seulement pour un serveur FTP sans TLS : le mot de passe y passe alors en clair, lisible par qui écoute le réseau.":
    "Only for an FTP server without TLS: the password then travels in clear text, readable by anyone listening on the network.",
  "{0} — {1} (clé privée)": "{0} — {1} (private key)",
  "Clés des serveurs SSH": "SSH server keys",
  "Acceptées pour les téléchargements SFTP. Un serveur qui présente une autre clé est bloqué jusqu'à votre décision.":
    "Accepted for SFTP downloads. A server that presents another key is blocked until you decide.",
  "Aucune clé acceptée.": "No accepted key.",
  "Oublier la clé": "Forget the key",
  "Clé de {0} oubliée": "Key of {0} forgotten",
  "Clé du serveur SSH": "SSH server key",
  "Accepter la clé…": "Accept the key…",
  "Accepter la clé de ce serveur ?": "Accept this server's key?",
  "Accepter la clé": "Accept the key",
  "La clé de ce serveur a changé depuis que vous l'avez acceptée. Quelqu'un pourrait se faire passer pour lui : ne l'acceptez que si son administrateur confirme ce changement.":
    "This server's key has changed since you accepted it. Someone could be impersonating it: accept it only if its administrator confirms the change.",
  "La clé n'est plus en attente (le moteur a redémarré) : relancer la tâche pour la revoir.":
    "The key is no longer pending (the engine restarted): retry the download to see it again.",
  "La clé de ce serveur a changé depuis que vous l'avez acceptée. Quelqu'un pourrait se faire passer pour lui.":
    "This server's key has changed since you accepted it. Someone could be impersonating it.",
  "Ce serveur n'est pas encore connu. Vérifiez son empreinte auprès de son administrateur avant de l'accepter.":
    "This server is not known yet. Check its fingerprint with its administrator before accepting it.",
  "Serveur {0}. Les téléchargements SFTP de ce serveur ne se feront qu'avec cette clé ; ceux qu'elle bloquait repartiront.":
    "Server {0}. SFTP downloads from this server will only proceed with this key; those it blocked will start again.",
  "Clé acceptée ; 1 téléchargement relancé": "Key accepted; 1 download restarted",
  "Clé acceptée ; {0} téléchargements relancés": "Key accepted; {0} downloads restarted",
  "clé du serveur SSH inconnue (voir les Propriétés)": "unknown SSH server key (see Properties)",
  "clé du serveur SSH changée (voir les Propriétés)": "SSH server key changed (see Properties)",
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
  // Proxy (F-07, phase 7)
  "Proxy": "Proxy",
  "Celui du système": "The system's",
  "Aucun (toujours en direct)": "None (always direct)",
  "Celui-ci :": "This one:",
  "En direct pour (séparés par des virgules)": "Direct for (comma-separated)",
  "Identifiant (facultatif)": "User name (optional)",
  "Adresses http://, https://, socks5:// ou socks5h:// (le proxy résout alors les noms). Les torrents n'y passent jamais ; les connexions des appareils distants et des plugins ne suivent qu'un proxy SOCKS5. Le mot de passe est rangé dans le coffre du système.":
    "http://, https://, socks5:// or socks5h:// addresses (the proxy then resolves names). Torrents never go through it; connections of remote devices and plugins only follow a SOCKS5 proxy. The password is kept in the system vault.",
  "Proxy appliqué": "Proxy applied",
  // Journaux et diagnostic (N-12, phase 7)
  "Détail des journaux": "Log detail",
  "Erreurs seulement": "Errors only",
  "Erreurs et avertissements": "Errors and warnings",
  "Ordinaire (conseillé)": "Normal (recommended)",
  "Détaillé, pour un dépannage": "Detailed, for troubleshooting",
  "Journaux du moteur, gardés à côté de ses données (25 Mo au plus) : jamais d'adresse complète, de mot de passe ni de cookie.":
    "Engine logs, kept next to its data (25 MB at most): never a full address, a password or a cookie.",
  "Exporter un diagnostic…": "Export a diagnostic…",
  "Diagnostic": "Diagnostic",
  "Voici tout ce que contiendra l'archive : un résumé de l'état du moteur, et ses journaux nettoyés (ni adresse, ni hôte, ni chemin, ni nom d'utilisateur, ni jeton). Rien n'est envoyé : vous l'enregistrez, puis la joignez vous-même à un signalement.":
    "This is everything the archive will hold: a summary of the engine's state, and its cleaned logs (no address, host, path, user name or token). Nothing is sent: you save it, then attach it yourself to a report.",
  "Résumé": "Summary",
  "Journaux": "Logs",
  "Enregistrer…": "Save…",
  "{0} ligne(s) de journal ; les {1} dernières :": "{0} log line(s); the last {1}:",
  "Aucune ligne de journal.": "No log line.",
  "Diagnostic enregistré : {0}": "Diagnostic saved: {0}",
  "Détail des journaux changé": "Log detail changed",
  // Plugins d'hébergeurs (étape 4c)
  "Plugins": "Plugins",
  "Un plugin lit la page d'un hébergeur de fichiers et en donne les liens directs, qu'AuroDownloader télécharge. Il tourne isolé : ni fichiers ni système, réseau seulement vers les domaines qu'il déclare, et jamais vos cookies ni vos mots de passe.":
    "A plugin reads a file host's page and gives its direct links, which AuroDownloader downloads. It runs isolated: no files, no system, network only to the domains it declares, and never your cookies or passwords.",
  "Aucun plugin installé.": "No plugin installed.",
  "Pages prises en charge": "Supported pages",
  "Domaines contactés": "Domains contacted",
  "Auteur": "Author",
  "Description": "Description",
  "Installé le": "Installed on",
  "Installer un plugin…": "Install a plugin…",
  "Installer un plugin": "Install a plugin",
  "Installer": "Install",
  "Activer": "Enable",
  "Désactiver": "Disable",
  "Identifiant": "Identifier",
  "Version": "Version",
  "{0} {1} (désactivé)": "{0} {1} (disabled)",
  "Le plugin pourra lire les pages de ces domaines, sans vos cookies ni vos mots de passe, et choisir les liens que vous téléchargerez. N'installez que des plugins d'une source à laquelle vous faites confiance, et vérifiez l'empreinte si elle est publiée.":
    "The plugin will be able to read pages from these domains, without your cookies or passwords, and to choose the links you download. Only install plugins from a source you trust, and check the fingerprint if one is published.",
  "Ce plugin est déjà installé, identique.": "This plugin is already installed, unchanged.",
  "Remplace la version {0} installée ; ses permissions sont celles montrées ici.":
    "Replaces installed version {0}; its permissions are the ones shown here.",
  "Plugin {0} installé": "Plugin {0} installed",
  "Plugin {0} désactivé": "Plugin {0} disabled",
  "Plugin {0} activé": "Plugin {0} enabled",
  "Plugin {0} retiré": "Plugin {0} removed",
  "Ajouter ces pages telles quelles, sans plugin": "Add these pages as they are, without a plugin",
  "Page lue par le plugin {0}, qui en donnera les liens.": "Page read by the {0} plugin, which will give its links.",
  "Pages lues par les plugins {0}, qui en donneront les liens.": "Pages read by the {0} plugins, which will give their links.",
  "{0} (plugin {1})": "{0} ({1} plugin)",
  "retiré": "removed",
  "Le plugin n'a trouvé aucun lien sur cette page.": "The plugin found no link on this page.",
  "Messages du plugin ({0})": "Plugin messages ({0})",
  "Une page d'hébergeur donne ses liens par son plugin : pas d'empreinte à donner.":
    "A file host's page gives its links through its plugin: no checksum to give.",
  "Une page d'hébergeur donne ses liens par son plugin : pas de miroir à donner.":
    "A file host's page gives its links through its plugin: no mirror to give.",
  "Choisissez les liens des pages d'hébergeurs, puis « Démarrer » ou « Télécharger plus tard ».":
    "Choose the links of the file host pages, then “Start” or “Download later”.",
  "aucun lien choisi dans {0}": "no link chosen in {0}",
  "Page d'origine": "Source page",
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
  "Le moteur ne répond pas : rien n'a été ajouté. Réessayer quand il sera de retour.":
    "The engine is not answering: nothing was added. Try again once it is back.",
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
  "Afficher la fenêtre au premier plan quand un téléchargement en file démarre":
    "Bring the window to the front when a queued download starts",
  // Guide de l'utilisateur (étape 9d).
  "Guide de l'utilisateur": "User guide",
  "Guide de l'utilisateur (F1)": "User guide (F1)",
  // Exploration d'un site (F-42, SPEC 7.18 bis, 2026-11-26).
  "La page est lue telle quelle, sans JavaScript ; ses pages liées ne sont parcourues qu'avec « Explorer le site ».": "The page is read as is, without JavaScript; its linked pages are only followed with \"Explore the site\".",
  "Exploration du site": "Site exploration",
  "Explorer le site : lire aussi les pages liées du même site": "Explore the site: also read the linked pages of the same site",
  "Les pages du même site sont lues de lien en lien, 2 à la fois ; les pages que robots.txt refuse aux robots ne sont pas lues. Les fichiers trouvés, même sur d'autres sites, sont montrés ; rien n'est téléchargé avant votre choix.": "The pages of the same site are read link after link, 2 at a time; the pages that robots.txt refuses to robots are not read. The files found, even on other sites, are shown; nothing is downloaded before you choose.",
  "Modèle": "Template",
  "Personnalisé": "Custom",
  "Toutes les images du site": "All the images of the site",
  "Audio de la page et d'un niveau en dessous": "Audio of the page and one level below",
  "Toutes les vidéos du site": "All the videos of the site",
  "Tous les fichiers sauf pages et images": "All files except pages and images",
  "Documents de la page et d'un niveau en dessous": "Documents of the page and one level below",
  "Profondeur (1 à 5)": "Depth (1 to 5)",
  "Pages lues au plus": "Pages read at most",
  "Pages à ne pas ouvrir (expression régulière)": "Pages not to open (regular expression)",
  "Rester sous le dossier de départ": "Stay under the starting folder",
  "Ignorer robots.txt": "Ignore robots.txt",
  "Arrêter l'exploration": "Stop the exploration",
  "Niveau": "Level",
  "Trouvé sur": "Found on",
  "Garder l'arborescence du site": "Keep the tree of the site",
  // Copie d'un site (F-43, SPEC 7.18 ter, étape B3).
  "Copier le site pour la navigation hors ligne": "Copy the site for offline browsing",
  "Une seule tâche copie les pages explorées, avec leurs images, styles, scripts et polices, et les fichiers cochés ; les liens des pages mènent à la copie. Les pages suivent les réglages de l'exploration : les pages cochées sont ignorées.": "A single task copies the explored pages, with their images, styles, scripts and fonts, and the checked files; the links of the pages lead to the copy. Pages follow the exploration settings: checked pages are ignored.",
  "Copie du site": "Site copy",
  "Page de départ": "Start page",
  "Page copiée": "Copied page",
  "Réglages": "Settings",
  "Copiés": "Copied",
  "Exploration": "Exploration",
  "Éléments en échec": "Failed items",
  "Raison": "Reason",
  "Site : {0}": "Site: {0}",
  "{0} page(s), {1} ressource(s), {2} fichier(s)": "{0} page(s), {1} resource(s), {2} file(s)",
  "{0}, {1} en échec": "{0}, {1} failed",
  "{0}, {1} laissé(s) par les limites": "{0}, {1} left out by the limits",
  "profondeur {0}": "depth {0}",
  "{0} pages au plus": "{0} pages at most",
  "{0} au plus": "{0} at most",
  "sous le dossier de départ": "under the starting folder",
  "robots.txt ignoré": "robots.txt ignored",
  "finie": "finished",
  "en cours": "in progress",
  "arrêtée : limite de pages atteinte": "stopped: page limit reached",
  "arrêtée : volume atteint": "stopped: size limit reached",
  "arrêtée : 20 000 éléments trouvés": "stopped: 20,000 items found",
  "Copie du site ajoutée": "Site copy added",
  "{0} fichiers cochés : une copie en prend 10 000 au plus.": "{0} files checked: a copy takes 10,000 at most.",
  "pas une page web": "not a web page",
  "redirigé vers un autre site": "redirected to another site",
  "trop gros pour une copie": "too big for a copy",
  "adresse refusée": "address refused",
  "la page de départ n'est pas une page HTML": "the start page is not an HTML page",
  "Chaque fichier va dans un sous-dossier au nom du site, puis de ses dossiers (8 niveaux au plus).": "Each file goes into a subfolder named after the site, then its folders (8 levels at most).",
  "Exploration du site en cours…": "Exploring the site…",
  "{0} page(s) lue(s), {1} en attente, {2} lien(s) trouvé(s)": "{0} page(s) read, {1} waiting, {2} link(s) found",
  "{0} lien(s) trouvé(s) en explorant {1} ({2} page(s) lue(s))": "{0} link(s) found while exploring {1} ({2} page(s) read)",
  "Exploration arrêtée : {0} lien(s) trouvé(s) ({1} page(s) lue(s))": "Exploration stopped: {0} link(s) found ({1} page(s) read)",
  "Limite de pages atteinte : les pages suivantes n'ont pas été lues.": "Page limit reached: the following pages were not read.",
  "256 Mio de pages lus : l'exploration s'est arrêtée là.": "256 MiB of pages read: the exploration stopped there.",
  "10 000 liens trouvés : l'exploration s'est arrêtée là.": "10,000 links found: the exploration stopped there.",
  "Exploration impossible : {0}": "Exploration impossible: {0}",
  "« Explorer le site » demande l'adresse d'une page web.": "\"Explore the site\" needs the address of a web page.",
  "Profondeur refusée : de 1 à 5.": "Depth refused: from 1 to 5.",
  "Nombre de pages refusé : de 1 à 2000.": "Number of pages refused: from 1 to 2000.",
  "Expression des pages à ne pas ouvrir illisible : {0}": "Expression of the pages not to open unreadable: {0}",
  "Connexion au moteur perdue : l'exploration s'est arrêtée.": "Connection to the engine lost: the exploration stopped.",
  "lue": "read",
  "en attente": "waiting",
  "non lue : au-delà de la profondeur": "not read: beyond the depth",
  "non lue : autre site": "not read: another site",
  "non lue : hors du dossier de départ": "not read: outside the starting folder",
  "non lue : robots.txt": "not read: robots.txt",
  "non lue : exclue": "not read: excluded",
  "non lue : échec": "not read: failed",
  "non lue : exploration arrêtée": "not read: exploration stopped",
  // Recherche et impression du guide (2026-11-26).
  "Rechercher dans le guide": "Search the guide",
  "Rechercher dans le guide…": "Search the guide…",
  "Résultat précédent": "Previous result",
  "Résultat suivant": "Next result",
  "Résultat précédent (Maj+Entrée)": "Previous result (Shift+Enter)",
  "Résultat suivant (Entrée)": "Next result (Enter)",
  "Aucun résultat": "No results",
  "Imprimer…": "Print…",
  // Bouton « Aide » de la barre d'outils (2026-11-26).
  "Aide": "Help",
  "Raccourcis clavier": "Keyboard shortcuts",
  "Contenu du guide": "Guide content",
  "Guide introuvable : {0}": "Guide not found: {0}",
  // Mises à jour de l'application (étape 8b).
  "Mises à jour": "Updates",
  "Rechercher les mises à jour": "Check for updates",
  "Au plus une fois par jour, auprès de GitHub, qui voit l'adresse IP et la version utilisée, rien d'autre. Rien ne s'installe sans votre accord.":
    "At most once a day, from GitHub, which sees the IP address and the version in use, nothing else. Nothing is installed without your consent.",
  "Rechercher maintenant": "Check now",
  "Nouvelle version": "New version",
  "Nouveautés": "What's new",
  "L'application se ferme le temps de l'installation, puis redémarre ; les téléchargements en cours reprennent ensuite.":
    "The application closes during the installation, then restarts; downloads in progress resume afterwards.",
  "Cette installation (paquet .deb) se met à jour par le paquet de la nouvelle version, à télécharger sur sa page.":
    "This installation (.deb package) is updated with the new version's package, downloaded from its page.",
  "Plus tard": "Later",
  "Ouvrir la page de la version": "Open the release page",
  "Installer et redémarrer": "Install and restart",
  "AuroDownloader recherche ses mises à jour au plus une fois par jour, auprès de GitHub, qui voit l'adresse IP et la version utilisée, rien d'autre. Une version nouvelle vous est proposée ; rien ne s'installe sans votre accord. Vous pouvez couper cette recherche ici ou dans Options, Général.":
    "AuroDownloader checks for updates at most once a day, from GitHub, which sees the IP address and the version in use, nothing else. A new version is offered to you; nothing is installed without your consent. You can turn this off here or in Options, General.",
  "Ne pas rechercher": "Don't check",
  "D'accord": "OK",
  "Mises à jour non configurées dans cette version.": "Updates are not configured in this build.",
  "Version {0} disponible.": "Version {0} available.",
  "Dernière recherche : {0}.": "Last check: {0}.",
  "Aucune recherche faite pour l'instant.": "No check made yet.",
  "Recherche d'une version nouvelle…": "Checking for a new version…",
  "Recherche impossible : {0}": "Check failed: {0}",
  "AuroDownloader est à jour (version {0}).": "AuroDownloader is up to date (version {0}).",
  "AuroDownloader {0} est disponible ; vous avez la version {1}.": "AuroDownloader {0} is available; you have version {1}.",
  "Téléchargement de la version {0}…": "Downloading version {0}…",
  "Mise à jour impossible : {0}": "Update failed: {0}",
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
  "Une adresse FTP ou SFTP n'a qu'une source : pas de miroir.": "An FTP or SFTP address has a single source: no mirror.",
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
  "Collez un jeton créé sur la machine du moteur (Options du bureau, ou auro token create). Il n'est envoyé qu'une fois, puis ce navigateur garde une session : 30 jours en HTTPS, 12 heures sinon.":
    "Paste a token created on the engine's machine (desktop Options, or auro token create). It is sent only once; this browser then keeps a session: 30 days over HTTPS, 12 hours otherwise.",
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
  POSTPROCESS_FAILED: "commande de fin en échec",
  SECRET_UNAVAILABLE: "coffre du système indisponible",
  AUTH_REQUIRED: "identifiants demandés par le serveur",
  AUTH_FAILED: "identifiants refusés par le serveur",
  HOST_KEY_UNKNOWN: "clé du serveur SSH inconnue (voir les Propriétés)",
  HOST_KEY_CHANGED: "clé du serveur SSH changée (voir les Propriétés)",
  // Audit W6 : codes de la SPEC 6.8 qui manquaient.
  TORRENT_SESSION: "session BitTorrent indisponible (port, réseau)",
  TORRENT_FAILED: "torrent refusé ou invalide",
  ADDRESS_FORBIDDEN: "adresse du réseau interne refusée",
  // Copie d'un site (SPEC 7.18 ter) : la tâche, et ses éléments en échec.
  SITE_NOT_HTML: "la page de départ n'est pas une page HTML",
  NOT_HTML: "pas une page web",
  REDIRECTED: "redirigé vers un autre site",
  TOO_BIG: "trop gros pour une copie",
  URL_INVALID: "adresse refusée",
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

/// Taille en unités binaires, une décimale au-delà de l'octet ; « ? » si inconnue. Recopiée
/// dans l'extension (`formatSize` d'`extension/video.js`), comparée par ses tests (audit W22).
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

/// Nom d'une langue (`fr`, `pt-BR`) dans la langue de l'interface, suivi de son code ; le code
/// seul s'il est inconnu. Recopiée dans l'extension (`extension/video.js`), comparée par ses
/// tests (audit W22).
export function languageName(code, lang, t) {
  try {
    const name = new Intl.DisplayNames([lang], { type: "language" }).of(code);
    return name && name !== code ? t("{0} ({1})", name, code) : code;
  } catch {
    return code;
  }
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

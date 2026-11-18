# Torrents

## Ajouter un torrent

- Collez un **lien magnet** (`magnet:?…`) dans « Ajouter » ;
- ou choisissez un **fichier `.torrent`** (« Choisir des fichiers .torrent ou Metalink… »), ou
  glissez-le sur la fenêtre ;
- ou ouvrez-le avec AuroDownloader depuis le navigateur ou l'explorateur de fichiers : il
  préremplit la boîte « Ajouter », que vous confirmez. L'association des liens magnet et des
  `.torrent` à AuroDownloader est une option, désactivée par défaut (Options, BitTorrent).

Un torrent n'a pas d'empreinte à donner : il vérifie ses morceaux lui-même.

## Choisir les fichiers

Les Propriétés d'un torrent montrent ses fichiers : décochez ceux que vous ne voulez pas, puis
validez. Les autres ne sont pas téléchargés.

## Partage

Un torrent terminé reste en **Partage** : il envoie aux autres pairs ce qu'il a reçu, jusqu'à
la limite choisie (Options, BitTorrent, « Après le téléchargement, partager ») :

- Ne pas partager ;
- Jusqu'à avoir envoyé autant que reçu (ratio 1), par défaut ;
- jusqu'à un autre ratio ;
- Sans limite, jusqu'à ce que je l'arrête.

Le partage ne prend pas de place dans la file : d'autres téléchargements démarrent pendant ce
temps. Sur un réseau facturé au volume (Android), rien n'est partagé.

## Réglages (Options, BitTorrent)

- **Limite d'envoi** : le débit d'envoi aux autres pairs.
- **Interface réseau des torrents (VPN)** : les torrents ne passent que par cette interface ;
  si elle disparaît (VPN coupé), ils s'arrêtent et attendent qu'elle revienne. Les autres
  téléchargements ne sont pas concernés.
- **Ouvrir le port d'écoute sur le routeur (UPnP)** : plus de pairs joignables, désactivé par
  défaut.

Les torrents ne passent jamais par le proxy.

## À la reprise

Après un redémarrage, AuroDownloader revérifie les morceaux déjà reçus avant de reprendre :
c'est plus long qu'une reprise instantanée, mais rien de faux n'est gardé.

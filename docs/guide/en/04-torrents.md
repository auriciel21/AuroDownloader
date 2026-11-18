# Torrents

## Adding a torrent

- Paste a **magnet link** (`magnet:?…`) into "Add";
- or choose a **`.torrent` file** ("Choose .torrent or Metalink files…"), or drop it on the
  window;
- or open it with AuroDownloader from the browser or the file manager: it fills in the "Add"
  box, which you confirm. Associating magnet links and `.torrent` files with AuroDownloader is
  an option, off by default (Options, BitTorrent).

A torrent needs no checksum: it checks its pieces itself.

## Choosing the files

A torrent's Properties show its files: untick the ones you do not want, then apply. The others
are not downloaded.

## Seeding

A finished torrent stays **Seeding**: it sends what it received to other peers, up to the
chosen limit (Options, BitTorrent, "After downloading, seed"):

- Do not seed;
- Until as much is sent as received (ratio 1), the default;
- up to another ratio;
- No limit, until I stop it.

Seeding does not take a place in the queue: other downloads start meanwhile. On a metered
network (Android), nothing is seeded.

## Settings (Options, BitTorrent)

- **Upload limit**: the upload speed to other peers.
- **Network interface for torrents (VPN)**: torrents only go through that interface; if it
  disappears (VPN off), they stop and wait for it to come back. Other downloads are not
  affected.
- **Open the listening port on the router (UPnP)**: more reachable peers, off by default.

Torrents never go through the proxy.

## When resuming

After a restart, AuroDownloader checks the pieces already received again before resuming: it
takes longer than an instant resume, but nothing faulty is kept.

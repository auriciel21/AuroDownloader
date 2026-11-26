# Options

"Options" opens the settings, in sections. The left and right arrow keys move from one section
to the next.

## General

- **Language**: "System language", Français or English; the window changes at once.
- **Start with the session, in the notification area**.
- **Closing the window keeps the app in the notification area**: otherwise, closing the window
  quits the application (downloads go on, through the engine).
- **Bring the window to the front when a download is added** (by the browser or the command
  line).
- **Bring the window to the front when a queued download starts**: when a place frees up in
  its queue, or when its time slot opens. Not for a new attempt after a temporary error, nor
  for the downloads that resume when the session opens.
- **Offer to add copied links**: see [Link grabber](06-links.md#copied-links).
- **Updates**: "Check for updates" (at most once a day, from GitHub, which sees the IP address
  and the version in use, nothing else) and "Check now". A new version shows up in "New
  version", with "What's new": "Install and restart" (Windows, AppImage), "Open the release
  page" (`.deb` package), or "Later". The update is checked by its signature; nothing is
  installed without your consent. During the installation, the application closes, then
  restarts, and downloads resume.

## Saving

- **Folder for new downloads**, when none is chosen (extension, copied links, remote
  interface).
- **Reserve each file's space from the start**: a full disk shows at once, and the file is less
  fragmented.
- **Look for the checksum next to the file (SHA256SUMS, .sha256…)**: when you give none,
  AuroDownloader looks for the checksum published next to the file, and checks it.

## Rules

The [filing rules](07-organising.md#filing-rules): New…, Edit…, Move up, Move down, Disable,
Delete.

## Connection

- **Global speed limit**: the same as in the status bar.
- **Proxy**: "The system's" (default), "None (always direct)", or "This one:" with:
  - **Address**: `http://host:port`, `https://host:port` or `socks5://host:port`;
  - **Direct for (comma-separated)**: the hosts reached without the proxy
    (`localhost, nas.home`);
  - **User name (optional)** and **Password** (stored in the system vault);
  - **Apply**: saves the proxy.

  Torrents never go through the proxy; the update check uses the system's.

## BitTorrent

- **After downloading, seed**: see [Seeding](04-torrents.md#seeding).
- **Upload limit**: the upload speed to other peers.
- **Network interface for torrents (VPN)**: "Refresh the list" reads the computer's interfaces
  again.
- **Open the listening port on the router (UPnP)**.
- **Open magnet links and .torrent files with AuroDownloader**: the association, off by
  default.

The details: [Torrents](04-torrents.md#settings-options-bittorrent).

## Video

- The version of yt-dlp and ffmpeg; "Install yt-dlp and ffmpeg" while they are missing,
  then "Reinstall" to download them again (for example if they are damaged).
- **Let yt-dlp update itself**: at most once a day, signature checked.

The details: [Videos](05-video.md).

## Site logins

- A site's **logins** (New…, Edit…, Delete), in the "Site login" box: **Site**, **User
  name**, **Password**, **SSH private key (SFTP, optional)** ("Browse…"), and "FTP: allow
  without encryption". Passwords go to the system vault and never come out.
- Accepted **SSH server keys**, with their fingerprint; "Forget the key".

## Plugins

A host plugin turns the page of a file host into direct links.

- "Install a plugin…": choose a `.wasm` file. Before installing, AuroDownloader shows its name,
  its author, the pages it handles, the domains it will contact and its fingerprint: only
  install plugins you trust.
- A plugin runs isolated: it sees neither your files, nor your passwords, nor your cookies, and
  only contacts its declared domains.
- Disable, Remove. A new version that asks for more domains asks for your consent again.

## Remote access

- **Control downloads from another device**, **Listening address** ("Refresh the list") and
  **Port**.
- The certificate in use and its fingerprint; "Use my certificate…" ("Provided certificate"
  box: "Certificate (chain included)" and "Private key", then "Use"); "Go back to
  AuroDownloader's certificate".
- **Devices**: each with its permissions and last use; "Add a device…", "Remove".

The details: [Remote access](11-remote-access.md).

## About

- AuroDownloader's version.
- **Log detail**: Errors only, Errors and warnings, Normal (recommended), Detailed, for
  troubleshooting.
- **Export a diagnostic…**: see [Troubleshooting](14-troubleshooting.md#diagnostic).
- **Quit and stop downloads**.

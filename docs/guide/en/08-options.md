# Options

"Options" opens the settings, in sections. The left and right arrow keys move from one section
to the next.

## General

- **Language**: the system's, English or French.
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
  version", with what is new: "Install and restart" (Windows, AppImage), "Open the release
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

- **Global speed limit**.
- **Proxy**: the system's (default), none (always direct), or "This one": address (`http://`,
  `https://` or `socks5://`), addresses to reach directly, login and password (stored in the
  system vault). Torrents never go through the proxy; the update check uses the system's.

## BitTorrent

Seeding, upload limit, network interface (VPN), UPnP, association of magnet links and
`.torrent` files: see [Torrents](04-torrents.md#settings-options-bittorrent).

## Video

Installing yt-dlp and ffmpeg, and updating them: see [Videos](05-video.md).

## Site logins

- A site's **logins** (New…, Edit…, Delete): site, user name, password or SSH private key
  (SFTP), "FTP: allow without encryption". Passwords go to the system vault and never come out.
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

See [Remote access](11-remote-access.md).

## About

- AuroDownloader's version.
- **Log detail**: Errors only, Errors and warnings, Normal (recommended), Detailed, for
  troubleshooting.
- **Export a diagnostic…**: see [Troubleshooting](14-troubleshooting.md#diagnostic).
- **Quit and stop downloads**.

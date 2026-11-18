# Android

The Android application holds the same engine as the desktop version: HTTP(S), FTP(S), SFTP
and torrent downloads, with the same safe resume. Site videos (yt-dlp) are not available
there.

## Adding

- **Add**, then the address: `https://…`, `ftp://…`, `sftp://…` or a magnet link.
- **Share** a link from another application (browser, messaging), or **open** a magnet link
  or a `.torrent` file with AuroDownloader: it is added directly.

Files go to `Download/AuroDownloader`, where other applications see them.

## The list and the details

Tap a download for its details: status, size, speed, time left, checksum; Pause, Resume, Retry,
Cancel the download, Remove from the list, Open; for a torrent, the choice of its files; for an
unknown SFTP server, its key fingerprint and "Accept the key…".

## In the background

A download goes on when the application is closed: a notification shows the progress, with
"Pause all". Android may ask to allow notifications on the first download.

## Settings

- **Download folder** (shown: `Download/AuroDownloader`) and **Global speed limit**.
- **Look for the checksum next to the file**, **Reserve each file's space from the start**.
- **Wi-Fi only**: no downloading on a metered connection (mobile data, hotspot).
- **Resume after the device restarts**: unfinished downloads resume, or a notification offers
  to resume them (Android 14 and later).
- **BitTorrent**: seeding after downloading, upload limit. Nothing is seeded on a metered
  connection.

To go through a VPN, use Android's: it applies to the whole application.

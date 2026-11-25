# Troubleshooting

## Common problems

| What you see | What to do |
| --- | --- |
| "Retrying soon" that lasts | The network or the server does not answer: AuroDownloader retries by itself. Check the connection and the proxy (Options, Connection). |
| "refused by the server" | The link may have expired: "Refresh address…" with a fresh link, taken from the source page. |
| The server asks you to sign in | Site logins (Options, Site logins), or use the extension, which passes the cookies on. |
| "certificate rejected" | The server's HTTPS certificate is not valid: AuroDownloader does not download without it. |
| "the file does not match its expected checksum" | The file received is not the one announced: check the checksum, or download again. |
| "disk full" | Make room: the download starts again by itself. |
| "remote file changed" | The file changed on the server during the download: "Download again…". |
| The extension takes nothing | Icon window, Settings, "Check"; disable IDM's extension; on Linux, prefer the `.deb` package. |
| A video is refused | It is protected (DRM), or yt-dlp does not know the site yet: wait for it to update itself. |
| Torrents do not start | If a network interface (VPN) is chosen (Options, BitTorrent), it must be there. |

## Diagnostic

Options, About, **Export a diagnostic…**: a summary of the engine's state and its recent logs,
**cleaned** (no addresses, file names, headers or logins), shown before being saved in a `.zip`
archive, to attach to a report. For a problem that happens again, first set "Log detail" to
"Detailed, for troubleshooting", reproduce it, then export.

## Where the data is

- The download list, the history and the engine's settings are in a database (`auro.db`), with
  the logs (`logs`): `%LOCALAPPDATA%\AuroDownloader` on Windows, `~/.local/share/aurodownloader`
  on Linux.
- Passwords, cookies and tokens are in the system vault, never in the database.

## Privacy

AuroDownloader sends no telemetry. It only contacts:

- the servers of the files you download (and their mirrors);
- torrent peers;
- GitHub, at most once a day, for updates (can be turned off);
- the official releases of yt-dlp and ffmpeg, if you install them;
- the domains declared by the plugins you install.

## Reporting a problem

On the [issues](https://github.com/auriciel21/AuroDownloader/issues) page, with the version
(Options, About), the system, what happens, and the diagnostic if possible. A security flaw is
reported privately, through the
[security advisories](https://github.com/auriciel21/AuroDownloader/security/advisories/new).

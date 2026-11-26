# AuroDownloader

**A free and open source download manager for Windows, Linux and Android.**

[Version française](README.fr.md)

One reliable download engine, running as its own process and driven by a JSON-RPC API,
with a desktop application, an Android application, a web interface, a command line and
a browser extension on top. None of them contains download logic: they all talk to the
same engine.

> **Status: version 1 in preparation.** Nothing has been released yet. The features below
> are written and tested; packages will be published on the
> [Releases](https://github.com/auriciel21/AuroDownloader/releases) page.

## Features

- **Fast, safe downloads**: HTTP and HTTPS split into segments over several connections,
  resumed after a crash or a power cut without losing or corrupting a byte; FTP, FTPS and
  SFTP (with server key confirmation).
- **Integrity**: expected checksum (SHA-256, SHA-1, MD5), checksum files found next to the
  download, faulty pieces of a Metalink downloaded again.
- **Mirrors and Metalink**: the same file from several servers, spread by speed, with
  automatic failover.
- **BitTorrent**: magnet links and `.torrent` files, file selection, seeding up to a ratio,
  optional binding to a network interface (VPN).
- **Video** (Windows and Linux): pages of video sites through yt-dlp and ffmpeg, installed
  on request and checked; quality, format and subtitles.
- **Organisation**: queues and priorities, speed limits (global, per queue, per download),
  time windows with sleep or shutdown at the end, rules that sort downloads into folders
  and rename them, a command run when a download finishes.
- **Link grabber**: all the file links of a web page or a list, filtered by type and size.
- **Browser extension** for Firefox, Chrome and Edge: downloads, magnet links and the video
  of the page, sent to AuroDownloader.
- **Remote web interface**: the same window from a phone or another computer, with access
  tokens and TLS.
- **Host plugins**: WebAssembly plugins, sandboxed and limited to their declared sites, turn
  a file hosting page into direct links.
- **Proxy**: system or manual, HTTP, HTTPS or SOCKS5.
- **Accessibility**: keyboard and screen reader support, tested with NVDA, Orca and
  TalkBack (WCAG 2.2 AA review in [`docs/ACCESSIBILITE.md`](docs/ACCESSIBILITE.md), in
  French).
- **Privacy**: no telemetry. The desktop application looks for updates at most once a day
  on GitHub (which sees the IP address and the version), installs nothing without your
  consent, and this check can be turned off.
- **Languages**: English and French.

## Installing

Once version 1 is out, from the [Releases](https://github.com/auriciel21/AuroDownloader/releases)
page:

| System | Package |
| --- | --- |
| Windows 10 and 11 (x64, ARM64) | `AuroDownloader_<version>_x64-setup.exe` or `_arm64-setup.exe`, installed for the current user |
| Linux (x86_64, aarch64) | `.deb` package or `.AppImage` (WebKitGTK 4.1: Ubuntu 22.04, Debian 12 or later) |
| Android 8 and later (arm64) | `AuroDownloader_<version>_arm64.apk` |

The browser extension is not yet in the browser stores; it can be loaded from the
`extension` folder.

`SHA256SUMS` lists the checksum of every file of a release. The Windows installer is not
signed: SmartScreen warns about an "unknown publisher"; choose "More info", then "Run
anyway", if you did get it from the Releases page.

## Documentation

- User guide: [`docs/guide/en`](docs/guide/en) (English), [`docs/guide/fr`](docs/guide/fr)
  (French); the desktop application includes a copy.
- Manual pages of the command line: [`docs/man`](docs/man) (French), installed by the
  `.deb` package (`man auro`, `man auro-add`, `man aurod`).
- Specification, engine protocol and design decisions, in French:
  [`docs/SPEC.md`](docs/SPEC.md), [`docs/protocol`](docs/protocol).

## Building from source

You need the stable Rust toolchain (1.90 or later). On Windows, the Visual Studio C++ Build
Tools (SQLite is compiled with the project). On Linux, for the desktop application,
WebKitGTK 4.1 and its development files (`libwebkit2gtk-4.1-dev` and the packages listed in
[`.github/workflows/ci.yml`](.github/workflows/ci.yml)).

```
cargo test --workspace
cargo run -p auro-cli -- get https://deb.debian.org/debian/README -o .
```

The desktop application is a separate Cargo workspace:

```
cd apps/desktop
cargo test
```

Installers, the Android application, the benchmarks and every other command are described in
[`CLAUDE.md`](CLAUDE.md) (in French), and the release procedure in
[`docs/PUBLICATION.md`](docs/PUBLICATION.md).

## Repository layout

| Folder | Contents |
| --- | --- |
| `crates/engine` | The engine (`auro-engine`): storage, segmentation, network, queues, API |
| `crates/daemon` | The engine as a standalone process, `aurod` |
| `crates/cli` | The command line `auro` and the browser extension relay `auro-relay` |
| `crates/android` | The engine inside the Android application |
| `apps/desktop` | The desktop application (Tauri 2) |
| `apps/android` | The Android application (Kotlin, Jetpack Compose) |
| `ui` | The page of the desktop application, also served as the remote web interface |
| `extension` | The browser extension |
| `plugins` | Example host plugin and the plugin author's guide |
| `docs` | Specification, protocol, guide, manual pages, reviews and measurements |

## Security

Please report vulnerabilities privately, through GitHub's
[security advisories](https://github.com/auriciel21/AuroDownloader/security/advisories/new),
not in public issues. The security review of the remote interface, the plugins and the local
channel is in [`docs/SECURITE.md`](docs/SECURITE.md) (in French).

## License

AuroDownloader is free software, under the GNU General Public License, version 3 or (at your
option) any later version: see [`LICENSE`](LICENSE).

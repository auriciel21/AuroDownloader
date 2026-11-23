# Installation

Packages are on the project's [Releases](https://github.com/aurodownloader/aurodownloader/releases)
page. Each release also provides `SHA256SUMS`, the checksum of every file, to check a
download.

## Windows 10 and 11

1. Download `AuroDownloader_<version>_x64-setup.exe` (most computers) or `_arm64-setup.exe`
   (computers with an ARM processor).
2. Run it. It installs for your account only, without administrator rights, in
   `%LOCALAPPDATA%\AuroDownloader`. If WebView2 is missing (rare on Windows 11), the
   installer downloads it.
3. If Windows shows "Windows protected your PC" and "unknown publisher", the installer is not
   signed. Choose "More info", then "Run anyway", if you did get it from the Releases page.

The installer also registers the [extension](09-extension.md)'s relay with the browsers.

## Linux

WebKitGTK 4.1 is required: Ubuntu 22.04, Debian 12 or later.

- **`.deb` package** (Debian, Ubuntu and derivatives):

  ```
  sudo apt install ./AuroDownloader_<version>_amd64.deb
  ```

  (`_arm64.deb` on an ARM computer). It registers the extension's relay with the browsers and
  installs the manual pages (`man auro`).
- **AppImage**: make the file executable (`chmod +x AuroDownloader_<version>_amd64.AppImage`)
  and run it. The AppImage updates itself, with your consent; it does not register the
  extension's relay.

## Android 8 and later

Download `AuroDownloader_<version>_arm64.apk` on the device and open it. Android asks you to
allow installing from that source (the browser or the file manager). See
[Android](10-android.md).

## Browser extension

See [Browser extension](09-extension.md).

## Updates

The desktop application looks for updates at most once a day and offers a new version;
nothing is installed without your consent (Options, General, "Updates"). The `.deb` package
only tells you, with a link to the release page: install the new package like the first one.
On Android, install the new APK over the old one.

## Uninstalling

- **Windows**: Settings, Apps, AuroDownloader, Uninstall. Your downloaded files are never
  touched. The download list, the history and the preferences are kept, unless you tick
  "delete the data": it also deletes the logs, the plugins, the video tools, the remote
  access certificate, and the cookies and passwords kept in the Windows vault.
- **Linux**: `sudo apt remove auro-downloader` for the `.deb`; simply delete the AppImage
  file.
- **Android**: like any application. Files already downloaded stay in
  `Download/AuroDownloader`.

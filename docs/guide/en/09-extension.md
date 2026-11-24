# Browser extension

The extension hands the browser's downloads, magnet links and the page's video to
AuroDownloader. It works with Firefox (128 and later), Chrome (121 and later) and Edge, with the
desktop application installed (it goes through its relay, `auro-relay`).

## Installing

The extension is not yet in the browser stores; it is loaded from the project's `extension`
folder:

- **Chrome**: `chrome://extensions`, "Developer mode", "Load unpacked", `extension` folder.
- **Edge**: `edge://extensions`, "Developer mode", "Load unpacked". Edge reports a warning
  about `background.scripts` (for Firefox): it has no effect.
- **Firefox**: `about:debugging#/runtime/this-firefox`, "Load Temporary Add-on…", file
  `extension/manifest.json` (to load again at each Firefox start). Then, in `about:addons`,
  AuroDownloader, Permissions: allow access to all sites, otherwise cookies cannot be read.

On Linux, the relay is registered with the browsers only by the `.deb` package, not by the
AppImage.

If IDM or another download manager is installed, disable its extension: otherwise it takes the
downloads before AuroDownloader.

## What it does

- **Downloads**: those over 1 MiB (adjustable) go to AuroDownloader. The browser keeps its own
  until AuroDownloader has accepted it; the icon shows "✓" or "!".
- **Click with Alt**: leaves that download, or that magnet link, to the browser.
- **Magnet links**: a click hands them to AuroDownloader (option on by default). Downloaded
  `.torrent` files become torrents.
- **Context menu**: "Download with AuroDownloader" on a link, an image, a video or a sound;
  "Download this page's video with AuroDownloader" (best quality).
- **Icon window**: the page's video, with the choice of quality, audio only and subtitles,
  then "Download".

## Settings

Icon window, "Settings":

- Hand downloads to AuroDownloader, only from a given size;
- Excluded sites (one per line; their subdomains too);
- In private browsing too (downloads and magnet links of private windows);
- **Send the page's cookies and address**: for downloads that require signing in;
  AuroDownloader keeps the cookies in the system vault;
- Hand magnet links to AuroDownloader;
- **Link with AuroDownloader**, "Check": "Relay found" if all is well.

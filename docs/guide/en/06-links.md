# Link grabber

## Every file of a page

1. "Links" opens "Analyse links".
2. Paste the address of a web page, or a text containing addresses, then "Analyse"; or "Import
   a list (text, CSV)…".
3. AuroDownloader reads the page (without running JavaScript) and lists the file links, with
   their name, size and address. Sizes are asked from the servers, four at a time.
4. **Filters**: extensions (`iso, zip`), minimum and maximum size, regular expression on the
   address. "Also show pages and links without an extension" shows the rest.
5. Tick ("Check all", "Uncheck all"), choose the folder, then "Download".

Each link becomes a download; a refused link does not stop the others.

## Copied links

Option "Offer to add copied links" (Options, General, off by default): when you copy the
address of a file or a magnet link, a system notification offers it; a click opens "Add". The
clipboard is only read for that, and nothing from it is kept. If AuroDownloader's
notifications are turned off in the system, the Options say so.

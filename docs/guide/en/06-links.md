# Link grabber

## Every file of a page

1. "Links" opens "Analyse links".
2. Paste into "Address of a web page, or text containing addresses" the address of a page,
   or a text containing addresses, then "Analyse"; or "Import a list (text, CSV)…". The page
   is read as is: its linked pages are only followed with "Explore the site" (below).
3. AuroDownloader reads the page (without running JavaScript) and lists the file links, with
   their name, size and address. Sizes are asked from the servers, four at a time.
4. **Filters**: Extensions (`iso, zip`), Minimum size and Maximum size (`10M`, `1G`…),
   Regular expression on the address. "Also show pages and links without an extension" shows
   the rest. The filters apply at once to the table (Download, Name, Size, Address).
5. Tick ("Check all", "Uncheck all"), choose "Save to" and, if needed, a "Subfolder", then
   "Start" or "Download later".

Each link becomes a download; a refused link does not stop the others.

## Explore a site

Like IDM's "Grabber": AuroDownloader reads the pages of a site link after link and lists the
files they cite. Nothing is downloaded before you choose.

1. In "Analyse links", paste the address of a page of the site then, under "Site
   exploration", tick "Explore the site: also read the linked pages of the same site".
2. Set the exploration, or choose a **Template**:
   - "All the images of the site", "All the videos of the site", "All files except pages and
     images" (archives, documents, music, programs, videos): depth 5;
   - "Audio of the page and one level below", "Documents of the page and one level below":
     depth 1.

   A template ticks the exploration and fills in the depth and the "Extensions" filter;
   changed by hand, the template goes back to "Custom".
   - **Depth (1 to 5)**: 1 reads the page and the pages it cites; 2, also those they cite,
     and so on.
   - **Pages read at most**: 200 by default, 2,000 at most.
   - **Pages not to open (regular expression)**: by default the sign-out pages (`logout`,
     `signout`, `déconnexion`…), which would close a session; empty the field to open
     everything.
   - **Stay under the starting folder**: only the pages under the folder of the pasted page.
   - **Ignore robots.txt**: also reads the pages the site asks robots not to read; keep it
     for your own sites.
3. "Analyse": the progress is shown (pages read, waiting, links found); "Stop the
   exploration" stops while keeping what was found. Closing the box stops it too.
4. At the end, the table gains two columns: **Level** (1 for a link of the starting page, 2
   for a link of a page it cites…) and **Found on** (the page that cites it). With "Also show
   pages and links without an extension", each page says whether it was read, or why it was
   not (beyond the depth, another site, robots.txt, excluded…).
5. Filter and tick as for a page. **Keep the tree of the site** puts each file into a
   subfolder named after the site, then its folders (8 levels at most); otherwise, they all
   go into the chosen folder.

What the exploration does:

- only the pages of the **same site** (same host name) are read; the files they cite are
  shown even when they are on another site (images of a third-party service, for example);
- two pages at a time at most, with a quarter of a second between two, so as not to
  overload the site; robots.txt is respected;
- a page protected by a password is read with the logins saved for its site (Options,
  Site logins); a sign-in form is not filled in;
- a link that leads to a file is never downloaded during the exploration: it is listed among
  the files;
- the exploration also stops after 256 MiB of pages read or 10,000 links found.

From the [remote page](11-remote-access.md), the exploration runs without the site logins,
and only on the Internet (never the local network).

## Copied links

Option "Offer to add copied links" (Options, General, off by default): when you copy the
address of a file or a magnet link, a system notification offers it; a click opens "Add". The
clipboard is only read for that, and nothing from it is kept. If AuroDownloader's
notifications are turned off in the system, the Options say so.

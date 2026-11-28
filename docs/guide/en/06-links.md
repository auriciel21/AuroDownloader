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
     `signout`, `déconnexion`…), which would close a session, and the sort links of folder
     listings (`?C=M;O=A`…), which give the same listing again; empty the field to open
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

## Copy a site for offline browsing

After an exploration, the **Copy the site for offline browsing** box (below the table) makes
"Add" create a single task, "Site: example.org", which copies the site to disk to read it
without a connection:

- the explored pages, with the exploration settings (depth, pages at most, pages not to open,
  starting folder, robots.txt), and their images, styles, scripts and fonts, even from
  another site;
- the files checked in the table (checked pages are ignored: pages come from the
  exploration);
- the links of the pages lead to the copy; a link to something not copied keeps its online
  address.

The copy takes a folder named after the site in the chosen folder ("example.org", then
"example.org (1)" for a second copy), with one subfolder per site. It always keeps the tree
of the site: "Keep the tree of the site" does not apply.

During the copy, the list shows three numbers instead of a percentage (the size is only
known at the end): the items downloaded, the technical failures (server refusal, network)
and the ignored links, left online by choice. A link is ignored when robots.txt refuses it,
when it matches the "Pages not to open", when it leads to another site, beyond the depth or
outside the start folder, to a file not ticked or to a page redirected to a refused page,
when it does not lead to a web page (a text file, for example), when it is too big, or when
a limit stopped the copy: these are not failures. Once finished, **Open** shows the start
page in the browser, and **Show in folder** the folder of the copy. The Properties give the
start page, the settings, the numbers **Downloaded** (pages, resources and files),
**Technical failures** and **Ignored / left online**, then the list of each with its cause
or reason.

Limits: pages made by a script (JavaScript) are copied as the server sends them, often empty;
a login form is not filled in; the copy stops exploring after 1 GiB of pages and resources.
"Also delete finished files from the disk" only deletes what the copy wrote.

## Projects: saved and scheduled explorations

A project keeps an exploration to run it again by hand or at a set time: the start page,
the exploration settings, the filters (extensions, sizes, expression on the address), the
choice between the site copy and files only, the tree of the site and the folder. It does
not keep the checked files: each run explores the site again and takes the files that pass
the filters, new ones included.

In the "Links" dialog, **Save as project…** (with "Explore the site") shows the **Project**
part: **Project name** (the site's, proposed), **Run at a set time** with the **Time** and
the days (with no day checked: once only) and **Run now**. **Save the project** saves it;
**Back to the links** returns to adding. The filters of the dialog choose the files; without
**Copy the site for offline browsing**, the project only downloads those files ("files
only"), flat or with **Keep the tree of the site**, and its row is called "Files:
example.org".

The **Projects** dialog (toolbar) lists the projects: name, start page, schedule, next and
last run, with the state of its copy and its numbers. **Run** runs it at once, **Edit**
reopens the "Links" dialog filled in, **Delete** deletes it (its task and its files stay);
**New project…** creates one.

A project has a single task, taken up again at each run: its copy is updated in place, in
the same folder. Pages are read again; a resource or file that the server says is unchanged
is not downloaded again, and counts in **Unchanged** (in the list and the Properties); a
changed file replaces the old one; a file gone from the site stays on the disk. A missed run
(computer off, engine stopped) is done once at the next start; if the copy is still running
at the set time, that run is skipped. Removing the task from the list does not delete the
project: its next run makes a new copy.

## Copied links

Option "Offer to add copied links" (Options, General, off by default): when you copy the
address of a file or a magnet link, a system notification offers it; a click opens "Add". The
clipboard is only read for that, and nothing from it is kept. If AuroDownloader's
notifications are turned off in the system, the Options say so.

# First steps

## The window

- **Toolbar**, at the top: Add, Video, Links, Resume, Stop, Resume all, Stop all, Delete,
  Clear finished, Properties, Schedule, History, Options, Help. "Help" opens a menu: User
  guide (F1), Keyboard shortcuts, About.
- **Categories**, on the left: All downloads, Unfinished, Finished, each with its file types
  (Archives, Documents, Images, Music, Programs, Videos, Other), then the queues. The number of
  downloads is next to each category.
- **Table**, in the middle: File name, Size, Progress, Status, Time left, Speed, Added. Clicking
  a column header sorts the table.
- **Status bar**, at the bottom: what is going on, and the global speed limit.

On first launch, AuroDownloader tells you it looks for updates at most once a day; "Don't
check" turns that off.

## A first download

1. Copy the address of a file, for example `https://deb.debian.org/debian/README`.
2. "Add" (or Ctrl+N), paste the address into "Addresses or magnet links, one per line".
3. "Save to" shows the folder: by default, your Downloads folder; "Browse…" picks another one.
4. "Start". The download appears in the table, with its progress.

"Download later" adds it paused: "Resume" will start it.

## What you can do with a download

Select a row (several with Ctrl or Shift), then use the toolbar, or right-click for the menu:
Open, Show in folder, Download again…, Move/Rename…, Refresh address…, Resume, Stop, Limit
speed…, Delete…, Properties.

- **Stop** pauses; **Resume** carries on where it stopped.
- **Delete…** removes it from the list; for a finished download, the "Also delete finished
  files from the disk" box deletes the file too.
- **Clear finished** removes completed and cancelled downloads from the list; their files stay
  on disk, and the [history](07-organising.md#history) keeps them.
- A double click, or Enter, opens the **Properties**.

A finished file opens with the system's program; for safety, AuroDownloader never opens a
program or an installation package: "Show in folder" shows it.

## Closing the window

Closing the window keeps the application in the notification area (next to the clock):
downloads go on. The notification area icon reopens the window, or quits: "Quit (downloads
continue)" or "Quit and stop downloads". This is set in the [Options](08-options.md#general).

When the computer restarts, unfinished downloads resume when AuroDownloader starts again;
"Start with the session" does that by itself.

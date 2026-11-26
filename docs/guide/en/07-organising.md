# Organising

## Queues

Every download is in a queue. The main queue exists from the start; it runs 3 downloads at a
time, the others wait ("Queued"). Other queues are created from the command line, with their
number of simultaneous downloads and their own speed limit:

```
auro queue create Night --max-active 2 --limit 2M
auro add <address> --queue Night
```

Queues appear under "Queues", in the categories.

## Speed limits

- **Global**: at the bottom of the window, "Global speed limit", or Options, Connection.
- **Of one download**: menu "Limit speed…", or its Properties.
- **Of a queue**: from the command line (`auro limit 1M --queue Night`).

A changed limit applies at once, to running downloads too.

## Schedule

"Schedule" sets, for the queue chosen in "Queue":

- **Download only during these time slots**: days and hours ("Add a time slot"), for example
  Monday to Friday from 23:00 to 07:00. Outside the slots, the queue waits ("Paused
  (schedule)") and starts again by itself when a slot opens.
- **When the queue has nothing left to download**: Do nothing, Put the computer to sleep or
  Shut down the computer. The action is announced a minute before, with a button to cancel it.

"Save" applies the schedule; an open slot starts the queue's waiting downloads at once.

## Filing rules

Options, Rules. A rule files the downloads added **without a chosen folder** (by the
extension, copied links, the remote interface):

The "Filing rule" box has a **Name** and an **Enabled** box, then:

- **If** (all criteria met; none: every download): **File types** (Archives, Documents,
  Images, Music, Programs, Videos…) **or extensions** (`iso, pdf`; one ticked type or one
  listed extension is enough), **Site domains**, **Address (regular expression)**, **File
  name (regular expression)**.
- **Then**:
  - a **folder**: full path, or relative to the download folder (`Videos/Talks`), created if
    missing;
  - a **file name**, a template with `{name}` (without the extension), `{ext}`, `{host}` and
    `{date}` (day added): for example `{date} {name}.{ext}`;
  - a **Command when the download ends**: "Program (full path)", "Arguments, one per line",
    and "Time limit (seconds)". Variables: `{path}` (the file), `{dir}`, `{name}`, `{size}`, `{sha256}`,
    `{id}`. For example, an antivirus with `{path}`. The command never goes through a shell;
    the download stays "Finishing" while it runs, then fails if it fails or runs past its time
    limit ("Resume" runs it again).

The first enabled rule that matches applies; "Move up" and "Move down" change the order. A
torrent only takes the folder. Rules can only be changed in the application on the computer
(not from the remote interface).

## History

"History" lists finished downloads, even those removed from the list, most recent first:
Finished, Name, Size, State (completed, failed, cancelled), Site.

- **Search (name, folder or site)**: the list filters as you type.
- **Show more**: the next entries, page by page.
- **Export as CSV…**: the list, to open in a spreadsheet.
- **Clear history…**: after confirmation ("Clear"), the history is emptied; the files stay on
  disk.

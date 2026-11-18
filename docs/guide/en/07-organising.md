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

"Schedule" sets, for a queue:

- **Download only during these time slots**: days and hours ("Add a time slot"), for example
  Monday to Friday from 23:00 to 07:00. Outside the slots, the queue waits ("Paused
  (schedule)") and starts again by itself when a slot opens.
- **When the queue has nothing left to download**: Do nothing, Put the computer to sleep or
  Shut down the computer. The action is announced a minute before, with a button to cancel it.

## Filing rules

Options, Rules. A rule files the downloads added **without a chosen folder** (by the
extension, copied links, the remote interface):

- **If**: file types or extensions, site domains, regular expression on the address or the
  file name (all criteria met; empty: any task).
- **Then**:
  - a **folder**: full path, or relative to the download folder (`Videos/Talks`), created if
    missing;
  - a **file name**, a template with `{name}` (without the extension), `{ext}`, `{host}` and
    `{date}` (day added): for example `{date} {name}.{ext}`;
  - a **command when the download ends**: a program (full path), its arguments, one per line,
    and a time limit. Variables: `{path}` (the file), `{dir}`, `{name}`, `{size}`, `{sha256}`,
    `{id}`. For example, an antivirus with `{path}`. The command never goes through a shell;
    the download stays "Finishing" while it runs, then fails if it fails or runs past its time
    limit ("Resume" runs it again).

The first enabled rule that matches applies; "Move up" and "Move down" change the order. A
torrent only takes the folder. Rules can only be changed in the application on the computer
(not from the remote interface).

## History

"History" lists finished downloads, even those removed from the list: search by name, folder
or site, "Export as CSV…", "Clear history…".

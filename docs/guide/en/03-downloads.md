# Downloads

## Adding

The "Add downloads" box takes:

- **addresses** `http://`, `https://`, `ftp://`, `ftps://` or `sftp://`, one per line (several
  at once);
- **magnet links** (see [Torrents](04-torrents.md));
- **`.torrent` or Metalink files** ("Choose .torrent or Metalink files…", or dropped on the
  window).

Optional fields:

- **Mirrors**, one per line: other addresses of the same file. Each one is checked (same size,
  same bytes) before use, and the download is spread between them by speed; a failing mirror
  is set aside.
- **Expected checksum**: SHA-256, SHA-1 or MD5 (`sha256:…` or the value alone). The file only
  takes its name once checked; otherwise the download fails with "the file does not match its
  expected checksum" and the file keeps its temporary name.
- **Save to**: the folder ("Browse…" picks another one).
- **Subfolder**: created in the folder above, for example `Music/Album`; handy from the
  [remote interface](11-remote-access.md), which does not choose a folder.

"Start" starts the downloads at once; "Download later" adds them paused.

If the address is refused, the box stays open and says why, with the cursor in the field
concerned.

A page of a file host supported by a [plugin](08-options.md#plugins) is read by that plugin;
"Add these pages as they are, without a plugin" avoids it.

## Statuses

Queued (queue full, or time slot closed), Downloading, Paused, Retrying soon (network or server
outage: AuroDownloader retries by itself), Verifying the checksum, Finishing (the command of a
[rule](07-organising.md#filing-rules) is running), Completed, Failed (the reason is given),
Cancelled, Seeding (torrent).

A full disk pauses the download ("disk full"): it starts again by itself once there is room.

## Properties

Double click on a row, Enter, or "Properties". The box keeps up to date while downloading.

- **At the top**: File, Folder, Address, Source page (where the link came from, through the
  extension or a plugin), Status, Size, Downloaded, Speed, Time left, Added, Finished, then a
  progress bar and a **map** of what has been received, part by part.
- **Connections**: the number of connections of this download (automatic by default:
  AuroDownloader adds some as long as the speed gains from it, and removes some if the server
  is overloaded).
- **Can resume**: whether the server allows resuming where the download stopped; otherwise,
  resuming starts over.
- **Checksum**: the expected one, or the one found next to the file, and whether it was
  checked. **Pieces** (Metalink): how many and how big, and how many were downloaded again
  because they were faulty.
- **Sent to other peers** (torrent): what was shared; the status gives the ratio.
- **SSH server key** (SFTP): the fingerprint of a key not yet known, and "Accept the key…"
  (see below).
- **Segments and connections**: each part in progress (No., Downloaded, State) and the source
  sending it.
- **Sources**: the Main one and each Mirror, with their state (To be checked, Good, Slow, Set
  aside) and their Average speed. "Add a mirror", then "Add": another address of the same
  file, checked before use; a running download restarts with it at once.
- **Torrent files**: Download (box), File, Size, Received; untick what you do not want, then
  "Apply the choice". The list arrives with the torrent's metadata, once it has started.
- **Maximum speed of this download**, then "Apply": the global limit also applies, the lower
  one wins. For a torrent, a new limit applies when it next resumes.
- **Expected checksum**, then "Save": SHA-256, SHA-1 or MD5; empty to check none. It can be
  changed until the download is finished. After a checksum failure, "Retry" checks the
  received file again, without downloading it again.
- **At the bottom**: "Pause" (or "Resume", "Retry" after a failure, "Stop seeding" for a
  torrent), "Cancel the download", and, once finished, "Open" and "Show in folder".

## FTP, FTPS and SFTP

- Unencrypted FTP never sends a password, unless you allow it for that site ("FTP: allow
  without encryption", in the [site logins](08-options.md#site-logins)).
- SFTP: on first contact, AuroDownloader shows the **server key fingerprint**, in the
  Properties ("SSH server key", "Accept the key…", which asks "Accept this server's key?").
  Check it with the server's administrator before accepting it; downloads from that server then resume. A key that changes blocks the
  downloads: it may be an attack.
- Logins: name and password, or an SSH private key, in the site logins. An address like
  `ftp://name:password@server/file` works too: the password is stored in the system vault and
  removed from the address.

## Sites that need an account

A site's login (Basic or Digest) is set in the [site logins](08-options.md#site-logins).
Passwords are kept in the system vault (Windows Credential Manager, Linux keyring), never in
AuroDownloader's database. For a site where you sign in through a page, use the
[extension](09-extension.md), which passes the cookies on.

## Metalink

A `.meta4` or `.metalink` file, or its address, gives each file's addresses and checksum. If it
holds several files, the box lets you tick them. Every faulty piece is downloaded again,
preferably from another source; a source that gives two faulty pieces is set aside.

## Moving, renaming, refreshing, downloading again

- **Move/Rename…**: "File name" and "Folder" ("Browse…"), then "Move"; even to another disk
  (the file is then copied, and the original deleted). Possible for a download that is not
  running, finished or not. A refused name (forbidden character, name already in use) is
  reported, never silently corrected.
- **Refresh address…**: replaces an expired link; the box shows the "Current address", you
  give the "New address", then "Refresh". For a started download, the new address must give
  the same file; what was received is kept.
- **Download again…**: starts over. "Replace the file already downloaded" overwrites it;
  otherwise the new one gets another name.

## Where files go

While downloading, the file has a temporary `.part` name; it takes its name at the end. A name
already in use gets a suffix (`file (1).iso`). [Filing rules](07-organising.md#filing-rules)
can choose the folder and the name.

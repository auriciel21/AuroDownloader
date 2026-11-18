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
- **Save to**: the folder.

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

Double click on a row, Enter, or "Properties":

- file, folder, address, source page, status, size, downloaded, speed, time left;
- **Connections**: the number of connections of this download (automatic by default);
- **Can resume**: whether the server allows resuming;
- **Checksum**: the expected one, or the one found next to the file, and whether it was
  checked; it can be given, changed or cleared here until the download is finished;
- **Mirrors**: each source, its state and speed; "Add a mirror";
- a **segment map**: what has been received, part by part;
- the **speed limit** of this download.

## FTP, FTPS and SFTP

- Unencrypted FTP never sends a password, unless you allow it for that site ("FTP: allow
  without encryption", in the [site logins](08-options.md#site-logins)).
- SFTP: on first contact, AuroDownloader shows the **server key fingerprint**, in the
  Properties ("SSH server key", "Accept the key…"). Check it with the server's administrator
  before accepting it; downloads from that server then resume. A key that changes blocks the
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

- **Move/Rename…**: another name or another folder, even on another disk; possible for a
  download that is not running, finished or not.
- **Refresh address…**: replaces an expired link. For a started download, the new address must
  give the same file; what was received is kept.
- **Download again…**: starts over. "Replace the file already downloaded" overwrites it;
  otherwise the new one gets another name.

## Where files go

While downloading, the file has a temporary `.part` name; it takes its name at the end. A name
already in use gets a suffix (`file (1).iso`). [Filing rules](07-organising.md#filing-rules)
can choose the folder and the name.

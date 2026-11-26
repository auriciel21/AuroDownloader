# Command line

`auro` drives the same engine as the application: a download added by `auro` shows up in the
window, and the other way round. It is installed with the application (next to it on Windows,
in the path on Linux with the `.deb` package). If the engine is not running, `auro` starts it.

Every command has its help (`auro add --help`) and, on Linux, its manual page (`man auro`,
`man auro-add`, `man aurod`). The command line's messages and manual pages are in French.

## Common commands

```
auro get https://deb.debian.org/debian/README -o .   # download and follow the progress
auro add <address>… --paused                         # add without waiting (paused)
auro list                                            # list
auro watch                                           # follow what is running
auro pause <id>… ; auro resume <id>…
auro remove <id>… ; auro restart <id>… --delete-files
auro move <id> -o <folder> -n <name>
auro refresh <id> <new-address>
```

Without `-o`, `auro add` downloads to the current folder; `--auto` lets the
[filing rules](07-organising.md#filing-rules) choose.

## Going further

```
auro add <address> --checksum sha256:<hex>          # expected checksum
auro get <address> --mirror <address>…              # mirrors
auro metalink <file.meta4> ; auro get <file.meta4> --select 0,2
auro add file.torrent ; auro files <id> --select 0,2-4
auro links <page> -e iso,zip --min-size 10M --add -o .
auro site <page> -o . --depth 2 ; auro site <page> -e pdf   # offline copy
auro video info <page> ; auro video get <page> --subs fr,en
auro limit 2M ; auro limit --upload 500K ; auro limit 1M --queue Night
auro schedule set --days 1-5 --from 23:00 --to 07:00 --then sleep
auro login set <site> -u <name> ; auro hostkey list
auro proxy manual socks5://host:1080 -u me ; auro proxy system
auro history [text] [--csv]
auro config ; auro config torrent.ratioLimit 1.5
auro diagnostic --save diag.zip
auro engine stop
```

## Exit codes

`auro` returns 0 when everything is done, 1 after an error or when part of the request
was refused (for instance one file of a Metalink, the others being added), 2 for a
mistyped command, and 130 when Ctrl-C stops a follow-up (`get` then pauses its tasks).
A displayed address never shows its password.

## The engine alone

`aurod` is the engine, without a window. For a server or a NAS, it runs on its own, and is
used through `auro` or through [remote access](11-remote-access.md).

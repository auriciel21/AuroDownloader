# Remote access

The remote web interface shows the AuroDownloader window in a browser, on a phone or another
computer on the same network, to add and follow downloads. It is off by default.

## Turning it on

Options, Remote access:

1. **Control downloads from another device**: tick it.
2. **Listening address** and **Port**: this computer's address on the local network (the list
   offers them), and the port (7380 for example). Windows may ask to allow AuroDownloader
   through the firewall.
3. Away from this computer, the connection is always encrypted (HTTPS). AuroDownloader makes
   its own certificate; the other device's browser warns the first time: compare the
   **SHA-256 fingerprint** shown in the Options before accepting. "Use my certificate…" takes a
   certificate of yours instead.

## Adding a device

"Add a device…": a **Device name** ("Phone"), then the **Permissions**:

- Full control: add, stop, delete;
- Add only;
- View only.

"Create the token" shows the **Page address** and a **QR code**: scan it with the device,
which opens the page signed in. Otherwise, open the address in its browser: the "Sign in to
AuroDownloader" box asks for the **Token**, to copy from "Token, to paste into the page if
needed" ("Copy"), then "Sign in". The token is shown only once; "Remove" a device cuts its
access at once. On the device, "Sign out" (the page's toolbar) closes its session.

## What the remote page does not do

For safety, it does not choose a folder on the computer (only a subfolder of the download
folder), does not open files, does not change the rules, the plugins or the remote access
settings, and only downloads from the Internet, not from the computer's local network (unless
`auro remote allow-private on`, from the command line).

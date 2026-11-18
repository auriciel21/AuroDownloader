# Accessibility

AuroDownloader can be used entirely from the keyboard, and has been tried with NVDA (Windows),
Orca (Linux) and TalkBack (Android).

## Keyboard, in the window

| Keys | Effect |
| --- | --- |
| F1 | This guide (also in Options, About) |
| Ctrl+N or Ctrl+L | Add |
| Tab, Shift+Tab | Toolbar, categories, table |
| Up and down arrows, Page Up, Page Down, Home, End | Previous row, next row… |
| Shift+arrows | Extend the selection |
| Ctrl+Space | Add a row to the selection, or remove it |
| Ctrl+A | Select all |
| Enter | Properties |
| Delete | Delete… |
| Shift+F10 or Menu key | Context menu |
| Escape | Close a menu or a box; focus goes back where it came from |
| Left and right arrows | Next or previous section of the Options |
| Ctrl+plus, Ctrl+minus, Ctrl+0 | Zoom in, zoom out, normal size |

In the "Add" box, Enter starts, Shift+Enter starts a new line.

## Screen readers

- Messages (refused addition, checksum cleared, download completed…) are announced; an error
  tied to a field is read with it.
- **Orca**: version 46 treats the application as a WebKitGTK page; in a field, letters move
  around while structural navigation is on: Orca+Z turns it off before typing. Orca 46 does
  not announce the page's status messages either.

The detail of the accessibility review (WCAG 2.2, levels A and AA, in French) is in
[`docs/ACCESSIBILITE.md`](../../ACCESSIBILITE.md).

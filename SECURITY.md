# Security & privacy

ClipVault sees everything you copy, so it's built to keep that data local and minimal.

## Guarantees

- **Local only.** History lives in `~/.local/share/clipvault/` (mode `0700`) and backups in `~/Documents/ClipVault Backups/` (files `0600`). No cloud, account, telemetry or analytics. Neither process makes network connections: the panel's Content-Security-Policy only allows its own assets, IPC and the local `cvmedia:` image scheme, and the daemon has no network code.
- **Local IPC only.** The panel and daemon talk over Unix sockets in `$XDG_RUNTIME_DIR/clipvault/` (directory `0700`, sockets `0600`), so only your user can connect. The protocol is a fixed set of typed requests; unknown requests are rejected.
- **No content in logs.** Logs record sizes, MIME types and reasons only.
- **Runs unprivileged.** Root is never needed. `sudo` is used only by `install.sh` to install the `.deb`.
- **Read-only inspection.** Reading the clipboard never changes its owner. ClipVault takes ownership only when you choose an item.
- **Original files are safe.** For copied files only paths/URIs are stored. Deleting entries never touches the files.
- **Narrow UI surface.** The panel has no filesystem, shell or HTTP permissions. Images are served by item id, never by path. Backup restore only accepts file names inside the backup folder.
- **Safe rendering and decoding.** Rich text is sanitized with DOMPurify (scripts, event handlers, forms, frames, styles and remote images removed), and the CSP blocks remote loads as a second layer. Images are decoded with dimension and allocation limits.

## What `--setup` changes on your desktop

All per user and all undone by `clipvault --remove-integration` / `scripts/uninstall.sh`:

| Change | Why |
|---|---|
| `~/.config/autostart/clipvault.desktop` | start the capture service at login |
| GNOME custom shortcut `…/custom-keybindings/clipvault/` running `clipvault --toggle` | Super+V |
| Removes `<Super>v` from `org.gnome.shell.keybindings toggle-message-tray` (Super+M still works) | frees Super+V; restored on removal |
| `~/.local/share/gnome-shell/extensions/clipvault@clipvault.dev/` + `enabled-extensions` | optional extension |

No system files are modified.

## The GNOME extension

- It grabs the shortcut, positions and focuses the ClipVault window, and on request sends Ctrl+V to the window that was focused before the panel opened.
- It exports `dev.clipvault.Shell` (`Ping`, `Toggle`, `Paste`) on the session bus. Any process of your user session could call `Paste`, which would type Ctrl+V into the previous window. That's no more than any session process can already do through accessibility or input tools.
- It never reads clipboard contents.
- Disabling it restores the keybindings it changed.

## Sensitive-content filter (on by default)

Skipped automatically:

- Content the source marks secret (`x-kde-passwordManagerHint`, used by KeePassXC and others).
- Well-known token formats: private-key blocks, AWS, GitHub, GitLab, Slack, Stripe, Google API keys, `sk-…` keys and JWTs.
- Password-shaped single words (8–64 characters mixing upper case, lower case, digits and symbols, and not a URL, path or e-mail).

This is a heuristic safety net, **not** a guarantee. Pause capture (top-bar icon or panel) before copying secrets you care about.

## Threat model notes

- Anyone with access to your user account can read the history, like browser history. Use full-disk encryption.
- Any X11/Xwayland client in your session can read the clipboard. ClipVault doesn't widen that, but it makes the data persistent.
- OCR (off by default) runs the local `tesseract` binary on image files from ClipVault's own media store.

## Reporting

Please report vulnerabilities privately to the maintainer rather than in a public issue.

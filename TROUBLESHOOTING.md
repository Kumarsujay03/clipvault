# Troubleshooting

Start with:

```fish
pgrep -a clipvault                     # clipvaultd should always be listed; clipvault only while the panel is loaded
clipvault --setup                      # re-applies start-at-login, Super+V and the extension
RUST_LOG=clipvault_core=debug clipvaultd   # run the service in the foreground (stop the old one first: clipvault --quit)
```

Logs never contain clipboard content.

### Super+V does nothing

- Open the panel another way (app grid → ClipVault), go to **Settings → Keyboard shortcut** and read the status line. Press **Use Super+V** if no shortcut is registered.
- Check GNOME's side:

```fish
gsettings get org.gnome.shell.keybindings toggle-message-tray        # should NOT contain '<Super>v'
gsettings get org.gnome.settings-daemon.plugins.media-keys custom-keybindings
```

- If the GNOME extension is active, it owns Super+V and removes the fallback shortcut. Check it with `gnome-extensions info clipvault@clipvault.dev`.

### The extension says "log out and back in"

GNOME on Wayland only discovers newly installed extensions at login. After logging in again:

```fish
gnome-extensions list --enabled | grep clipvault
```

If it's missing, run `gnome-extensions enable clipvault@clipvault.dev`. If GNOME reports it as incompatible after a GNOME upgrade, add your Shell version to `shell-version` in `~/.local/share/gnome-shell/extensions/clipvault@clipvault.dev/metadata.json`.

### Items are copied but not pasted automatically

Auto-paste needs the active GNOME extension and *Settings → Paste after choosing an item*. Without them, press Ctrl+V yourself. In terminals the extension sends Ctrl+Shift+V.

### "Clipboard recording service isn't running"

Press **Start** in the banner, or run `clipvault --daemon &`. If it exits immediately, run `clipvaultd` in a terminal to see why. Missing Xwayland is the usual cause:

```fish
echo $DISPLAY; pgrep -a Xwayland
```

### "X11 setup failed: Authorization required"

A capture service from an earlier login is still holding that session's (now deleted) Xwayland cookie. Ubuntu keeps user processes running after logout. Since 0.1.0 the service re-reads the current session's credentials on every reconnect, and a new login replaces a service that isn't working. To fix an older copy right away:

```fish
clipvault --quit; clipvault --daemon &
```

### Something wasn't recorded

- Capture may be paused (banner in the panel, or the top-bar icon menu).
- A toast *"Not saved — likely sensitive: …"* means the secret filter skipped it. You can change this under Settings → Capture.
- The item may exceed *Largest item*.
- Password managers that mark entries as secret are skipped on purpose.

### No top-bar icon

Check *Settings → Show icon in the top bar*, then check that Ubuntu's AppIndicator extension is enabled:

```fish
gnome-extensions list --enabled | grep -i appindicator
```

### The panel opens slowly

The first open after the panel unloaded starts WebKit (about 0.35 s). Raise *Settings → Performance → Keep panel loaded after closing* for instant reopening; that keeps about 265 MB in use while it's loaded.

### Database or settings were reset

A damaged database is renamed to `clipvault.db.corrupt-<timestamp>` in `~/.local/share/clipvault/`, and an invalid settings file to `settings.json.invalid`.

### Start completely fresh

```fish
./scripts/uninstall.sh --purge
./scripts/install.sh
```

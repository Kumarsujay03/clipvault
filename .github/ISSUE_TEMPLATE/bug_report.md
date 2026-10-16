---
name: Bug report
about: Something doesn't work
labels: bug
---

**What happened**

**What you expected**

**Steps to reproduce**

**System**
- Ubuntu version:
- GNOME Shell version (`gnome-shell --version`):
- Session (`echo $XDG_SESSION_TYPE`):
- ClipVault version (`clipvault --version`):
- GNOME extension active? (Settings → Keyboard shortcut):

**Logs** (`clipvault --quit; RUST_LOG=clipvault_core=debug clipvaultd`). They never contain clipboard text, but check for anything private before pasting.

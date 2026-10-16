# Changelog

All notable changes to ClipVault are listed here. Versions follow [Semantic Versioning](https://semver.org/).

## [0.2.1] - 2026-10-17

### Fixed
- **Dragging the panel really keeps it open now.** 0.2.0 still closed the panel when you pressed and held the top strip before moving: a 0.6-second safety timer switched the protection off before GNOME took focus away for the move. The panel now tracks the move itself (until you release the button or it regains focus), so press-and-hold, slow drags and quick drags all keep it open.
- Every time the panel hides, the log records why (for example "focus moved to another window" or "close button or Esc"), which makes problems like this easy to diagnose: `journalctl --user -b | grep "panel hidden"`.

## [0.2.0] - 2026-10-17

### Fixed
- **The panel no longer closes when you drag it.** On GNOME Wayland, the compositor briefly takes focus away from a window while it is being moved, and ClipVault mistook that for "clicked elsewhere". Dragging the top strip of the panel now moves it, and it stays open.

### Added
- **Updates from inside the app.** Settings → Updates shows your version, checks GitHub for a newer release and installs it for you, then restarts ClipVault on the new version.
  - `.deb` installs ask for your password through the normal system dialog.
  - AppImages are replaced in place, with no password needed.
  - Every download is checked against the release's `SHA256SUMS` before anything is replaced.
- **Optional daily update check** (Settings → Updates, off by default). ClipVault still makes no network requests unless you turn this on or press *Check for updates*.
- `clipvault --update` checks for and installs updates from the terminal, and `clipvault --restart` restarts the background service.
- **Seamless upgrades.** When a package upgrade replaces ClipVault on disk, the background service restarts itself on the new version within about 20 seconds. You no longer need to log out.
- The GNOME extension's files are refreshed automatically when a new version ships a changed extension; the change takes effect at the next login.

### Changed
- Release packages are now built by GitHub Actions on Ubuntu 24.04, so the `.deb` and AppImage run on Ubuntu 24.04 and newer.

## [0.1.0] - 2026-10-16

First public release: Super+V clipboard panel, the ~5 MB background service, pins, favorites, tags, search, image and file history, optional OCR, backup and restore, and the GNOME extension for opening at the pointer and auto-paste.

[0.2.1]: https://github.com/Kumarsujay03/clipvault/releases/tag/v0.2.1
[0.2.0]: https://github.com/Kumarsujay03/clipvault/releases/tag/v0.2.0
[0.1.0]: https://github.com/Kumarsujay03/clipvault/releases/tag/v0.1.0

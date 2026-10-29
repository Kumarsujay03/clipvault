# Developing ClipVault

## Layout

```
src/                         React + TypeScript panel (Vite, Tailwind v4, zustand)
  App.tsx                    panel shell + keyboard model
  components/                PanelHeader, Cards (virtual list), PreviewView, SettingsView, …
  lib/api.ts                 typed wrappers around Tauri commands/events
  lib/mock.ts                in-memory backend for browser preview & tests only
src-tauri/                   Cargo workspace
  src/                       `clipvault` — launcher + panel (Tauri). No capture code.
    main.rs                  CLI fast paths (--toggle/--show/--hide/--quit/--setup/--daemon)
    app.rs                   window lifecycle: show, hide-on-blur, unload after keep-alive
    commands.rs              validated commands for the UI
    media_protocol.rs        cvmedia:// serves images by item id
  core/                      `clipvault-core` — engine, no GUI toolkit
    src/bin/clipvaultd.rs    the background service
    src/daemon.rs            capture → ingest → store, IPC server, tray, maintenance
    src/capture/xfixes.rs    X11/Xwayland clipboard backend (read + own/serve, INCR)
    src/ingest.rs            classify, filter secrets, dedupe, store
    src/store/               SQLite repository + migrations (FTS5 trigram)
    src/ipc.rs               JSON-lines protocol over Unix sockets
    src/desktop.rs           GNOME shortcut, extension install, paste request
    src/autostart.rs         ~/.config/autostart entry
    src/tray.rs              StatusNotifierItem over D-Bus (ksni, no GTK)
    src/export.rs, ocr.rs, classify.rs, sensitive.rs, media.rs, restore.rs
    tests/live_clipboard.rs  live tests against the real desktop clipboard
gnome-extension/             optional GNOME Shell companion (embedded into the binaries)
scripts/                     build-daemon.mjs, install.sh, uninstall.sh
```

## Commands (fish)

```fish
git clone https://github.com/Kumarsujay03/clipvault.git
cd clipvault
npm ci

# Use a throwaway profile: separate data, config and sockets. Desktop integration
# (autostart, shortcuts, extension) is never touched for such profiles.
set -x CLIPVAULT_DATA_DIR /tmp/cv-dev/data
set -x CLIPVAULT_CONFIG_DIR /tmp/cv-dev/config
npm run tauri dev                 # builds clipvaultd too; the panel starts it on demand

npm run dev                       # panel UI in a browser, labelled "Preview — demo data"
npm test                          # vitest + Testing Library
npm run typecheck
cd src-tauri
cargo test --workspace --lib      # unit tests (store, ingest, ipc, export, desktop, …)
cargo test -p clipvault-core --test live_clipboard -- --ignored --test-threads=1   # overwrites your clipboard
cargo clippy --workspace --all-targets
cd ..
npm run tauri build               # release .deb + AppImage (clipvaultd bundled as sidecar)
```

Logging uses `RUST_LOG` (default `clipvault_core=info`). Never log clipboard contents: log sizes, MIME types and reasons only.

## Releasing a new version

1. Bump the version in `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` and `src-tauri/core/Cargo.toml`.
2. Add a section to `CHANGELOG.md`; its text becomes the release notes.
3. Commit, then tag and push: `git tag v0.3.0 && git push origin main v0.3.0`.
4. The **Release** workflow builds the `.deb` and AppImage on Ubuntu 24.04, writes `SHA256SUMS` and publishes the GitHub release. If that version is already released (for example a tag pushed again after rewriting history), the workflow skips instead of failing, and the published files stay as they are. Installed copies then see it under Settings → Updates and through `clipvault --update`.

To test the update flow locally, pretend to be an older version: `CLIPVAULT_FAKE_VERSION=0.0.1 clipvault --update`.

## Design notes

- **Two processes.** `clipvaultd` is always on, small, and links no GTK/WebKit (check with `ldd`). The panel is on demand. They share the SQLite database (WAL mode). The panel reads and writes history directly. Only clipboard ownership (restore), pause and settings reloads go through IPC, and the daemon pushes events (`history_changed`, `status`, `skipped`) to the panel.
- **Updates.** `src-tauri/src/update.rs` (panel process only) talks to GitHub, verifies `SHA256SUMS` and installs through `pkexec apt-get` or by replacing the AppImage. The daemon watches `/proc/self/exe`; when a package upgrade deletes its binary it re-execs the new one.
- **Panel lifecycle.** The panel shows when the frontend calls `panel_ready` (1.5 s fallback), hides on Esc, close or blur, and calls `exit(0)` after `panel_keep_alive_secs` hidden. `clipvault --toggle` writes `toggle` to `$XDG_RUNTIME_DIR/clipvault/ui.sock` and exits in milliseconds if a panel is running.
- **Memory.** After image ingestion and restores the daemon calls `malloc_trim(0)`, so a 4K screenshot doesn't leave tens of MB resident.
- **Migrations.** Append to `MIGRATIONS` in `core/src/store/migrations.rs`; never edit released ones. Migration 2 shows how to rebuild the FTS index.
- **Extension.** It's plain ESM for GNOME 45+ and uses only supported Shell APIs (`grab_accelerator`, `Main.activateWindow`, `Clutter` virtual keyboard). It's embedded in the binary with `include_str!` and installed per user by `--setup`. It never reads clipboard contents.
- **Adding a desktop.** Add a capture backend next to `xfixes.rs` speaking the same `Command` protocol and select it in `CaptureController::supervise` from `SessionInfo`.

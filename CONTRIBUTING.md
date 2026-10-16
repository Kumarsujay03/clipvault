# Contributing to ClipVault

Thanks for helping! Bug reports, fixes and support for more desktops are all welcome.

## Getting started

See [DEVELOPMENT.md](DEVELOPMENT.md) for the layout, build commands and design notes. Work in an isolated profile so your real clipboard history is untouched:

```bash
export CLIPVAULT_DATA_DIR=/tmp/cv-dev/data CLIPVAULT_CONFIG_DIR=/tmp/cv-dev/config
npm run tauri dev
```

## Before opening a pull request

```bash
npm run typecheck && npm test
cd src-tauri
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace --lib
```

If you touched capture or restore code, also run the live tests (they overwrite your clipboard):

```bash
cargo test -p clipvault-core --test live_clipboard -- --ignored --test-threads=1
```

## Ground rules

- **Never log clipboard contents.** Log sizes, MIME types and reasons only.
- **No network access.** No telemetry, update checks or remote fonts and images.
- **Database changes** go in a new migration appended to `core/src/store/migrations.rs`. Never edit a released one.
- **Keep the daemon small.** `clipvaultd` must not depend on GTK, WebKit or Tauri.
- **Wayland honesty.** If a desktop can't do something (paste, focus, global keys), say so in the UI and docs rather than faking it.

## Reporting bugs

Please include your Ubuntu/GNOME versions, X11 or Wayland, whether the GNOME extension is active (Settings → Keyboard shortcut), and the output of:

```bash
clipvault --quit; RUST_LOG=clipvault_core=debug clipvaultd
```

Remove anything private before posting. Logs never contain clipboard text, but paths may include your username.

Security issues: see [SECURITY.md](SECURITY.md).

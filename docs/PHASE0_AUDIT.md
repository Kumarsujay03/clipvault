# ClipVault — Phase 0: Environment Audit & Implementation Plan

Audit date: 2026-10-16. Everything below was measured on the development machine (Ubuntu 26.04.1, GNOME 50.1, Wayland), not assumed. Kept as a record of the platform constraints behind the design.

## 1. Environment

| Item | Found |
|---|---|
| OS | Ubuntu 26.04.1 LTS (resolute), kernel 7.0.0-38 |
| Desktop | GNOME Shell 50.1, Mutter (`libmutter-18`) |
| Session | **Wayland** (`WAYLAND_DISPLAY=wayland-0`), Xwayland running on `:0` |
| Shell | fish |
| Hardware | 16 cores, 14 GiB RAM, 249 GiB free on `/` |
| Repository | No existing ClipVault code; started from scratch. |

## 2. Toolchain & dependencies

| Requirement | Status |
|---|---|
| gcc / build-essential / pkg-config / curl / wget / file | ✅ installed |
| libgtk-3-dev, libsoup-3.0, libssl-dev, glib 2.88 | ✅ installed |
| libwebkit2gtk-4.1 runtime | ✅ installed — **`-dev` headers missing** |
| libayatana-appindicator3 runtime | ✅ installed — **`-dev` missing** (tray icon) |
| librsvg2-dev | ❌ missing |
| libxdo-dev | ❌ missing (Tauri menu/accelerator support) |
| Node.js / npm | ❌ missing (apt candidate: Node 22.22, npm 9.2) |
| Rust / Cargo / rustup | ❌ missing (apt candidate: rustup 1.27) |
| Tauri CLI | ❌ missing — will be a project-local npm devDependency, no global install |
| wl-clipboard (`wl-copy`/`wl-paste`) | ❌ missing — needed only for automated capture tests |
| sqlite3 CLI | ❌ missing — optional, for debugging (the app bundles SQLite) |
| xclip, xsel | ✅ installed |
| xdg-desktop-portal + gnome backend | ✅ active |
| AppIndicator extension (`ubuntu-appindicators@ubuntu.com`) | ✅ enabled → tray icon will work |

## 3. Clipboard capability findings (the important part)

1. **Mutter does not implement `wlr-data-control` or `ext-data-control`.** `libmutter-18` exports only the core `wl_data_device` and `zwp_primary_selection_v1` protocols. Background Wayland clients therefore *cannot* read or watch the clipboard on GNOME. Rules out `wl-paste --watch`, `wl-clipboard-rs` monitoring and every "data-control" crate as the main GNOME backend.
2. **The Xwayland bridge works.** With a Wayland app (Chromium-based) owning the clipboard, an X11 client on `:0` received the full target list (`text/html`, `text/plain;charset=utf-8`, `UTF8_STRING`, …) and the payload. Mutter mirrors the Wayland selection into Xwayland, so an X11 client that uses XFixes selection-change events can monitor the clipboard event-driven, with no focus and no polling.
3. **The portal `Clipboard` interface exists, but only inside a `RemoteDesktop` session.** That needs a consent dialog and is meant for remote control, so it isn't suitable.
4. **The portal `GlobalShortcuts` interface is present** (GNOME backend). Tauri's own global-shortcut plugin is X11-only and will not work on this session.
5. **No custom GNOME keybindings are set** (`custom-keybindings = []`), so `Ctrl+Shift+V` is free there. GNOME Terminal uses it for paste inside the terminal, though; this is noted in the setup guide.
6. **Synthetic keystrokes (auto-paste) are blocked on Wayland** for normal apps. Only the compositor (a GNOME Shell extension using a Clutter virtual input device) or a uinput tool (`ydotool`, which needs extra permissions) can send Ctrl+V.

## 4. Architecture decision

```
┌──────────── React + TS + Vite + Tailwind (UI) ────────────┐
│ zustand store · TanStack Virtual list · typed invoke API  │
└───────────────▲──────────────── Tauri 2 commands/events ──┘
                │
┌───────────────┴──────── Rust core (src-tauri) ────────────┐
│ commands.rs  – validated, typed API (no fs/shell to UI)   │
│ capture/     – ClipboardBackend trait + session detection │
│   ├ gnome_ext.rs   D-Bus bridge to shell extension (best) │
│   ├ xfixes.rs      Xwayland/X11 XFixes monitor (fallback) │
│   └ data_control.rs  wlroots/KDE (later, not GNOME)       │
│ ingest.rs    – hash/dedupe, sensitivity filter, limits    │
│ store/       – rusqlite (bundled, FTS5), migrations, repo │
│ media.rs     – content-addressed files + thumbnails       │
│ settings.rs  – validated config (~/.config/clipvault)     │
└───────────────────────────────────────────────────────────┘
        ▲ D-Bus (org.clipvault.ShellBridge), optional
┌───────┴──── gnome-extension/ (separate, optional) ────────┐
│ Meta.Selection 'owner-changed' → read via St.Clipboard     │
│ write-back, Ctrl+Shift+V keybinding, raise window,        │
│ optional virtual-keyboard paste                           │
└───────────────────────────────────────────────────────────┘
```

**Capture backends, chosen at runtime in this order:**
1. **GNOME Shell extension bridge.** Native compositor events. It's also the only clean way to get a reliable global shortcut, focus/raise on Wayland and true auto-paste. Shipped as a separate, optional component.
2. **XFixes on Xwayland/X11.** Works with zero extra installs (verified above). It's the Phase 1 default. To restore an item, ClipVault takes ownership of the X `CLIPBOARD` and serves requests, and Mutter bridges that back to Wayland apps. Limitation: restored content stays available only while ClipVault runs, which is fine because it runs in the background with a tray icon.
3. **data-control** (wl-clipboard-rs), for future KDE/Sway support. Not used on GNOME.

The UI always shows which backend is active, or that none is. ClipVault never claims capture it isn't doing.

**Other decisions**
- **Storage:** `rusqlite` with bundled SQLite and FTS5, WAL mode, versioned migrations, and a dedicated DB worker thread so the UI never blocks. Files go in `~/.local/share/clipvault/{clipvault.db,media/,thumbs/}` (XDG).
- **Dedupe:** SHA-256 of the normalized payload. If the same hash repeats within the configured window, ClipVault bumps `last_used_at` instead of inserting a new row.
- **Images:** the `image` crate with decode limits (max dimensions and bytes). Thumbnails are WebP/PNG at the configured quality. Full-size images load on demand via a scoped asset path.
- **Global shortcut:** extension keybinding, then the `GlobalShortcuts` portal (via `ashpd`), then a GNOME custom shortcut running `clipvault --toggle` (single-instance plugin). The app reports which one is actually registered.
- **Paste:** "Copy to clipboard" (restore) and "Paste" (restore + Ctrl+V via the extension only) are separate actions with separate status.
- **Privacy:** no network access at all (strict Tauri CSP, no HTTP plugin), and no clipboard content in logs. There's a startup privacy mode, pause/resume, and a conservative sensitive-content filter (password-manager MIME hints such as `x-kde-passwordManagerHint`, plus high-entropy token heuristics, each one explainable).

## 5. Phases

| Phase | Deliverable | Exit test |
|---|---|---|
| 1 | Tauri shell, SQLite + migrations, XFixes text/image capture, history list, search, pin, delete, restore, settings, themes | Copy from browser/terminal/GNOME screenshot tool → appears; restart → persists |
| 2 | Thumbnails/preview, files (`text/uri-list`), HTML (sanitized), filters/sort, retention + storage limits, keyboard nav, tray | Checklist items 3, 4, 9, 11, 12 |
| 3 | GNOME extension bridge, shortcut, autostart `.desktop`, panel focus/position, install/uninstall scripts | Shortcut opens panel under Wayland; extension enable/disable is clean |
| 4 | Tags/favorites, code-language detection, optional local OCR (tesseract, opt-in) | — |
| 5 | Export/import, tests, a11y, `.deb` + AppImage, README/DEVELOPMENT/TROUBLESHOOTING/SECURITY | Built artifacts install and run |

## 6. Prerequisites to install (fish shell, needs your sudo password)

```fish
sudo apt update
sudo apt install libwebkit2gtk-4.1-dev librsvg2-dev libayatana-appindicator3-dev libxdo-dev nodejs npm rustup wl-clipboard sqlite3
rustup default stable
```

Verify:

```fish
node --version; npm --version; rustc --version; cargo --version; wl-paste --version
```

Nothing is installed system-wide beyond these standard Ubuntu packages. The Tauri CLI and all JS dependencies are project-local.

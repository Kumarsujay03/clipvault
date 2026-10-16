#!/usr/bin/env bash
# Install ClipVault from the built .deb and set up Super+V, start-at-login and the GNOME
# extension for the current user. Run from fish with:  ./scripts/install.sh
set -euo pipefail

cd "$(dirname "$0")/.."
deb=$(ls -t src-tauri/target/release/bundle/deb/ClipVault_*_amd64.deb 2>/dev/null | head -n1 || true)
if [[ -z "$deb" ]]; then
  echo "No .deb found. Build it first:  npm ci && npm run tauri build" >&2
  exit 1
fi

if [[ $EUID -eq 0 ]]; then
  echo "Run this as your normal user (it asks for sudo only to install the package)." >&2
  exit 1
fi

echo "Installing $deb …"
# Stop a running copy so the new version starts cleanly.
clipvault --quit >/dev/null 2>&1 || true
sudo apt install -y "./$deb"

echo "Setting up ClipVault for $USER …"
clipvault --setup

echo "Starting the capture service …"
nohup clipvault --daemon >/dev/null 2>&1 &
disown || true

cat <<'EOF'

ClipVault is installed.
  • Press Super+V to open your clipboard history.
  • The GNOME extension (panel at the pointer + automatic paste) activates after you
    log out and back in. Until then Super+V still works via a GNOME shortcut.
  • Settings live inside the panel (gear icon).
EOF

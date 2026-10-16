#!/usr/bin/env bash
# Remove ClipVault. Keeps your history unless --purge is given.
#   ./scripts/uninstall.sh            remove app, shortcut, autostart, GNOME extension
#   ./scripts/uninstall.sh --purge    …and delete clipboard history and settings
set -euo pipefail

purge=false
[[ "${1:-}" == "--purge" ]] && purge=true

if command -v clipvault >/dev/null 2>&1; then
  clipvault --remove-integration || true
  clipvault --quit >/dev/null 2>&1 || true
fi

if dpkg -s clip-vault >/dev/null 2>&1; then
  sudo apt remove -y clip-vault
fi

# Belt and braces in case the binary was already gone.
rm -f "${XDG_CONFIG_HOME:-$HOME/.config}/autostart/clipvault.desktop"
rm -rf "${XDG_DATA_HOME:-$HOME/.local/share}/gnome-shell/extensions/clipvault@clipvault.dev"

if $purge; then
  rm -rf "${XDG_DATA_HOME:-$HOME/.local/share}/clipvault" "${XDG_CONFIG_HOME:-$HOME/.config}/clipvault"
  echo "ClipVault removed, including history and settings."
else
  echo "ClipVault removed. History kept in ~/.local/share/clipvault (use --purge to delete it)."
fi
echo "Files you copied were never touched."

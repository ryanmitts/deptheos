#!/bin/sh
# Install Deptheos from this folder: copies bin/ and share/ into $PREFIX
# (default ~/.local), so the app is in the menu and deptheos on the PATH.
#
# Usage: ./install.sh              install, or update an earlier install
#        ./install.sh --uninstall  remove what ./install.sh put there
#
# For every user: sudo PREFIX=/usr/local ./install.sh
set -eu

prefix="${PREFIX:-$HOME/.local}"
here=$(cd "$(dirname "$0")" && pwd)

uninstall=0
case "${1:-}" in
  "") ;;
  --uninstall) uninstall=1 ;;
  *) echo "usage: $0 [--uninstall]" >&2; exit 2 ;;
esac

# The menus and icon caches, refreshed where the tools exist (a desktop
# rescans by itself, but sooner with these).
refresh() {
  if command -v update-desktop-database > /dev/null 2>&1; then
    update-desktop-database -q "$prefix/share/applications" 2> /dev/null || true
  fi
  if command -v gtk-update-icon-cache > /dev/null 2>&1; then
    # -t: a per-user hicolor folder has no index.theme.
    gtk-update-icon-cache -q -t -f "$prefix/share/icons/hicolor" 2> /dev/null || true
  fi
}

# Every file this folder installs, relative to it.
files=$(cd "$here" && find bin share -type f | sort)

if [ "$uninstall" = 1 ]; then
  echo "Removing Deptheos from $prefix"
  for f in $files; do
    rm -f "$prefix/$f"
  done
  rmdir "$prefix/share/doc/deptheos" 2> /dev/null || true
  rmdir "$prefix/share/deptheos/samples/hornet" "$prefix/share/deptheos/samples/pcb" \
    "$prefix/share/deptheos/samples" \
    "$prefix/share/deptheos" 2> /dev/null || true
  refresh
  exit 0
fi

echo "Installing Deptheos into $prefix"
for f in $files; do
  mkdir -p "$prefix/$(dirname "$f")"
  case "$f" in
    bin/*) mode=755 ;;
    *) mode=644 ;;
  esac
  cp "$here/$f" "$prefix/$f.new"
  chmod "$mode" "$prefix/$f.new"
  # Renamed into place, so a running copy keeps its old file.
  mv -f "$prefix/$f.new" "$prefix/$f"
done

# The menu entry runs deptheos-gui by its full path: a desktop session's
# PATH often lacks ~/.local/bin.
desktop="$prefix/share/applications/com.deptheos.Deptheos.desktop"
escaped=$(printf '%s' "$prefix" | sed 's/[&|\\]/\\&/g')
sed "s|^Exec=deptheos-gui |Exec=\"$escaped/bin/deptheos-gui\" |" "$desktop" > "$desktop.new"
mv -f "$desktop.new" "$desktop"

refresh

case ":$PATH:" in
  *":$prefix/bin:"*) ;;
  *)
    echo "Note: $prefix/bin is not on your PATH, so the deptheos command needs"
    echo "its full path ($prefix/bin/deptheos) until you add it. The app runs"
    echo "from the menu either way."
    ;;
esac
echo "Done. Deptheos is in the menu, or run deptheos-gui; ./install.sh --uninstall removes it."

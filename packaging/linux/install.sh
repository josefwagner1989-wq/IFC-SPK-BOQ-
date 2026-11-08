#!/bin/bash
# IFC Sprinkler Viewer – ALPHA 0.1.0 – Created by Josef Wagner
# Instalação por usuário (sem root) em ~/.local
set -e
cd "$(dirname "$0")"
if ! ldconfig -p 2>/dev/null | grep -q libwebkit2gtk-4.1; then
  echo "Benötigt: webkit2gtk-4.1  ->  sudo pacman -S --needed webkit2gtk-4.1"
  exit 1
fi
install -Dm755 ifc-sprinkler-viewer "$HOME/.local/bin/ifc-sprinkler-viewer"
install -Dm644 ifc-sprinkler-viewer.png "$HOME/.local/share/icons/hicolor/256x256/apps/ifc-sprinkler-viewer.png"
install -Dm644 LIESMICH-ALPHA.txt "$HOME/.local/share/ifc-sprinkler-viewer/LIESMICH-ALPHA.txt"
mkdir -p "$HOME/.local/share/applications"
cat > "$HOME/.local/share/applications/ifc-sprinkler-viewer.desktop" <<DESK
[Desktop Entry]
Type=Application
Name=IFC Sprinkler Viewer (Alpha)
Comment=IFC-Viewer und Mengenauszug für Sprinkleranlagen – Created by Josef Wagner
Exec=env WEBKIT_DISABLE_DMABUF_RENDERER=1 $HOME/.local/bin/ifc-sprinkler-viewer %f
Icon=ifc-sprinkler-viewer
Terminal=false
Categories=Office;Engineering;Graphics;
MimeType=application/x-step;model/ifc;application/x-ifc;
DESK
update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
gtk-update-icon-cache "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
echo "Installiert. Start über das Anwendungsmenü ('IFC Sprinkler Viewer') oder: ~/.local/bin/ifc-sprinkler-viewer"

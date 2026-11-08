#!/bin/bash
# Gera todos os pacotes (tarball Linux, MSI Windows, setup Windows) no Linux.
# Requisitos: Rust, mingw-w64, wixl (msitools), nsis, fontes da std do Rust para -Zbuild-std
set -euo pipefail
cd "$(dirname "$0")/.."
VER=0.1.0-alpha
OUT=${OUT:-dist}; mkdir -p "$OUT"; OUT=$(realpath "$OUT")
cargo build --release -p spk-ifc-viewer
RUSTC_BOOTSTRAP=1 cargo build --release -p spk-ifc-viewer --target x86_64-pc-windows-gnu -Zbuild-std=std,panic_abort
# Linux
L=$(mktemp -d); D="$L/IFC-Sprinkler-Viewer-$VER"; mkdir -p "$D"
cp target/release/spk-ifc-viewer "$D/ifc-sprinkler-viewer"; strip "$D/ifc-sprinkler-viewer"
cp src-tauri/icons/128x128@2x.png "$D/ifc-sprinkler-viewer.png"
cp packaging/linux/*.sh packaging/windows/LIESMICH-ALPHA.txt "$D/"
tar -C "$L" -czf "$OUT/IFC-Sprinkler-Viewer_${VER}_linux-x86_64.tar.gz" "IFC-Sprinkler-Viewer-$VER"
# Windows
W=$(mktemp -d); R=target/x86_64-pc-windows-gnu/release
cp $R/spk-ifc-viewer.exe "$W/IFCSprinklerViewer.exe"; x86_64-w64-mingw32-strip "$W/IFCSprinklerViewer.exe"
cp $R/WebView2Loader.dll src-tauri/icons/icon.ico "$W/"
iconv -f utf-8 -t utf-16 packaging/windows/LIESMICH-ALPHA.txt > "$W/LIESMICH-ALPHA.txt"
wixl -a x64 -D SourceDir="$W" -o "$OUT/IFC-Sprinkler-Viewer_${VER}_x64.msi" packaging/windows/IFCSprinklerViewer.wxs
N=$(mktemp -d); cp "$W"/IFCSprinklerViewer.exe "$W"/WebView2Loader.dll "$W"/icon.ico "$N/"
{ printf '\xef\xbb\xbf'; cat packaging/windows/LIESMICH-ALPHA.txt; } > "$N/LIESMICH-ALPHA.txt"
makensis -V1 -DSRC="$N" -DOUT="$OUT/IFC-Sprinkler-Viewer_${VER}_Setup.exe" packaging/windows/installer.nsi
ls -la "$OUT"

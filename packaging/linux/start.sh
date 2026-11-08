#!/bin/bash
# IFC Sprinkler Viewer – ALPHA 0.1.0 – Created by Josef Wagner
# Configurações gráficas estáveis para Wayland/NVIDIA (WebKitGTK)
export WEBKIT_DISABLE_DMABUF_RENDERER=1
# Se a janela ainda ficar vazia:  IFCSV_X11=1 ./start.sh
[ -n "$IFCSV_X11" ] && export GDK_BACKEND=x11 && export WEBKIT_DISABLE_COMPOSITING_MODE=1
exec "$(dirname "$(readlink -f "$0")")/ifc-sprinkler-viewer" "$@" 2> >(grep -v "Gdk-CRITICAL\|GDK_IS_WAYLAND_WINDOW\|^$" >&2)

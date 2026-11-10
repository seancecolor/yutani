#!/usr/bin/env bash
# ==============================================================================
# YUTANI WAYLAND DISPLAY PROFILER — UNINSTALLER FOR NOBARA / FEDORA LINUX
# ==============================================================================

set -euo pipefail

APP_ID="yutani.calibration"

echo "> Removing user-local Yutani files..."
rm -f "${HOME}/.local/bin/yutani"
rm -f "${HOME}/.local/share/applications/${APP_ID}.desktop"
rm -f "${HOME}/.local/share/applications/yutani.desktop"
rm -f "${HOME}/.local/share/icons/hicolor/scalable/apps/yutani.svg"
rm -f "${HOME}/.local/share/icons/hicolor/scalable/apps/${APP_ID}.svg"
rm -f "${HOME}/.local/share/icons/hicolor/256x256/apps/yutani.png"
rm -f "${HOME}/.local/share/icons/hicolor/256x256/apps/${APP_ID}.png"
rm -f "${HOME}/Desktop/${APP_ID}.desktop"

if [[ -f "/usr/local/bin/yutani" ]] || [[ -f "/etc/udev/rules.d/60-yutani-spyder.rules" ]]; then
    echo "> Removing system-wide Yutani files & udev rules (requires sudo)..."
    sudo rm -f "/usr/local/bin/yutani"
    sudo rm -f "/usr/share/applications/${APP_ID}.desktop"
    sudo rm -f "/usr/share/applications/yutani.desktop"
    sudo rm -f "/usr/share/icons/hicolor/scalable/apps/yutani.svg"
    sudo rm -f "/usr/share/icons/hicolor/scalable/apps/${APP_ID}.svg"
    sudo rm -f "/usr/share/icons/hicolor/256x256/apps/yutani.png"
    sudo rm -f "/usr/share/icons/hicolor/256x256/apps/${APP_ID}.png"
    sudo rm -f "/etc/udev/rules.d/60-yutani-spyder.rules"
    if command -v udevadm >/dev/null 2>&1; then
        sudo udevadm control --reload-rules || true
    fi
fi

if command -v kbuildsycoca6 >/dev/null 2>&1; then
    kbuildsycoca6 --noincremental >/dev/null 2>&1 || true
fi

echo "> Yutani uninstalled cleanly. (Generated ICC profiles in ~/.local/share/icc were preserved.)"

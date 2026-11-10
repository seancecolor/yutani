#!/usr/bin/env bash
# ==============================================================================
# YUTANI WAYLAND DISPLAY PROFILER v0.1.5 — NOBARA / FEDORA LINUX INSTALLER
# ==============================================================================
# Builds and installs the Yutani binary, scalable CRT Amber icon, KDE Plasma /
# Wayland .desktop launcher, Desktop shortcut, and Datacolor Spyder USB udev rules.
#
# Usage:
#   ./install-nobara.sh [OPTIONS]
#
# Options:
#   --update                Pull latest git changes, rebuild, and update binary in-place
#   --system                Install binary & launcher system-wide (/usr/local)
#   --no-deps               Skip checking/installing Nobara dnf build packages
#   --no-udev               Skip installing /etc/udev/rules.d/60-yutani-spyder.rules
#   --no-desktop-shortcut   Do not place a clickable launcher icon on ~/Desktop
#   --skip-build            Use existing target/release/yutani without cargo build
#   -h, --help              Show this help message
# ==============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_ID="yutani.calibration"
BIN_NAME="yutani"
VERSION="$(awk -F'"' '/^version *=/ { print $2; exit }' "${SCRIPT_DIR}/Cargo.toml" 2>/dev/null || echo "0.1.5")"

UPDATE_MODE=0
SYSTEM_INSTALL=0
INSTALL_DEPS=1
INSTALL_UDEV=1
DESKTOP_SHORTCUT=1
SKIP_BUILD=0

for arg in "$@"; do
    case "$arg" in
        --update|-u)
            UPDATE_MODE=1
            ;;
        --system)
            SYSTEM_INSTALL=1
            ;;
        --no-deps)
            INSTALL_DEPS=0
            ;;
        --no-udev)
            INSTALL_UDEV=0
            ;;
        --no-desktop-shortcut)
            DESKTOP_SHORTCUT=0
            ;;
        --skip-build)
            SKIP_BUILD=1
            ;;
        -h|--help)
            sed -n '2,21p' "${BASH_SOURCE[0]}"
            exit 0
            ;;
        *)
            echo "[!] Unknown option: $arg" >&2
            exit 1
            ;;
    esac
done

# In --update mode, skip sudo dnf/udev checks if udev rule is already installed
if [[ "$UPDATE_MODE" -eq 1 ]]; then
    INSTALL_DEPS=0
    if [[ -f "/etc/udev/rules.d/60-yutani-spyder.rules" ]]; then
        INSTALL_UDEV=0
    fi
fi

 amber=$'\033[38;2;255;180;40m'
 cyan=$'\033[38;2;90;235;190m'
 reset=$'\033[0m'

echo "${amber}========================================================================${reset}"
echo "${amber}  Y U T A N I   W A Y L A N D   //   NOBARA LINUX INSTALLER (v${VERSION})  ${reset}"
echo "${amber}========================================================================${reset}"

# ------------------------------------------------------------------------------
# 0. Git Pull (when invoked with --update)
# ------------------------------------------------------------------------------
if [[ "$UPDATE_MODE" -eq 1 ]] && [[ -d "${SCRIPT_DIR}/.git" ]]; then
    echo "${amber}> Pulling latest updates from git repository...${reset}"
    git -C "$SCRIPT_DIR" pull --ff-only
    VERSION="$(awk -F'"' '/^version *=/ { print $2; exit }' "${SCRIPT_DIR}/Cargo.toml" 2>/dev/null || echo "$VERSION")"
    echo "${cyan}> Updated source tree to Yutani v${VERSION}.${reset}"
fi

# ------------------------------------------------------------------------------
# 1. Check / Install Nobara & Fedora DNF Build + Runtime Dependencies
# ------------------------------------------------------------------------------
if [[ "$INSTALL_DEPS" -eq 1 ]] && command -v dnf >/dev/null 2>&1; then
    REQUIRED_PKGS=(
        gcc
        pkgconf-pkg-config
        systemd-devel
        libusb1-devel
        hidapi-devel
        lcms2-devel
        wayland-devel
        libxkbcommon-devel
        mesa-libEGL-devel
        colord
    )
    if ! command -v cargo >/dev/null 2>&1; then
        REQUIRED_PKGS+=(cargo rust)
    fi

    MISSING_PKGS=()
    for pkg in "${REQUIRED_PKGS[@]}"; do
        if ! rpm -q "$pkg" >/dev/null 2>&1; then
            MISSING_PKGS+=("$pkg")
        fi
    done

    if [[ "${#MISSING_PKGS[@]}" -gt 0 ]]; then
        echo "${amber}> Installing missing Nobara/Fedora packages via dnf: ${MISSING_PKGS[*]}${reset}"
        sudo dnf install -y "${MISSING_PKGS[@]}"
    else
        echo "${cyan}> All Nobara/Fedora native libraries and toolchains verified.${reset}"
    fi
fi

# ------------------------------------------------------------------------------
# 2. Build Release Binary
# ------------------------------------------------------------------------------
cd "$SCRIPT_DIR"
if [[ "$SKIP_BUILD" -eq 0 ]]; then
    if ! command -v cargo >/dev/null 2>&1; then
        echo "[!] Error: 'cargo' not found in PATH. Install rustup or run with dnf enabled." >&2
        exit 1
    fi
    echo "${amber}> Compiling Yutani v${VERSION} (release profile)...${reset}"
    cargo build --release
fi

RELEASE_BIN="${SCRIPT_DIR}/target/release/${BIN_NAME}"
if [[ ! -f "$RELEASE_BIN" ]]; then
    echo "[!] Error: Release binary not found at ${RELEASE_BIN}" >&2
    exit 1
fi

# ------------------------------------------------------------------------------
# 3. Install Binary, SVG/PNG Icons, and .desktop Application Launcher
# ------------------------------------------------------------------------------
if [[ "$SYSTEM_INSTALL" -eq 1 ]]; then
    BIN_DIR="/usr/local/bin"
    APP_DIR="/usr/share/applications"
    ICON_BASE="/usr/share/icons/hicolor"
    SUDO_CMD="sudo"
else
    BIN_DIR="${HOME}/.local/bin"
    APP_DIR="${HOME}/.local/share/applications"
    ICON_BASE="${HOME}/.local/share/icons/hicolor"
    SUDO_CMD=""
fi

TARGET_BIN="${BIN_DIR}/${BIN_NAME}"
SCALABLE_ICON_DIR="${ICON_BASE}/scalable/apps"
PNG256_ICON_DIR="${ICON_BASE}/256x256/apps"

echo "${amber}> Installing binary to ${TARGET_BIN}...${reset}"
$SUDO_CMD mkdir -p "$BIN_DIR" "$APP_DIR" "$SCALABLE_ICON_DIR"
$SUDO_CMD install -m 0755 "$RELEASE_BIN" "$TARGET_BIN"

echo "${amber}> Installing scalable vector icon to ${SCALABLE_ICON_DIR}...${reset}"
$SUDO_CMD install -m 0644 "${SCRIPT_DIR}/assets/yutani.svg" "${SCALABLE_ICON_DIR}/yutani.svg"
$SUDO_CMD install -m 0644 "${SCRIPT_DIR}/assets/yutani.svg" "${SCALABLE_ICON_DIR}/${APP_ID}.svg"

# Optional raster 256x256 PNG generation if rsvg-convert is available on Nobara
if command -v rsvg-convert >/dev/null 2>&1; then
    $SUDO_CMD mkdir -p "$PNG256_ICON_DIR"
    rsvg-convert -w 256 -h 256 "${SCRIPT_DIR}/assets/yutani.svg" -o "/tmp/yutani-256.png"
    $SUDO_CMD install -m 0644 "/tmp/yutani-256.png" "${PNG256_ICON_DIR}/yutani.png"
    $SUDO_CMD install -m 0644 "/tmp/yutani-256.png" "${PNG256_ICON_DIR}/${APP_ID}.png"
    rm -f "/tmp/yutani-256.png"
fi

# Generate .desktop entry pointing directly to the installed binary path so KDE
# Application Launcher and Desktop icon work even if ~/.local/bin is not in PATH.
TMP_DESKTOP="$(mktemp)"
sed \
    -e "s|^Exec=yutani$|Exec=${TARGET_BIN}|" \
    -e "s|^TryExec=yutani$|TryExec=${TARGET_BIN}|" \
    -e "s|__YUTANI_REPO_DIR__|${SCRIPT_DIR}|g" \
    -e "s|yutani detect|${TARGET_BIN} detect|g" \
    -e "s|yutani probe|${TARGET_BIN} probe|g" \
    "${SCRIPT_DIR}/assets/${APP_ID}.desktop" > "$TMP_DESKTOP"

echo "${amber}> Installing FreeDesktop launcher to ${APP_DIR}/${APP_ID}.desktop...${reset}"
$SUDO_CMD install -m 0644 "$TMP_DESKTOP" "${APP_DIR}/${APP_ID}.desktop"
$SUDO_CMD install -m 0644 "$TMP_DESKTOP" "${APP_DIR}/yutani.desktop"

# Also place a clickable launcher icon on ~/Desktop if requested and directory exists
if [[ "$DESKTOP_SHORTCUT" -eq 1 ]] && [[ -d "${HOME}/Desktop" ]]; then
    DESKTOP_FILE="${HOME}/Desktop/${APP_ID}.desktop"
    echo "${amber}> Creating clickable Desktop launcher at ${DESKTOP_FILE}...${reset}"
    install -m 0755 "$TMP_DESKTOP" "$DESKTOP_FILE"
    if command -v gio >/dev/null 2>&1; then
        gio set "$DESKTOP_FILE" metadata::trusted true >/dev/null 2>&1 || true
    fi
fi
rm -f "$TMP_DESKTOP"

# Ensure ~/.local/share/icc exists for one-click profile installation
mkdir -p "${HOME}/.local/share/icc"

# ------------------------------------------------------------------------------
# 4. Install Datacolor SpyderX / SpyderX2 / SpyderPro 2024 USB udev Rules
# ------------------------------------------------------------------------------
if [[ "$INSTALL_UDEV" -eq 1 ]]; then
    UDEV_DEST="/etc/udev/rules.d/60-yutani-spyder.rules"
    echo "${amber}> Installing USB udev rules for SpyderX2 (085c:0a0a) & SpyderPro (085c:0a0b) to ${UDEV_DEST}...${reset}"
    sudo install -m 0644 "${SCRIPT_DIR}/assets/60-yutani-spyder.rules" "$UDEV_DEST"
    if command -v udevadm >/dev/null 2>&1; then
        sudo udevadm control --reload-rules
        sudo udevadm trigger --subsystem-match=usb --attr-match=idVendor=085c || true
    fi
fi

# ------------------------------------------------------------------------------
# 5. Refresh KDE Plasma 6 / FreeDesktop Icon & Application Caches
# ------------------------------------------------------------------------------
if command -v update-desktop-database >/dev/null 2>&1; then
    $SUDO_CMD update-desktop-database "$APP_DIR" >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    $SUDO_CMD gtk-update-icon-cache -f -t "$ICON_BASE" >/dev/null 2>&1 || true
fi
if command -v kbuildsycoca6 >/dev/null 2>&1; then
    kbuildsycoca6 --noincremental >/dev/null 2>&1 || true
elif command -v kbuildsycoca5 >/dev/null 2>&1; then
    kbuildsycoca5 --noincremental >/dev/null 2>&1 || true
fi

echo ""
echo "${cyan}========================================================================${reset}"
echo "${cyan}  YUTANI v${VERSION} INSTALLATION COMPLETE                              ${reset}"
echo "${cyan}========================================================================${reset}"
echo "  Binary   : ${TARGET_BIN}"
echo "  Launcher : ${APP_DIR}/${APP_ID}.desktop"
echo "  Icon     : ${SCALABLE_ICON_DIR}/yutani.svg"
if [[ "$DESKTOP_SHORTCUT" -eq 1 ]] && [[ -d "${HOME}/Desktop" ]]; then
    echo "  Desktop  : ${HOME}/Desktop/${APP_ID}.desktop"
fi
if [[ "$INSTALL_UDEV" -eq 1 ]]; then
    echo "  USB Rule : /etc/udev/rules.d/60-yutani-spyder.rules (reloaded)"
fi
echo ""
echo "You can now launch 'Yutani Display Profiler' directly from your Nobara"
echo "Application Launcher, KRunner (Alt+Space), or Desktop icon!"

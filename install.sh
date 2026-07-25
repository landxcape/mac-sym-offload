#!/usr/bin/env bash
set -e

# mso: macOS Developer Storage Migrator Installation Script

REPO="landxcape/mac-sym-offload"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"

echo "=================================================="
echo "  mso: macOS Developer Storage Migrator Installer"
echo "=================================================="
echo ""

# Detect OS
if [ "$(uname -s)" != "Darwin" ]; then
  echo "❌ Error: mso is specifically designed for macOS."
  exit 1
fi

# Detect Architecture
ARCH="$(uname -m)"
case "$ARCH" in
  arm64|aarch64)
    ASSET_NAME="mso-macos-arm64.tar.gz"
    ;;
  x86_64)
    ASSET_NAME="mso-macos-x86_64.tar.gz"
    ;;
  *)
    echo "❌ Error: Unsupported architecture '$ARCH'."
    exit 1
    ;;
esac

DOWNLOAD_URL="https://github.com/${REPO}/releases/latest/download/${ASSET_NAME}"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

echo "📥 Downloading pre-compiled binary ($ASSET_NAME)..."
if ! curl -fsSL "$DOWNLOAD_URL" -o "$TMP_DIR/$ASSET_NAME"; then
  echo "❌ Error: Failed to download release asset from GitHub."
  echo "   Please check your network connection or download manually from:"
  echo "   https://github.com/${REPO}/releases"
  exit 1
fi

echo "📦 Extracting release archive..."
tar -xzf "$TMP_DIR/$ASSET_NAME" -C "$TMP_DIR"

if [ ! -f "$TMP_DIR/mso" ]; then
  echo "❌ Error: Binary 'mso' not found in release archive."
  exit 1
fi

echo "⚙️ Installing mso binary to $INSTALL_DIR..."
if [ -w "$INSTALL_DIR" ]; then
  mv "$TMP_DIR/mso" "$INSTALL_DIR/mso"
else
  echo "🔐 Requesting administrator privileges (sudo) to install to $INSTALL_DIR..."
  sudo mv "$TMP_DIR/mso" "$INSTALL_DIR/mso"
fi

chmod +x "$INSTALL_DIR/mso"

echo ""
echo "🎉 Installation complete!"
echo "   Run 'mso --version' or 'mso' to start offloading caches."

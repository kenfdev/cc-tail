#!/bin/sh
set -e

REPO="kenfdev/cc-tail"
BINARY="cctail"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"

# Detect architecture
ARCH=$(uname -m)
case "$ARCH" in
  x86_64)  ARCH="x86_64" ;;
  aarch64) ARCH="aarch64" ;;
  arm64)   ARCH="aarch64" ;;
  *) echo "Unsupported architecture: $ARCH" && exit 1 ;;
esac

# Detect OS
OS=$(uname -s)
case "$OS" in
  Linux)  TARGET="${ARCH}-unknown-linux-musl" ;;
  Darwin) TARGET="${ARCH}-apple-darwin" ;;
  *) echo "Unsupported OS: $OS" && exit 1 ;;
esac

URL="https://github.com/${REPO}/releases/latest/download/${BINARY}-${TARGET}"

echo "Installing ${BINARY} (${TARGET})..."
curl -fsSL "$URL" -o "${BINARY}"
chmod +x "${BINARY}"

if [ -w "$INSTALL_DIR" ]; then
  mv "${BINARY}" "${INSTALL_DIR}/${BINARY}"
else
  echo "Moving to ${INSTALL_DIR} (requires sudo)..."
  sudo mv "${BINARY}" "${INSTALL_DIR}/${BINARY}"
fi

echo "Installed ${BINARY} to ${INSTALL_DIR}/${BINARY}"

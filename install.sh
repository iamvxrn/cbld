#!/usr/bin/env sh
set -e

REPO="iamvxrn/cbld"
BINARY="cbld"

OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "$ARCH" in
    x86_64|amd64)
        ARCH="x86_64"
        ;;
    aarch64|arm64)
        ARCH="aarch64"
        ;;
    *)
        echo "Unsupported architecture: $ARCH"
        exit 1
        ;;
esac

case "$OS" in
    linux)
        TARGET="${ARCH}-unknown-linux-gnu"
        EXT="tar.gz"
        ;;
    darwin)
        TARGET="${ARCH}-apple-darwin"
        EXT="tar.gz"
        ;;
    *)
        echo "Unsupported operating system: $OS"
        exit 1
        ;;
esac

TAG=$(curl -s "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/')
if [ -z "$TAG" ]; then
    TAG="v0.4.0"
fi

URL="https://github.com/${REPO}/releases/download/${TAG}/${BINARY}-${TAG}-${TARGET}.${EXT}"
INSTALL_DIR="${HOME}/.local/bin"

echo "Downloading ${BINARY} ${TAG} for ${TARGET}..."
TMP_DIR=$(mktemp -d)
curl -sSL "$URL" -o "${TMP_DIR}/${BINARY}.${EXT}"

tar -xzf "${TMP_DIR}/${BINARY}.${EXT}" -C "$TMP_DIR"
mkdir -p "$INSTALL_DIR"
mv "${TMP_DIR}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
chmod +x "${INSTALL_DIR}/${BINARY}"
rm -rf "$TMP_DIR"

echo "${BINARY} successfully installed to ${INSTALL_DIR}/${BINARY}"
echo "Make sure ${INSTALL_DIR} is in your PATH."

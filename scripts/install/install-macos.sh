#!/usr/bin/env sh
set -eu

REPO="${ADO_AW_REPO:-githubnext/ado-aw}"
VERSION="${ADO_AW_VERSION:-latest}"
ASSET="ado-aw-darwin-arm64"

require() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

require curl
require grep
require shasum

if [ "$(uname -s)" != "Darwin" ]; then
  echo "This installer is for macOS only." >&2
  exit 1
fi

case "$(uname -m)" in
  arm64|aarch64) ;;
  *)
    echo "Unsupported macOS architecture: $(uname -m). Expected arm64." >&2
    exit 1
    ;;
esac

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT INT TERM

if [ "$VERSION" = "latest" ]; then
  DOWNLOAD_BASE="https://github.com/$REPO/releases/latest/download"
else
  DOWNLOAD_BASE="https://github.com/$REPO/releases/download/$VERSION"
fi

BIN_PATH="$TMP_DIR/$ASSET"
CHECKSUMS_PATH="$TMP_DIR/checksums.txt"
CHECKSUM_LINE_PATH="$TMP_DIR/checksum.line"

curl -fsSL "$DOWNLOAD_BASE/$ASSET" -o "$BIN_PATH"
curl -fsSL "$DOWNLOAD_BASE/checksums.txt" -o "$CHECKSUMS_PATH"

grep "  $ASSET\$" "$CHECKSUMS_PATH" > "$CHECKSUM_LINE_PATH" || {
  echo "Unable to find checksum entry for $ASSET." >&2
  exit 1
}

(cd "$TMP_DIR" && shasum -a 256 -c checksum.line)

INSTALL_DIR="/usr/local/bin"
if [ ! -w "$INSTALL_DIR" ]; then
  INSTALL_DIR="$HOME/.local/bin"
  mkdir -p "$INSTALL_DIR"
fi

INSTALL_PATH="$INSTALL_DIR/ado-aw"
cp "$BIN_PATH" "$INSTALL_PATH"
chmod 0755 "$INSTALL_PATH"

if ! printf ':%s:' "$PATH" | grep -q ":$INSTALL_DIR:"; then
  PROFILE="$HOME/.zshrc"
  case "${SHELL:-}" in
    */bash) PROFILE="$HOME/.bash_profile" ;;
    */zsh) PROFILE="$HOME/.zshrc" ;;
  esac

  if [ -f "$PROFILE" ] && grep -Fq "export PATH=\"$INSTALL_DIR:\$PATH\"" "$PROFILE"; then
    :
  else
    printf '\nexport PATH="%s:$PATH"\n' "$INSTALL_DIR" >> "$PROFILE"
  fi

  export PATH="$INSTALL_DIR:$PATH"
  echo "Added $INSTALL_DIR to PATH via $PROFILE."
fi

echo "Installed ado-aw to $INSTALL_PATH"
echo "Run: ado-aw --version"

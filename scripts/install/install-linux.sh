#!/usr/bin/env sh
# Wrapped in `main` so a truncated `curl ... | sh` cannot execute a partial
# installer: shell parses the entire script before calling `main` on the last
# line, so an incomplete download fails to parse and nothing runs.
set -eu

main() {
  REPO="${ADO_AW_REPO:-githubnext/ado-aw}"
  VERSION="${ADO_AW_VERSION:-latest}"
  ASSET="ado-aw-linux-x64"

  require curl
  require grep
  require sha256sum

  if [ "$(uname -s)" != "Linux" ]; then
    echo "This installer is for Linux only." >&2
    exit 1
  fi

  if [ "$(uname -m)" != "x86_64" ]; then
    echo "Unsupported Linux architecture: $(uname -m). Expected x86_64." >&2
    exit 1
  fi

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

  (cd "$TMP_DIR" && sha256sum -c checksum.line)

  INSTALL_DIR="/usr/local/bin"
  if [ ! -w "$INSTALL_DIR" ]; then
    INSTALL_DIR="$HOME/.local/bin"
    mkdir -p "$INSTALL_DIR"
  fi

  INSTALL_PATH="$INSTALL_DIR/ado-aw"
  cp "$BIN_PATH" "$INSTALL_PATH"
  chmod 0755 "$INSTALL_PATH"

  case ":$PATH:" in
    *":$INSTALL_DIR:"*)
      ;;
    *)
    PROFILE="$HOME/.bashrc"
    case "${SHELL:-}" in
      */zsh) PROFILE="$HOME/.zshrc" ;;
      */bash) PROFILE="$HOME/.bashrc" ;;
    esac

    if [ -f "$PROFILE" ] && grep -Fq "export PATH=\"$INSTALL_DIR:\$PATH\"" "$PROFILE"; then
      :
    else
      printf '\nexport PATH="%s:$PATH"\n' "$INSTALL_DIR" >> "$PROFILE"
    fi

    export PATH="$INSTALL_DIR:$PATH"
    echo "Added $INSTALL_DIR to PATH via $PROFILE."
    ;;
  esac

  echo "Installed ado-aw to $INSTALL_PATH"
  echo "Run: ado-aw --version"
}

require() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

main "$@"

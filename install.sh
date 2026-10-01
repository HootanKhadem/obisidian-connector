#!/bin/sh
# Installs obsidian-connector on macOS or Linux.
#   curl -fsSL https://raw.githubusercontent.com/HootanKhadem/obisidian-connector/main/install.sh | sh
# Set INSTALL_DIR to choose where the binary goes (default: ~/.local/bin).
set -eu

REPO="HootanKhadem/obisidian-connector"
BIN="obsidian-connector"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"

case "$(uname -s)" in
  Linux) os="unknown-linux-musl" ;;
  Darwin) os="apple-darwin" ;;
  *) echo "Unsupported OS: $(uname -s). On Windows use install.ps1." >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  arm64 | aarch64) arch="aarch64" ;;
  *) echo "Unsupported CPU: $(uname -m)" >&2; exit 1 ;;
esac
target="$arch-$os"
url="https://github.com/$REPO/releases/latest/download/$BIN-$target.tar.gz"

mkdir -p "$INSTALL_DIR"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $BIN for $target..."
if curl -fsSL "$url" -o "$tmp/$BIN.tar.gz" && tar xzf "$tmp/$BIN.tar.gz" -C "$tmp"; then
  install -m 755 "$tmp/$BIN" "$INSTALL_DIR/$BIN"
elif command -v cargo >/dev/null 2>&1; then
  echo "No prebuilt binary available; building from source with cargo..."
  cargo install --git "https://github.com/$REPO" --root "$tmp/cargo" --locked "$BIN"
  install -m 755 "$tmp/cargo/bin/$BIN" "$INSTALL_DIR/$BIN"
else
  echo "Could not download $url and cargo is not installed (https://rustup.rs)." >&2
  exit 1
fi

echo "Installed $INSTALL_DIR/$BIN"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) echo "Note: add $INSTALL_DIR to your PATH." ;;
esac
cat <<EOF

Next, connect it to your agent, for example:
  $BIN setup claude-desktop --vault "My Vault"
  $BIN setup claude-code --vault ~/Documents/MyVault
Run '$BIN --help' for everything else.
EOF

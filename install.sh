#!/bin/sh
# Installs the latest panic release: curl -fsSL https://raw.githubusercontent.com/toastx/panic-at-the-program/master/install.sh | sh
set -eu

repo=toastx/panic-at-the-program
dir=${PANIC_INSTALL_DIR:-$HOME/.local/bin}

case "$(uname -s)-$(uname -m)" in
    Darwin-arm64) target=aarch64-apple-darwin ;;
    Darwin-x86_64) target=x86_64-apple-darwin ;;
    Linux-x86_64) target=x86_64-unknown-linux-gnu ;;
    *) echo "no prebuilt panic for $(uname -s)-$(uname -m); use: cargo install --git https://github.com/$repo" >&2; exit 1 ;;
esac

tag=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$repo/releases/latest")
tag=${tag##*/}
asset="panic-${tag#v}-$target.tar.gz"
url="https://github.com/$repo/releases/download/$tag/$asset"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cd "$tmp"
curl -fsSLO "$url"
curl -fsSLO "$url.sha256"
if command -v sha256sum >/dev/null; then sha256sum -c "$asset.sha256" >/dev/null; else shasum -a 256 -c "$asset.sha256" >/dev/null; fi
tar xzf "$asset"

mkdir -p "$dir"
mv panic "$dir/panic"
chmod +x "$dir/panic"
echo "installed panic $tag to $dir/panic"
case ":$PATH:" in *":$dir:"*) ;; *) echo "add $dir to your PATH" ;; esac

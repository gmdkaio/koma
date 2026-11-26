#!/usr/bin/env bash
# builds dist/koma-linux-x86_64.tar.gz (prebuilt binary + install.sh).
# needs rust plus the gtk4, libadwaita, sqlite3 and libwebp dev packages.
# bakes in the desktop oauth client from KOMA_GDRIVE_CLIENT_ID/SECRET, or from the given json.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/scripts/oauth-env.sh" "${1:-${XDG_CONFIG_HOME:-$HOME/.config}/koma/gdrive_client.json}"

cargo build --release --manifest-path "$repo_root/Cargo.toml"

name=koma-linux-x86_64
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
out="$stage/$name"

install -Dm755 "$repo_root/target/release/koma" "$out/target/release/koma"
install -Dm755 "$repo_root/scripts/install.sh" "$out/scripts/install.sh"
install -Dm644 -t "$out/assets" \
    "$repo_root/assets/icon.svg" "$repo_root/assets/icon.png" "$repo_root/assets/dev.koma.App.desktop"
cat > "$out/install.sh" <<'EOF'
#!/usr/bin/env bash
exec "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/scripts/install.sh" --no-build
EOF
chmod +x "$out/install.sh"

mkdir -p "$repo_root/dist"
tar czf "$repo_root/dist/$name.tar.gz" -C "$stage" "$name"
echo "tarball: $repo_root/dist/$name.tar.gz"

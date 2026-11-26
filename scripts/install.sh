#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
data_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
apps_dir="$data_dir/applications"
icons_dir="$data_dir/icons/hicolor"

if [[ "${1:-}" != "--no-build" ]]; then
    cargo build --release --manifest-path "$repo_root/Cargo.toml"
fi

install -Dm755 "$repo_root/target/release/koma" "$bin_dir/koma"
install -Dm644 "$repo_root/assets/icon.svg" "$icons_dir/scalable/apps/dev.koma.App.svg"
install -Dm644 "$repo_root/assets/icon.png" "$icons_dir/512x512/apps/dev.koma.App.png"

mkdir -p "$apps_dir"
sed -e "s|^Exec=koma$|Exec=$bin_dir/koma|" \
    -e "s|^Icon=dev.koma.App$|Icon=$icons_dir/512x512/apps/dev.koma.App.png|" \
    "$repo_root/assets/dev.koma.App.desktop" \
    > "$apps_dir/dev.koma.App.desktop"

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$apps_dir" >/dev/null 2>&1 || true
fi
if command -v gtk4-update-icon-cache >/dev/null 2>&1; then
    gtk4-update-icon-cache -f -t "$icons_dir" >/dev/null 2>&1 || true
elif command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$icons_dir" >/dev/null 2>&1 || true
fi

echo "Installed koma to $bin_dir/koma"
echo "Desktop entry: $apps_dir/dev.koma.App.desktop"

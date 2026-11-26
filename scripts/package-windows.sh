#!/usr/bin/env bash
# builds dist/koma (exe + gtk runtime) and dist/koma-setup.exe.
# needs msys2 at C:\msys64 with the ucrt64 gtk4, libadwaita, libwebp and gcc packages,
# rust's x86_64-pc-windows-gnu target, and inno setup 6 for the installer.
# bakes in the desktop oauth client from KOMA_GDRIVE_CLIENT_ID/SECRET, or from the imported json.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/scripts/oauth-env.sh" "$APPDATA/koma/gdrive_client.json"
msys="${MSYS2_ROOT:-/c/msys64}"
ucrt="$msys/ucrt64"
target=x86_64-pc-windows-gnu
dist="$repo_root/dist"
out="$dist/koma"

export PATH="$ucrt/bin:$PATH"
export PKG_CONFIG="$ucrt/bin/pkg-config.exe"
export PKG_CONFIG_PATH="$ucrt/lib/pkgconfig"

if [[ "${1:-}" != "--no-build" ]]; then
    cargo build --release --target "$target" --manifest-path "$repo_root/Cargo.toml"
fi

rm -rf "$out"
mkdir -p "$out/bin" "$out/lib" "$out/share/icons" "$out/share/glib-2.0/schemas"

cp "$repo_root/target/$target/release/koma.exe" "$out/bin/"
cp -r "$ucrt/lib/gdk-pixbuf-2.0" "$out/lib/"
find "$out/lib" -name '*.a' -delete

# copy every ucrt64 dll the exe and the pixbuf loaders link against.
loaders=("$out"/lib/gdk-pixbuf-2.0/2.10.0/loaders/*.dll)
"$msys/usr/bin/ldd" "$out/bin/koma.exe" "${loaders[@]}" \
    | awk '{print $3}' | grep -i '^/ucrt64/' | sort -u \
    | while read -r dll; do cp -n "$msys$dll" "$out/bin/"; done

cp -r "$ucrt/share/icons/Adwaita" "$out/share/icons/"
cp -r "$ucrt/share/icons/hicolor" "$out/share/icons/"
install -Dm644 "$repo_root/assets/icon.png" "$out/share/icons/hicolor/512x512/apps/dev.koma.App.png"
"$ucrt/bin/gtk4-update-icon-cache.exe" -f -t "$out/share/icons/hicolor" >/dev/null
cp "$ucrt/share/glib-2.0/schemas/gschemas.compiled" "$out/share/glib-2.0/schemas/"

iscc="/c/Program Files (x86)/Inno Setup 6/ISCC.exe"
[[ -x "$iscc" ]] || iscc="$LOCALAPPDATA/Programs/Inno Setup 6/ISCC.exe"
if [[ -x "$iscc" ]]; then
    version="$(grep -m1 '^version' "$repo_root/Cargo.toml" | cut -d'"' -f2)"
    "$iscc" //Q "//DAppVersion=$version" "$(cygpath -w "$repo_root/scripts/koma.iss")"
    echo "installer: $dist/koma-setup.exe"
else
    echo "inno setup not found, skipped the installer. portable build: $out"
fi

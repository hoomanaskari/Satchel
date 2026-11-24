#!/bin/sh
# Satchel Dev's icon is Satchel's with a blue DEV band, so the two never look alike in Finder,
# Launchpad, System Settings or the Dock. This redraws src-tauri/icons/dev/ from
# src-tauri/icons/Satchel.icon: the Icon Composer file (tauri.dev.conf.json lists it, and Tauri
# compiles it into Satchel Dev's Assets.car, the icon macOS 26 and later show), and icon.icns
# with the PNGs Tauri wants (older macOS).
# Run it after changing Satchel.icon, and commit what it writes. Needs Xcode (Icon Composer).
set -e
[ "$(uname)" = Darwin ] || exit 0

root=$(cd "$(dirname "$0")/.." && pwd)
icons="$root/src-tauri/icons"
dev="$icons/dev"
ictool="$(xcode-select -p)/../Applications/Icon Composer.app/Contents/Executables/ictool"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' 0

mkdir -p "$dev"
rm -rf "$dev/Satchel.icon"
cp -R "$icons/Satchel.icon" "$dev/Satchel.icon"
for picture in "$icons"/Satchel.icon/Assets/*.png; do
  swift "$root/scripts/dev-icon.swift" "$picture" "$dev/Satchel.icon/Assets/$(basename "$picture")"
done

# icon.icns and the PNGs, from an 824 px tile in 1024 px, as macOS draws an app icon.
"$ictool" "$dev/Satchel.icon" --export-image --output-file "$tmp/tile.png" --platform macOS \
  --rendition Default --width 824 --height 824 --scale 1 >/dev/null
sips --padToHeightWidth 1024 1024 "$tmp/tile.png" --out "$tmp/1024.png" >/dev/null
mkdir "$tmp/icon.iconset"
for size in 16 32 128 256 512; do
  sips -z $size $size "$tmp/1024.png" --out "$tmp/icon.iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z $double $double "$tmp/1024.png" --out "$tmp/icon.iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$tmp/icon.iconset" -o "$dev/icon.icns"
sips -z 32 32 "$tmp/1024.png" --out "$dev/32x32.png" >/dev/null
sips -z 128 128 "$tmp/1024.png" --out "$dev/128x128.png" >/dev/null
sips -z 256 256 "$tmp/1024.png" --out "$dev/128x128@2x.png" >/dev/null
echo "Drew Satchel Dev's icon in $dev"

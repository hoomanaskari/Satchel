#!/bin/sh
# Rehearses an update end to end on this Mac, without publishing anything. See "Rehearsing an
# update" in docs/BUILDING.md.
#
#   sh scripts/update-rehearsal.sh [port]      (default port 8975)
#
# Builds "Satchel Rehearsal" twice, as 0.0.1 and 0.0.2: a separate app
# (app.satchel.desktop.rehearsal, its own data folder, ⌥⇧⌘V) with the installed Satchel's
# updater and public key, but whose update feed is http://127.0.0.1:<port>/latest.json. Then it
# makes the 0.0.2 update the way `pnpm release` does (archive, signature with the update key,
# latest.json) and a copy with a changed archive, into target/update-rehearsal/:
#
#   run/Satchel Rehearsal.app   0.0.1, the copy to open (the update replaces it in place)
#   feed/                       latest.json and the signed 0.0.2 archive: serve this folder
#   tampered/                   the same, with one byte of the archive changed
#
# The local feed is build-time configuration (`tauri build --config`), compiled into these two
# binaries only. Satchel reads its feed from the config embedded in its own binary; nothing at
# run time (no environment variable, file or flag) changes it, and `pnpm release` refuses a
# config whose feed is not GitHub's or that allows plain HTTP. The rehearsal still has to pass
# the real signature check: its archive is signed with Satchel's update key.
set -e
cd "$(dirname "$0")/.."

PORT="${1:-8975}"
FROM=0.0.1
TO=0.0.2
NAME="Satchel Rehearsal"
ID=app.satchel.desktop.rehearsal
KEY="${SATCHEL_UPDATER_KEY:-$HOME/.tauri/satchel-updater.key}"
OUT="$PWD/target/update-rehearsal"
export CARGO_TARGET_DIR="$PWD/target/rehearsal"
BUILT="$CARGO_TARGET_DIR/release/bundle/macos/$NAME.app"
ARCHIVE="Satchel_${TO}_aarch64.app.tar.gz"

stop() {
  echo "Stopped: $1" >&2
  exit 1
}
[ -f "$KEY" ] || stop "no update signing key at $KEY. See \"Updates\" in docs/BUILDING.md."
pgrep -f "$NAME.app/Contents/MacOS" >/dev/null && stop "quit $NAME first."

config() {
  printf '{"version":"%s","productName":"%s","identifier":"%s","plugins":{"updater":{"endpoints":["http://127.0.0.1:%s/latest.json"],"dangerousInsecureTransportProtocol":true}}}' \
    "$1" "$NAME" "$ID" "$PORT"
}

. scripts/signing-identity.sh
[ -n "$APPLE_SIGNING_IDENTITY" ] && export APPLE_SIGNING_IDENTITY

rm -rf "$OUT"
mkdir -p "$OUT/run" "$OUT/feed" "$OUT/tampered" "$OUT/build"
for version in "$FROM" "$TO"; do
  echo "Building $NAME $version"
  rm -rf "$BUILT"
  pnpm tauri build --bundles app --config "$(config "$version")" -- --locked
  [ "$(plutil -extract CFBundleShortVersionString raw -o - "$BUILT/Contents/Info.plist")" = "$version" ] \
    || stop "the build is not version $version."
  mkdir -p "$OUT/build/$version"
  ditto "$BUILT" "$OUT/build/$version/$NAME.app"
done
ditto "$OUT/build/$FROM/$NAME.app" "$OUT/run/$NAME.app"

echo "Making the $TO update"
COPYFILE_DISABLE=1 tar -czf "$OUT/feed/$ARCHIVE" -C "$OUT/build/$TO" "$NAME.app"
TAURI_SIGNING_PRIVATE_KEY_PASSWORD="" pnpm -s tauri signer sign --app-version "$TO" -f "$KEY" "$OUT/feed/$ARCHIVE" >/dev/null
printf 'A rehearsal update. Nothing is published.\n' > "$OUT/notes.md"
node scripts/update-feed.ts --version "$TO" --archive "$OUT/feed/$ARCHIVE" --notes "$OUT/notes.md" \
  --url "http://127.0.0.1:$PORT/$ARCHIVE" --out "$OUT/feed/latest.json"

# The same feed, with the archive's last byte changed: the signature no longer matches.
cp "$OUT/feed/latest.json" "$OUT/feed/$ARCHIVE.sig" "$OUT/tampered/"
node -e '
  const fs = require("fs");
  const bytes = fs.readFileSync(process.argv[1]);
  bytes[bytes.length - 1] ^= 0xff;
  fs.writeFileSync(process.argv[2], bytes);
' "$OUT/feed/$ARCHIVE" "$OUT/tampered/$ARCHIVE"

cat <<EOF

Ready in $OUT.
1. Serve the update:   python3 -m http.server $PORT --bind 127.0.0.1 --directory "$OUT/feed"
   (or "$OUT/tampered" to see a changed archive refused)
2. Open $FROM:          open "$OUT/run/$NAME.app"
   Its menu bar icon gets a dot and its menu "Update to $NAME $TO…" within seconds.
3. Choose the update; $NAME reopens as $TO. "Check for Updates…" then says it is up to date.
4. Afterwards: quit $NAME, stop the server, then
   rm -rf "$OUT" "$HOME/Library/Application Support/$ID" "$HOME/Library/Caches/$ID" "$HOME/Library/WebKit/$ID"
EOF

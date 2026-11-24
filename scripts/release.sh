#!/bin/sh
# Makes the installed Satchel for people to download: target/satchel-release/<version>/
#   Satchel_<version>_<arch>.dmg          signed with the Developer ID, notarized and stapled
#   Satchel_<version>_<arch>.app.tar.gz   the update: the same notarized, stapled Satchel.app
#   Satchel_<version>_<arch>.app.tar.gz.sig  its signature, made with the update key
#   latest.json                           the update feed every installed Satchel reads
#   SHA256SUMS.txt                        checksums of the DMG, the update and the feed
# Satchel.app inside is signed with the Developer ID and the hardened runtime, notarized and
# stapled. The update archive is signed with the update key (SATCHEL_UPDATER_KEY, default
# ~/.tauri/satchel-updater.key), never kept in this repository. See docs/BUILDING.md.
#
#   pnpm release           the real thing: needs a clean main that matches origin/main, a version
#                          with no tag yet, and the notary login saved in the Keychain
#                          (SATCHEL_NOTARY_PROFILE, default "satchel-notary").
#   pnpm release --local   a rehearsal from the working tree: everything except notarization and
#                          stapling, into target/satchel-release/local/<version>/. Gatekeeper
#                          refuses its DMG on other Macs.
#
# Only this script makes the installed Satchel (app.satchel.desktop), from tauri.conf.json as it
# is, never with tauri.dev.conf.json. It builds in its own Cargo target folder,
# target/production, so it never replaces a Satchel Dev build or a Satchel that is running.
set -e
cd "$(dirname "$0")/.."

local=false
case "$1" in
  --local) local=true ;;
  "") ;;
  *) echo "Use: pnpm release [--local]" >&2; exit 1 ;;
esac

stop() {
  echo "Stopped: $1" >&2
  exit 1
}

[ "$(uname)" = Darwin ] || stop "Satchel releases are made on a Mac."
VERSION=$(node -e 'const v = require("./src-tauri/tauri.conf.json").version; if (typeof v !== "string" || !/^\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$/.test(v)) throw Error("Invalid release version"); console.log(v)')
TAG="v$VERSION"
ARCH=$(uname -m)
[ "$ARCH" = arm64 ] && ARCH=aarch64
PROFILE="${SATCHEL_NOTARY_PROFILE:-satchel-notary}"
KEY="${SATCHEL_UPDATER_KEY:-$HOME/.tauri/satchel-updater.key}"
NOTES="docs/release-notes/$VERSION.md"
export CARGO_TARGET_DIR="$PWD/target/production"
APP="$CARGO_TARGET_DIR/release/bundle/macos/Satchel.app"
DMG="Satchel_${VERSION}_${ARCH}.dmg"
ARCHIVE="Satchel_${VERSION}_${ARCH}.app.tar.gz"
if $local; then OUT="target/satchel-release/local/$VERSION"; else OUT="target/satchel-release/$VERSION"; fi

# Everything that can stop a release is checked before the long build.
node --input-type=module -e '
  import { readFileSync } from "node:fs";
  import { assertProduction } from "./scripts/lib/identity.ts";
  assertProduction(JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8")));
' || stop "src-tauri/tauri.conf.json is not the installed Satchel's."
[ -f "$KEY" ] || stop "no update signing key at $KEY. See \"Updates\" in docs/BUILDING.md."
[ -f "$NOTES" ] || stop "write what's new in $NOTES first (it goes on the GitHub release and into the update)."
command -v create-dmg >/dev/null || stop "create-dmg is missing. Install it with: brew install create-dmg"
cargo about --version >/dev/null 2>&1 || stop "cargo-about is missing. See \"Third-party notices\" in docs/BUILDING.md."
. scripts/signing-identity.sh
[ -n "$APPLE_SIGNING_IDENTITY" ] || stop "no Developer ID Application certificate on this Mac."
export APPLE_SIGNING_IDENTITY
if ! $local; then
  [ -z "$(git status --porcelain)" ] || stop "there are uncommitted changes."
  [ "$(git branch --show-current)" = main ] || stop "releases are made from main."
  git fetch -q origin main
  [ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || stop "local main differs from origin/main."
  [ -z "$(git tag --list "$TAG")" ] && [ -z "$(git ls-remote --tags origin "refs/tags/$TAG")" ] \
    || stop "$TAG already exists. Set a new version with: pnpm set-version <version>"
  xcrun notarytool history --keychain-profile "$PROFILE" >/dev/null 2>&1 \
    || stop "no notary login saved as \"$PROFILE\". See docs/BUILDING.md."
  [ ! -e "$OUT" ] || stop "$OUT already exists."
fi

echo "Checking Satchel $VERSION"
pnpm install --frozen-lockfile
# THIRD-PARTY-NOTICES.txt ships inside Satchel.app: it must match Cargo.lock and the web bundle.
pnpm notices --check
pnpm check
pnpm test
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings

echo "Building Satchel $VERSION, signed with $APPLE_SIGNING_IDENTITY"
rm -rf "$APP"
pnpm tauri build --bundles app -- --locked

# What was built must be the installed Satchel, signed for distribution, before anything leaves
# this Mac.
plist="$APP/Contents/Info.plist"
[ "$(plutil -extract CFBundleIdentifier raw -o - "$plist")" = app.satchel.desktop ] \
  || stop "the build is not the installed Satchel (its identifier is not app.satchel.desktop)."
[ "$(plutil -extract CFBundleName raw -o - "$plist")" = Satchel ] || stop "the build is not called Satchel."
[ "$(plutil -extract CFBundleShortVersionString raw -o - "$plist")" = "$VERSION" ] || stop "the build is not version $VERSION."
cmp -s "$APP/Contents/Resources/THIRD-PARTY-NOTICES.txt" THIRD-PARTY-NOTICES.txt \
  || stop "Satchel.app does not contain the current THIRD-PARTY-NOTICES.txt."
codesign --verify --deep --strict "$APP"
signature=$(codesign -dvv "$APP" 2>&1)
echo "$signature" | grep -q "^Authority=Developer ID Application:" || stop "Satchel.app is not signed with a Developer ID."
echo "$signature" | grep -q "flags=.*(runtime)" || stop "Satchel.app is not signed with the hardened runtime."
echo "$signature" | grep -q "^Timestamp=" || stop "Satchel.app's signature has no secure timestamp."

rm -rf "$OUT"
mkdir -p "$OUT"
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' 0
trap 'exit 130' INT
trap 'exit 143' TERM

notarize() {
  echo "Notarizing $(basename "$1") (Apple usually takes a few minutes)"
  xcrun notarytool submit "$1" --keychain-profile "$PROFILE" --wait
}

if ! $local; then
  ditto -c -k --keepParent "$APP" "$STAGE/Satchel-for-notary.zip"
  notarize "$STAGE/Satchel-for-notary.zip"
  xcrun stapler staple "$APP"
  spctl --assess --type execute "$APP"
fi

echo "Making the DMG"
mkdir "$STAGE/dmg"
ditto "$APP" "$STAGE/dmg/Satchel.app"
create-dmg --volname "Satchel $VERSION" --window-size 540 360 --icon-size 96 \
  --icon Satchel.app 140 170 --hide-extension Satchel.app --app-drop-link 400 170 \
  --codesign "$APPLE_SIGNING_IDENTITY" --no-internet-enable \
  "$OUT/$DMG" "$STAGE/dmg" >/dev/null
if ! $local; then
  notarize "$OUT/$DMG"
  xcrun stapler staple "$OUT/$DMG"
  xcrun stapler validate "$OUT/$DMG"
  spctl --assess --type open --context context:primary-signature "$OUT/$DMG"
fi

# The update: the same Satchel.app (notarized and stapled above, unless --local), signed with
# the update key for its version, and the feed that announces it. Every installed Satchel checks
# the signature against the public key in tauri.conf.json and refuses anything else.
echo "Making the update"
COPYFILE_DISABLE=1 tar -czf "$OUT/$ARCHIVE" -C "$(dirname "$APP")" Satchel.app
TAURI_SIGNING_PRIVATE_KEY_PASSWORD="" pnpm -s tauri signer sign --app-version "$VERSION" -f "$KEY" "$OUT/$ARCHIVE" >/dev/null
node scripts/update-feed.ts --version "$VERSION" --archive "$OUT/$ARCHIVE" --notes "$NOTES" \
  --url "https://github.com/hoomanaskari/Satchel/releases/download/$TAG/$ARCHIVE" --out "$OUT/latest.json"

(cd "$OUT" && shasum -a 256 "$DMG" "$ARCHIVE" latest.json > SHA256SUMS.txt)
echo
cat "$OUT/SHA256SUMS.txt"
echo
if $local; then
  echo "Rehearsal of Satchel $VERSION in $OUT (signed, not notarized). Publish nothing from it."
else
  echo "Satchel $VERSION is ready in $OUT."
  echo "Next: tag $TAG, push the tag, and create a GitHub release for it with $NOTES as its text."
fi
echo "Attach these files to the release:"
echo "  $OUT/$DMG"
echo "  $OUT/$ARCHIVE"
echo "  $OUT/latest.json"
echo "  $OUT/SHA256SUMS.txt"
echo "Installed copies update once the release is published (not a draft or pre-release): they read"
echo "latest.json from the newest release. The .sig file is inside latest.json; it need not be attached."

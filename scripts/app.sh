#!/bin/sh
# Builds Satchel on this Mac: target/release/bundle/macos/Satchel.app.
#
# Signed with one identity that stays the same from build to build. macOS ties the Accessibility
# permission (needed to paste into other apps) to that signature: an ad-hoc build looks like a
# new app every time, so the permission would have to be granted again after each rebuild.
# Signs with APPLE_SIGNING_IDENTITY when set; otherwise with the Developer ID Application
# certificate on this Mac that lasts longest; with none, ad-hoc (and the permission won't last).
set -e

root=$(cd "$(dirname "$0")/.." && pwd)
. "$root/scripts/signing-identity.sh"

if [ -n "$APPLE_SIGNING_IDENTITY" ]; then
  export APPLE_SIGNING_IDENTITY
  echo "Signing Satchel.app with $APPLE_SIGNING_IDENTITY"
else
  echo "No Developer ID certificate: Satchel.app will be signed ad-hoc"
fi

cd "$root"
pnpm tauri build --bundles app "$@"

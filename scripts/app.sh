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

# Signing identity: unless APPLE_SIGNING_IDENTITY is set, the Developer ID Application
# certificate whose expiry date is furthest away. With none, it stays empty.
if [ -z "$APPLE_SIGNING_IDENTITY" ] && [ "$(uname)" = Darwin ]; then
  newest=0
  for hash in $(security find-identity -v -p codesigning | awk '/"Developer ID Application:/ { print $2 }'); do
    ends=$(security find-certificate -a -Z -p | awk -v h="$hash" '
      /^SHA-1 hash:/ { keep = ($3 == h) }
      keep' | openssl x509 -noout -enddate | cut -d= -f2)
    ends=$(date -j -f "%b %e %T %Y %Z" "$ends" +%s)
    if [ "$ends" -gt "$newest" ]; then
      newest=$ends
      APPLE_SIGNING_IDENTITY=$hash
    fi
  done
fi

if [ -n "$APPLE_SIGNING_IDENTITY" ]; then
  export APPLE_SIGNING_IDENTITY
  echo "Signing Satchel.app with $APPLE_SIGNING_IDENTITY"
else
  echo "No Developer ID certificate: Satchel.app will be signed ad-hoc"
fi

cd "$root"
pnpm tauri build --bundles app "$@"

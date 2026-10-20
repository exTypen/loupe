#!/bin/sh
# Builds, signs, notarizes and staples build/Loupe-<version>.dmg for distribution.
#   LOUPE_VERSION=0.1.0 scripts/release.sh
# One-time setup:
#   1. A "Developer ID Application" certificate in the login keychain (Xcode → Settings → Accounts →
#      Manage Certificates → + → Developer ID Application; only the team's Account Holder can create one).
#   2. Notary credentials stored under a keychain profile:
#      xcrun notarytool store-credentials loupe-notary --apple-id <apple-id> --team-id <team-id>
#      (asks for an app-specific password from appleid.apple.com)
set -e
cd "$(dirname "$0")/.."
VERSION="${LOUPE_VERSION:-0.1.0}"
PROFILE="${LOUPE_NOTARY_PROFILE:-loupe-notary}"

SIGN_ID="${LOUPE_SIGN_ID:-$(security find-identity -v -p codesigning | awk -F'"' 'index($2, "Developer ID Application") == 1 { print $2; exit }')}"
[ -n "$SIGN_ID" ] || { echo "No Developer ID Application certificate found; see the setup notes at the top of this script." >&2; exit 1; }
LOUPE_SIGN_ID="$SIGN_ID" LOUPE_VERSION="$VERSION" scripts/bundle.sh

DMG="build/Loupe-$VERSION.dmg"
STAGE=$(mktemp -d)
cp -R build/Loupe.app "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
hdiutil create -volname "Loupe $VERSION" -srcfolder "$STAGE" -fs HFS+ -format UDZO -ov "$DMG" >/dev/null
rm -rf "$STAGE"
codesign --force --timestamp -s "$SIGN_ID" "$DMG"

xcrun notarytool submit "$DMG" --keychain-profile "$PROFILE" --wait
xcrun stapler staple "$DMG"
spctl --assess --type open --context context:primary-signature -v "$DMG"
shasum -a 256 "$DMG"

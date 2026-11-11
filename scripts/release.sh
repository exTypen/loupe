#!/bin/sh
# Builds, signs, notarizes and staples build/Loupe-<version>.dmg for distribution, signs it for Sparkle and writes
# build/appcast.xml, the feed installed apps read at releases/latest/download/appcast.xml.
#   LOUPE_VERSION=0.1.0 scripts/release.sh
# One-time setup:
#   1. A "Developer ID Application" certificate in the login keychain (Xcode → Settings → Accounts →
#      Manage Certificates → + → Developer ID Application; only the team's Account Holder can create one).
#   2. Notary credentials stored under a keychain profile:
#      xcrun notarytool store-credentials loupe-notary --apple-id <apple-id> --team-id <team-id>
#      (asks for an app-specific password from appleid.apple.com)
#   3. A Sparkle EdDSA key pair. After any `swift build`, Sparkle's tools are in .build/artifacts/sparkle/Sparkle/bin:
#      .build/artifacts/sparkle/Sparkle/bin/generate_keys
#      stores the private key in the login keychain and prints the public key; paste that into
#      scripts/sparkle-public-key.txt and commit it. Keep the private key: back it up with `generate_keys -x <file>`.
#      Losing it means installed copies can no longer verify (and so never install) a new release.
set -e
cd "$(dirname "$0")/.."
VERSION="${LOUPE_VERSION:-0.1.0}"
PROFILE="${LOUPE_NOTARY_PROFILE:-loupe-notary}"
SPARKLE_BIN=.build/artifacts/sparkle/Sparkle/bin
REPO_URL=https://github.com/exTypen/loupe

SIGN_ID="${LOUPE_SIGN_ID:-$(security find-identity -v -p codesigning | awk -F'"' 'index($2, "Developer ID Application") == 1 { print $2; exit }')}"
[ -n "$SIGN_ID" ] || { echo "No Developer ID Application certificate found; see the setup notes at the top of this script." >&2; exit 1; }
LOUPE_SIGN_ID="$SIGN_ID" LOUPE_VERSION="$VERSION" scripts/bundle.sh
PLIST=build/Loupe.app/Contents/Info.plist
/usr/libexec/PlistBuddy -c "Print :SUPublicEDKey" "$PLIST" >/dev/null 2>&1 \
  || { echo "No Sparkle public key in the build: a release without one could never update itself. See step 3 above." >&2; exit 1; }
BUILD=$(/usr/libexec/PlistBuddy -c "Print :CFBundleVersion" "$PLIST")

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

# Sign the final (stapled) bytes: stapling changes the file. Prints: sparkle:edSignature="…" length="…"
SIGNATURE=$("$SPARKLE_BIN/sign_update" "$DMG")
PUBDATE=$(LC_ALL=C date -u "+%a, %d %b %Y %H:%M:%S +0000")
cat > build/appcast.xml <<XML
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>Loupe</title>
    <link>$REPO_URL</link>
    <item>
      <title>Loupe $VERSION</title>
      <pubDate>$PUBDATE</pubDate>
      <sparkle:version>$BUILD</sparkle:version>
      <sparkle:shortVersionString>$VERSION</sparkle:shortVersionString>
      <sparkle:minimumSystemVersion>15.0</sparkle:minimumSystemVersion>
      <sparkle:releaseNotesLink>$REPO_URL/releases/tag/v$VERSION</sparkle:releaseNotesLink>
      <enclosure url="$REPO_URL/releases/download/v$VERSION/Loupe-$VERSION.dmg" $SIGNATURE type="application/octet-stream"/>
    </item>
  </channel>
</rss>
XML
echo "Wrote build/appcast.xml (version $VERSION, build $BUILD)"
echo "Publish with:"
echo "  gh release create v$VERSION $DMG build/appcast.xml --title \"Loupe $VERSION\" --notes-file <notes.md>"
echo "  (or, for an existing release: gh release upload v$VERSION $DMG build/appcast.xml --clobber)"

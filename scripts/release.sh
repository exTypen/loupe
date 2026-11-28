#!/bin/sh
# Builds, signs, notarizes and staples build/Loupe-<version>.dmg for distribution.
#   scripts/release.sh            (the version comes from app/Cargo.toml; LOUPE_VERSION overrides it)
# One-time setup:
#   1. A "Developer ID Application" certificate in the login keychain.
#   2. Notary credentials under a keychain profile:
#      xcrun notarytool store-credentials loupe-notary --apple-id <apple-id> --team-id <team-id>
#   3. The Sparkle EdDSA private key in the login keychain (build/cache/sparkle-*/bin/generate_keys created it; back it
#      up with `generate_keys -x <file>`). Losing it means installed copies can never verify a new release.
# Installed copies update through Sparkle from releases/latest/download/appcast.xml, which this writes too.
set -e
cd "$(dirname "$0")/.."
VERSION="${LOUPE_VERSION:-$(awk -F'"' '/^version/ { print $2; exit }' app/Cargo.toml)}"
PROFILE="${LOUPE_NOTARY_PROFILE:-loupe-notary}"
REPO_URL=https://github.com/exTypen/loupe

SIGN_ID="${LOUPE_SIGN_ID:-$(security find-identity -v -p codesigning | awk -F'"' 'index($2, "Developer ID Application") == 1 { print $2; exit }')}"
[ -n "$SIGN_ID" ] || { echo "No Developer ID Application certificate found; see the setup notes at the top of this script." >&2; exit 1; }
LOUPE_SIGN_ID="$SIGN_ID" LOUPE_VERSION="$VERSION" scripts/bundle.sh
/usr/libexec/PlistBuddy -c "Print :SUPublicEDKey" build/Loupe.app/Contents/Info.plist >/dev/null 2>&1 \
  || { echo "No Sparkle public key in the build: a release without one could never update itself." >&2; exit 1; }
BUILD=$(/usr/libexec/PlistBuddy -c "Print :CFBundleVersion" build/Loupe.app/Contents/Info.plist)

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

SPARKLE_BIN="${SPARKLE_BIN:-$(ls -d build/cache/sparkle-*/bin | tail -1)}"
[ -x "$SPARKLE_BIN/sign_update" ] || { echo "No Sparkle sign_update in $SPARKLE_BIN" >&2; exit 1; }
{
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
}
# This release's performance baseline (about a minute, headless: nothing opens on screen), what the next release is
# compared against with `scripts/bench.sh compare $VERSION`. Never fails the release; LOUPE_SKIP_BENCH=1 skips it.
if [ -z "$LOUPE_SKIP_BENCH" ]; then
  scripts/bench.sh record "$VERSION" && echo "Commit bench/baselines/$VERSION.json with the release." \
    || echo "warning: scripts/bench.sh record $VERSION failed; record the baseline by hand." >&2
fi
echo "Publish with:"
echo "  gh release create v$VERSION $DMG build/appcast.xml --title \"Loupe $VERSION\" --notes-file <notes.md>"

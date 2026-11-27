#!/bin/sh
# Builds a release Loupe.app into ./build.
#   scripts/bundle.sh [--install]
# Signing identity: $LOUPE_SIGN_ID, else the first "Developer ID Application" certificate, else the first
# "Apple Development" one, else ad-hoc. A stable identity matters: the Keychain ACL on stored tokens is tied to
# the code signature, and an ad-hoc signature changes on every build, so each launch re-prompts for access.
set -e
cd "$(dirname "$0")/.."
VERSION="${LOUPE_VERSION:-$(awk -F'"' '/^version/ { print $2; exit }' app/Cargo.toml)}"
# CFBundleVersion must grow with every release: x.y.z → x*10000 + y*100 + z, so 0.10.0 → 1000.
BUILD="${LOUPE_BUILD:-$(echo "$VERSION" | awk -F. 'NF != 3 || $0 !~ /^[0-9]+\.[0-9]+\.[0-9]+$/ || $2 > 99 || $3 > 99 { exit 1 } { print $1 * 10000 + $2 * 100 + $3 }')}" \
  || { echo "LOUPE_VERSION must be x.y.z with y and z below 100 (or set LOUPE_BUILD): $VERSION" >&2; exit 1; }

MACOSX_DEPLOYMENT_TARGET=15.0 cargo build --release -p loupe
APP=build/Loupe.app
rm -rf "$APP" && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/loupe "$APP/Contents/MacOS/Loupe"
cp scripts/AppIcon.icns "$APP/Contents/Resources/"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Loupe</string>
  <key>CFBundleDisplayName</key><string>Loupe</string>
  <key>CFBundleIdentifier</key><string>dev.extypen.loupe</string>
  <key>CFBundleExecutable</key><string>Loupe</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$BUILD</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>LSMinimumSystemVersion</key><string>15.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticTermination</key><false/>
</dict></plist>
PLIST

identity() { security find-identity -v -p codesigning | awk -F'"' -v k="$1" 'index($2, k) == 1 { print $2; exit }'; }
SIGN_ID="${LOUPE_SIGN_ID:-$(identity "Developer ID Application")}"
SIGN_ID="${SIGN_ID:-$(identity "Apple Development")}"
case "$SIGN_ID" in
  # Notarization requires the hardened runtime and a secure timestamp.
  "Developer ID Application"*) codesign --force --options runtime --timestamp -s "$SIGN_ID" "$APP" ;;
  *) codesign --force -s "${SIGN_ID:--}" "$APP" ;;
esac
codesign --verify --deep --strict "$APP"
echo "Signed with: ${SIGN_ID:-ad-hoc}"
echo "Built $APP ($VERSION, build $BUILD)"
if [ "$1" = "--install" ]; then rm -rf /Applications/Loupe.app && cp -R "$APP" /Applications/ && echo "Installed /Applications/Loupe.app"; fi

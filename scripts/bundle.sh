#!/bin/sh
# Builds a release Loupe.app into ./build.
#   scripts/bundle.sh [--install]
# Signing identity: $LOUPE_SIGN_ID, else the first "Developer ID Application" certificate, else the first
# "Apple Development" one, else ad-hoc. A stable identity matters: the Keychain ACL on stored tokens is tied to
# the code signature, and an ad-hoc signature changes on every build, so each launch re-prompts for access.
# Auto-update: the Sparkle EdDSA public key comes from $LOUPE_SPARKLE_PUBLIC_KEY, else scripts/sparkle-public-key.txt
# (one-time key setup is described in scripts/release.sh). Without a key the app builds but hides updating.
set -e
cd "$(dirname "$0")/.."
VERSION="${LOUPE_VERSION:-0.1.0}"
# CFBundleVersion must grow with every release (Sparkle compares it): x.y.z → x*10000 + y*100 + z, so 0.8.0 → 800.
# A version that doesn't fit the formula (not x.y.z, or y/z above 99) would give a build number that can go backwards.
BUILD="${LOUPE_BUILD:-$(echo "$VERSION" | awk -F. 'NF != 3 || $0 !~ /^[0-9]+\.[0-9]+\.[0-9]+$/ || $2 > 99 || $3 > 99 { exit 1 } { print $1 * 10000 + $2 * 100 + $3 }')}" \
  || { echo "LOUPE_VERSION must be x.y.z with y and z below 100 (or set LOUPE_BUILD): $VERSION" >&2; exit 1; }

swift build -c release
APP=build/Loupe.app
rm -rf "$APP" && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Frameworks"
cp .build/release/Loupe "$APP/Contents/MacOS/Loupe"
# macOS draws an app in its pre-26 look (no Liquid Glass sidebar, toolbar or controls) unless the binary says it was
# built with the macOS 26+ SDK. Since Xcode 27.2, SwiftPM's link stamps LC_BUILD_VERSION's sdk with the deployment
# target (15.0) instead, so stamp the SDK we actually built against. Signing below covers the rewritten binary.
xcrun vtool -set-build-version macos 15.0 "$(xcrun --show-sdk-version)" -replace \
  -output "$APP/Contents/MacOS/Loupe" "$APP/Contents/MacOS/Loupe"
# ditto keeps the framework's Versions/Current symlinks, which codesign requires.
ditto .build/release/Sparkle.framework "$APP/Contents/Frameworks/Sparkle.framework"
otool -l "$APP/Contents/MacOS/Loupe" | grep -q "@executable_path/../Frameworks" \
  || install_name_tool -add_rpath @executable_path/../Frameworks "$APP/Contents/MacOS/Loupe"
cp scripts/AppIcon.icns "$APP/Contents/Resources/"

SPARKLE_KEY=$(printf %s "${LOUPE_SPARKLE_PUBLIC_KEY:-$(grep -v '^#' scripts/sparkle-public-key.txt 2>/dev/null)}" | tr -d '[:space:]')
if [ -n "$SPARKLE_KEY" ]; then SPARKLE_KEY_PLIST="<key>SUPublicEDKey</key><string>$SPARKLE_KEY</string>"
else echo "warning: no Sparkle public key (LOUPE_SPARKLE_PUBLIC_KEY or scripts/sparkle-public-key.txt): this build will not offer updates" >&2; fi

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
  <key>SUFeedURL</key><string>https://github.com/exTypen/loupe/releases/latest/download/appcast.xml</string>
  ${SPARKLE_KEY_PLIST:-}
  <key>SUEnableAutomaticChecks</key><true/>
  <key>SUScheduledCheckInterval</key><integer>86400</integer>
</dict></plist>
PLIST

identity() { security find-identity -v -p codesigning | awk -F'"' -v k="$1" 'index($2, k) == 1 { print $2; exit }'; }
SIGN_ID="${LOUPE_SIGN_ID:-$(identity "Developer ID Application")}"
SIGN_ID="${SIGN_ID:-$(identity "Apple Development")}"
case "$SIGN_ID" in
  # Notarization requires the hardened runtime and a secure timestamp, on nested code too.
  "Developer ID Application"*) sign() { codesign --force --options runtime --timestamp -s "$SIGN_ID" "$@"; } ;;
  *) sign() { codesign --force -s "${SIGN_ID:--}" "$@"; } ;;
esac
# Inside-out, as Sparkle documents: its helpers, the framework, then the app. Not --deep, which drops entitlements.
SP="$APP/Contents/Frameworks/Sparkle.framework/Versions/B"
sign "$SP/XPCServices/Installer.xpc"
sign --preserve-metadata=entitlements "$SP/XPCServices/Downloader.xpc"
sign "$SP/Autoupdate"
sign "$SP/Updater.app"
sign "$APP/Contents/Frameworks/Sparkle.framework"
sign "$APP"
codesign --verify --deep --strict "$APP"
echo "Signed with: ${SIGN_ID:-ad-hoc}"
echo "Built $APP ($VERSION, build $BUILD)"
if [ "$1" = "--install" ]; then rm -rf /Applications/Loupe.app && cp -R "$APP" /Applications/ && echo "Installed /Applications/Loupe.app"; fi

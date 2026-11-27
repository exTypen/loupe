#!/bin/sh
# Builds a release Loupe.app into ./build.
#   scripts/bundle.sh [--install]
# Signing identity: $LOUPE_SIGN_ID, else the first "Developer ID Application" certificate, else the first
# "Apple Development" one, else ad-hoc. A stable identity matters: the Keychain ACL on stored tokens is tied to
# the code signature, and an ad-hoc signature changes on every build, so each launch re-prompts for access.
# Auto-update: Sparkle 2 (pinned below, downloaded once into build/cache) goes into Contents/Frameworks, and the feed
# and EdDSA public key (scripts/sparkle-public-key.txt, or $LOUPE_SPARKLE_PUBLIC_KEY) into Info.plist. The app loads
# Sparkle at run time (app/src/updates.rs); without a key the framework is left out and the app hides updating.
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

SPARKLE_VERSION=2.10.1
SPARKLE_SHA256=1a856d2a9edb8e628de88121d7be04cf0e2167f8641650b78fca00b14244cf27
SPARKLE_KEY=$(printf %s "${LOUPE_SPARKLE_PUBLIC_KEY:-$(grep -v '^#' scripts/sparkle-public-key.txt 2>/dev/null)}" | tr -d '[:space:]')
if [ -n "$SPARKLE_KEY" ]; then
  SPARKLE=build/cache/sparkle-$SPARKLE_VERSION
  if [ ! -d "$SPARKLE/Sparkle.framework" ]; then
    mkdir -p "$SPARKLE"
    curl -sSL -o "$SPARKLE.tar.xz" "https://github.com/sparkle-project/Sparkle/releases/download/$SPARKLE_VERSION/Sparkle-$SPARKLE_VERSION.tar.xz"
    echo "$SPARKLE_SHA256  $SPARKLE.tar.xz" | shasum -a 256 -c - >/dev/null || { echo "Sparkle download doesn't match its checksum" >&2; exit 1; }
    tar -xf "$SPARKLE.tar.xz" -C "$SPARKLE"
  fi
  mkdir -p "$APP/Contents/Frameworks"
  # ditto keeps the framework's Versions/Current symlinks, which codesign requires.
  ditto "$SPARKLE/Sparkle.framework" "$APP/Contents/Frameworks/Sparkle.framework"
  SPARKLE_PLIST="<key>SUFeedURL</key><string>https://github.com/exTypen/loupe/releases/latest/download/appcast.xml</string>
  <key>SUPublicEDKey</key><string>$SPARKLE_KEY</string>
  <key>SUEnableAutomaticChecks</key><true/>
  <key>SUScheduledCheckInterval</key><integer>86400</integer>"
else
  echo "warning: no Sparkle public key (LOUPE_SPARKLE_PUBLIC_KEY or scripts/sparkle-public-key.txt): this build will not offer updates" >&2
fi

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
  ${SPARKLE_PLIST:-}
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
if [ -d "$APP/Contents/Frameworks/Sparkle.framework" ]; then
  SP="$APP/Contents/Frameworks/Sparkle.framework/Versions/B"
  sign "$SP/XPCServices/Installer.xpc"
  sign --preserve-metadata=entitlements "$SP/XPCServices/Downloader.xpc"
  sign "$SP/Autoupdate"
  sign "$SP/Updater.app"
  sign "$APP/Contents/Frameworks/Sparkle.framework"
fi
sign "$APP"
codesign --verify --deep --strict "$APP"
echo "Signed with: ${SIGN_ID:-ad-hoc}"
echo "Built $APP ($VERSION, build $BUILD)"
if [ "$1" = "--install" ]; then rm -rf /Applications/Loupe.app && cp -R "$APP" /Applications/ && echo "Installed /Applications/Loupe.app"; fi

#!/bin/sh
set -e

basedir="$(dirname "$(realpath "$0")")"
cd "$basedir"

export CFLAGS= CXXFLAGS= CPPFLAGS= LDFLAGS= RUSTFLAGS=

ANDROID_APP="target/dx/p2pro-rs/release/android/app/app"
ANDROID_RES="$ANDROID_APP/src/main/res"

# The custom android/AndroidManifest.xml references @xml/device_filter (the
# USB device filter for the USB_DEVICE_ATTACHED intent filter).  dx renders
# the Gradle project from its own templates and cannot know about that
# resource, so pre-seed it BEFORE `dx build` runs Gradle: dx only ever
# (over)writes its own generated files and never wipes the project
# directory, so a pre-seeded file survives the rendering.
mkdir -p "$ANDROID_RES/xml"
cp android/res/xml/device_filter.xml "$ANDROID_RES/xml/"

# R8 (release minification) can't see the JNI call sites in native code, so it
# would otherwise strip/rename MainActivity's JNI-called static methods.
# Pre-seed a keep rule under a name dx's own template doesn't generate (unlike
# proguard-rules.pro, which dx re-renders from scratch every build)
mkdir -p "$ANDROID_APP"
cp android/proguard-jni-keep.pro "$ANDROID_APP/"

dx build --android --target aarch64-linux-android --release

# Fix the generated display name (dx derives it from the binary name).
sed -i 's|<string name="app_name">P2ProRs</string>|<string name="app_name">InfiRay P2Pro Rs</string>|' \
    "$ANDROID_RES/values/strings.xml"

# dx hardcodes versionCode = 1 in its build.gradle.kts template.  Derive it
# from the Cargo workspace version (major*10000 + minor*100 + patch).
VERSION="$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')"
VER_MAJOR="$(echo "$VERSION" | cut -d. -f1)"
VER_MINOR="$(echo "$VERSION" | cut -d. -f2)"
VER_PATCH="$(echo "$VERSION" | cut -d. -f3)"
VERSION_CODE="$(expr "$VER_MAJOR" \* 10000 + "$VER_MINOR" \* 100 + "$VER_PATCH")"
sed -i "s/versionCode = 1\b/versionCode = $VERSION_CODE/" \
    "$ANDROID_APP/build.gradle.kts"

# dx hardcodes default launcher icons into the Android project and doesn't
# honour [bundle] icon or [android] icon for Android builds.  Work around
# this by overwriting the generated resources and re-running gradle.
cp android/res/drawable/ic_launcher_background.xml         "$ANDROID_RES/drawable/"
cp android/res/drawable-v24/ic_launcher_foreground.xml     "$ANDROID_RES/drawable-v24/"
cp android/res/mipmap-anydpi-v26/ic_launcher.xml           "$ANDROID_RES/mipmap-anydpi-v26/"
cp android/res/mipmap-mdpi/ic_launcher.webp                "$ANDROID_RES/mipmap-mdpi/"
cp android/res/mipmap-hdpi/ic_launcher.webp                "$ANDROID_RES/mipmap-hdpi/"
cp android/res/mipmap-xhdpi/ic_launcher.webp               "$ANDROID_RES/mipmap-xhdpi/"
cp android/res/mipmap-xxhdpi/ic_launcher.webp              "$ANDROID_RES/mipmap-xxhdpi/"
cp android/res/mipmap-xxxhdpi/ic_launcher.webp             "$ANDROID_RES/mipmap-xxxhdpi/"

# Rebuild the release APK with the updated icons.
(
    cd "$ANDROID_APP/.."
    ./gradlew packageRelease
    ./gradlew bundleRelease
)

cp "./$ANDROID_APP/build/outputs/apk/release/app-release-unsigned.apk" \
   ./p2pro-rs-aarch64-unsigned.apk
cp "./$ANDROID_APP/build/outputs/bundle/release/app-release.aab" \
   ./p2pro-rs-aarch64-unsigned.aab

./android-sign.sh

echo
echo "Successfully built Android packages (signed with dev keys):"
echo "  p2pro-rs-aarch64.apk"
echo "  p2pro-rs-aarch64.aab"

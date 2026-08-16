#!/bin/sh
set -e

basedir="$(realpath "$0" | xargs dirname)"
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

dx build --android --target aarch64-linux-android --release

# dx hardcodes default launcher icons into the Android project and doesn't
# honour [bundle] icon or [android] icon for Android builds.  Work around
# this by overwriting the generated resources and re-running gradle.
#cp android/res/drawable/ic_launcher_background.xml         "$ANDROID_RES/drawable/"
#cp android/res/drawable-v24/ic_launcher_foreground.xml     "$ANDROID_RES/drawable-v24/"
#cp android/res/mipmap-anydpi-v26/ic_launcher.xml           "$ANDROID_RES/mipmap-anydpi-v26/"
#cp android/res/mipmap-mdpi/ic_launcher.webp                "$ANDROID_RES/mipmap-mdpi/"
#cp android/res/mipmap-hdpi/ic_launcher.webp                "$ANDROID_RES/mipmap-hdpi/"
#cp android/res/mipmap-xhdpi/ic_launcher.webp               "$ANDROID_RES/mipmap-xhdpi/"
#cp android/res/mipmap-xxhdpi/ic_launcher.webp              "$ANDROID_RES/mipmap-xxhdpi/"
#cp android/res/mipmap-xxxhdpi/ic_launcher.webp             "$ANDROID_RES/mipmap-xxxhdpi/"

# Rebuild the release APK with the updated icons.
(
    cd target/dx/p2pro-rs/release/android/app
    ./gradlew packageRelease
    ./gradlew bundleRelease
)

cp ./target/dx/p2pro-rs/release/android/app/app/build/outputs/apk/release/app-release-unsigned.apk \
   ./p2pro-rs-aarch64-unsigned.apk
cp ./target/dx/p2pro-rs/release/android/app/app/build/outputs/bundle/release/app-release.aab \
   ./p2pro-rs-aarch64-unsigned.aab

./android-sign.sh

echo
echo "Successfully built Android packages (signed with dev keys):"
echo "  p2pro-rs-aarch64.apk"
echo "  p2pro-rs-aarch64.aab"

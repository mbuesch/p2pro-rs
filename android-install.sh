#!/bin/sh
set -e

basedir="$(dirname "$(realpath "$0")")"
cd "$basedir"

if [ -z "$APK_SIGNED" ]; then APK_SIGNED="p2pro-rs-aarch64.apk"; fi

adb uninstall ch.bues.p2pro 2>/dev/null || true
adb install "$APK_SIGNED"

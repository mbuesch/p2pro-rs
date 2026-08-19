#!/bin/sh

basedir="$(realpath "$0" | xargs dirname)"
cd "$basedir"

cargo clean
rm -f p2pro-rs-aarch64-unsigned.apk
rm -f p2pro-rs-aarch64.apk
rm -f p2pro-rs-aarch64.apk.idsig
rm -f p2pro-rs-aarch64-release.apk
rm -f p2pro-rs-aarch64-release.apk.idsig
rm -f p2pro-rs-aarch64-unsigned.aab
rm -f p2pro-rs-aarch64.aab
rm -f p2pro-rs-aarch64-release.aab

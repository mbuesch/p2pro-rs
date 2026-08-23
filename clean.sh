#!/bin/sh
set -e

basedir="$(dirname "$(realpath "$0")")"
cd "$basedir"

rm -f p2pro-rs-aarch64-unsigned.apk
rm -f p2pro-rs-aarch64.apk
rm -f p2pro-rs-aarch64.apk.idsig
rm -f p2pro-rs-aarch64-release.apk
rm -f p2pro-rs-aarch64-release.apk.idsig
rm -f p2pro-rs-aarch64-unsigned.aab
rm -f p2pro-rs-aarch64.aab
rm -f p2pro-rs-aarch64-release.aab
cargo clean

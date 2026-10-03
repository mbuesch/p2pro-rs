#!/bin/sh
set -e

basedir="$(dirname "$(realpath "$0")")"
cd "$basedir"

dx build --desktop --release
cargo build -p p2pro-cli --release

cp ./target/dx/p2pro-rs/release/linux/app/p2pro-rs \
   ./p2pro-rs-desktop-linux-x64
cp ./target/release/p2pro-cli \
   ./p2pro-cli-desktop-linux-x64

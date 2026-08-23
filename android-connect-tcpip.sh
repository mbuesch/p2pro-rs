#!/bin/sh
set -e
addr="$1"
if [ -z "$addr" ]; then
    echo "Usage: $0 ANDROID_IP_ADDR" >&2
    exit 1
fi
adb tcpip 5555
adb connect "$addr:5555"

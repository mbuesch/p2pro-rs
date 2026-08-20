#!/bin/sh
set -e
addr="$1"
adb tcpip 5555
adb connect "$addr:5555"

#!/bin/sh
set -e
adb logcat * | grep -i -e P2Pro

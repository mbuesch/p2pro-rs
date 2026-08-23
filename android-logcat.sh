#!/bin/sh
set -e
adb logcat * | grep -i -Ee 'P2Pro|RustStdoutStderr'

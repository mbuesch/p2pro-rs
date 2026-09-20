#!/bin/sh
# -*- coding: utf-8 -*-

basedir="$(dirname "$(realpath "$0")")"

. "$basedir/scripts/lib.sh"

install_entry_checks()
{
    [ -f "$bin" ] || die "p2pro-rs is not built! Run ./desktop-build-linux.sh"
    [ "$(id -u)" = "0" ] || die "Must be root to install p2pro-rs."
}

install_dirs()
{
    do_install \
        -o root -g root -m 0755 \
        -d /opt/p2pro-rs/bin
}

install_p2prors()
{
    do_install \
        -o root -g root -m 0755 \
        "$bin" \
        /opt/p2pro-rs/bin/p2pro-rs
}

install_udev_rules()
{
    if ! [ -f /etc/udev/rules.d/99-p2pro.rules ]; then
        do_install \
            -o root -g root -m 0644 \
            "$basedir/assets/99-p2pro.rules" \
            /etc/udev/rules.d/99-p2pro.rules
        udevadm control --reload-rules || die "Failed to reload udev rules."
    else
        info "Udev rules /etc/udev/rules.d/99-p2pro.rules already installed."
    fi
}

bin="$basedir/p2pro-rs-desktop-linux-x64"

install_entry_checks
install_dirs
install_p2prors
install_udev_rules

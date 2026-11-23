#!/bin/sh
# Package a staged, stripped release binary. Run on Debian or Ubuntu.
set -eu
binary=$1
version=$2
output=$3
staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT
install -Dm755 "$binary" "$staging/usr/bin/hermes-browser"
install -Dm644 packaging/linux/hermes-browser.desktop "$staging/usr/share/applications/hermes-browser.desktop"
install -Dm644 apps/hermes-desktop/resources/hermes-browser.svg "$staging/usr/share/icons/hicolor/scalable/apps/hermes-browser.svg"
install -Dm644 LICENSE "$staging/usr/share/doc/hermes-browser/copyright"
mkdir -p "$staging/DEBIAN"
cat > "$staging/DEBIAN/control" <<CONTROL
Package: hermes-browser
Version: $version
Architecture: amd64
Maintainer: Hermes Browser contributors
Section: web
Priority: optional
Depends: libc6 (>= 2.35), libstdc++6, libfontconfig1, libx11-xcb1, libxcursor1, libxkbcommon-x11-0, libegl1, libgl1, libvulkan1, gstreamer1.0-gl, gstreamer1.0-plugins-base, gstreamer1.0-plugins-good, gstreamer1.0-plugins-bad, gstreamer1.0-plugins-ugly, gstreamer1.0-libav, gstreamer1.0-pulseaudio
Recommends: mesa-vulkan-drivers | vulkan-icd
Description: Hermes desktop web browser
 A native desktop browser powered by Servo, Slint and GStreamer.
CONTROL
dpkg-deb -Zgzip -z6 --root-owner-group --build "$staging" "$output"

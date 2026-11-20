#!/bin/sh
# Builds a .deb and an .rpm from a static Linux release archive. The binary
# needs no system libraries, so one package per CPU suits every distribution.
#
#   sh scripts/build-linux-packages.sh target/dist/megawren-X.Y.Z-x86_64-unknown-linux-musl.tar.gz
set -eu
root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
archive=$1
name=$(basename "$archive" .tar.gz)
version=$(echo "$name" | cut -d- -f2)
case $(echo "$name" | cut -d- -f3) in
    x86_64) arch=amd64 ;;
    aarch64) arch=arm64 ;;
    *) echo "build-linux-packages: unsupported archive $archive" >&2; exit 1 ;;
esac
work="$root/target/linux-packages"
rm -rf "$work"
mkdir -p "$work"
tar -xzf "$archive" -C "$work"
cat >"$work/nfpm.yaml" <<EOF
name: megawren
arch: $arch
platform: linux
version: $version
release: 1
section: net
priority: optional
maintainer: def324 <184112+def324@users.noreply.github.com>
description: |-
  Terminal file browser and transfer client for MEGA.
  Statically linked, so it needs no other packages.
homepage: https://github.com/def324/megawren
license: MIT
contents:
  - src: $name/bin/megawren
    dst: /usr/bin/megawren
    file_info:
      mode: 0755
  - src: $name/LICENSE
    dst: /usr/share/doc/megawren/copyright
    packager: deb
  - src: $name/LICENSE
    dst: /usr/share/licenses/megawren/LICENSE
    type: license
    packager: rpm
  - src: $name/THIRD_PARTY_NOTICES.md
    dst: /usr/share/doc/megawren/THIRD_PARTY_NOTICES.md
  - src: $name/licenses
    dst: /usr/share/doc/megawren/licenses
    type: tree
EOF
for packager in deb rpm; do
    docker run --rm --user "$(id -u):$(id -g)" -v "$work:/work" -w /work \
        goreleaser/nfpm:v2.48.0@sha256:d57a1452e7a019896b355f7b1ec67262278f6c503631f652d4fce7aef9ac8df7 pkg --config nfpm.yaml --packager "$packager" --target /work/
done
cd "$work"
for package in *.deb *.rpm; do
    mv "$package" "$root/target/dist/"
    (cd "$root/target/dist" && sha256sum "$package" >"$package.sha256")
    echo "$root/target/dist/$package"
done

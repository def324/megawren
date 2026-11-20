#!/bin/sh
# Builds a statically linked Linux release inside an Alpine Linux container.
# The binary needs no system libraries, so it also runs on NAS systems.
#
#   docker run --rm -v "$PWD:/work/megawren" -w /work/megawren alpine:3.24 \
#       sh scripts/build-static-linux.sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
owner=$(stat -c %u:%g "$root")
trap 'chown -R "$owner" "$root/target" 2>/dev/null || true' EXIT
deps="$root/target/static-deps"
prefix="$deps/prefix"
jobs=$(nproc)
map="-ffile-prefix-map=$root=/src/megawren -ffile-prefix-map=$HOME=/build"

apk add --no-cache bash build-base cmake samurai pkgconf perl python3 git \
    linux-headers rustup unzip xz \
    openssl-dev openssl-libs-static sqlite-dev sqlite-static icu-dev icu-static \
    libsodium-dev libsodium-static zlib-dev zlib-static
# The checkout belongs to the host user. The SDK script reads its submodule.
git config --global --add safe.directory '*'

fetch() {
    if [ ! -f "$3" ]; then
        wget -q -O "$3.part" "$1"
        mv "$3.part" "$3"
    fi
    echo "$2  $3" | sha512sum -c -
}
mkdir -p "$deps/src" "$prefix/lib/pkgconfig" "$prefix/include"

# Alpine ships Crypto++ only as a shared library. Checksums match Alpine's own
# package recipes.
if [ ! -f "$prefix/lib/libcryptopp.a" ]; then
    fetch https://github.com/weidai11/cryptopp/releases/download/CRYPTOPP_8_9_0/cryptopp890.zip \
        903970c4c0312272e93bae0cc3477be66b273dd38c9329fda9f7157ec44e247a3bc16b9d2b4ad625f258af0eaf2dc2c4210d4d14829b455f180983859a5c4b41 \
        "$deps/src/cryptopp890.zip"
    rm -rf "$deps/src/cryptopp" && mkdir "$deps/src/cryptopp"
    unzip -q "$deps/src/cryptopp890.zip" -d "$deps/src/cryptopp"
    make -C "$deps/src/cryptopp" -j"$jobs" static CXXFLAGS="-DNDEBUG -O3 -fPIC -pthread $map"
    mkdir -p "$prefix/include/cryptopp"
    cp "$deps/src/cryptopp"/*.h "$prefix/include/cryptopp/"
    cp "$deps/src/cryptopp/libcryptopp.a" "$prefix/lib/"
fi
cat > "$prefix/lib/pkgconfig/libcrypto++.pc" <<EOF
prefix=$prefix
Name: Crypto++
Description: Crypto++ class library
Version: 8.9.0
Libs: -L\${prefix}/lib -lcryptopp
Cflags: -I\${prefix}/include
EOF

# Alpine's static curl pulls in LGPL libraries. This build keeps only OpenSSL
# and zlib. The SDK pins MEGA's keys, so curl needs no CA bundle.
if [ ! -f "$prefix/lib/libcurl.a" ]; then
    fetch https://curl.se/download/curl-8.23.0.tar.xz \
        69cf0feafdc79fbad007cf2fc977b777ce6ccb05d01afd17b5a42f8bedbfcc342fc3a48193faa7f3eafe6800cd7d07948916e8733db1b9de60287ed06219b840 \
        "$deps/src/curl-8.23.0.tar.xz"
    rm -rf "$deps/src/curl-8.23.0" "$deps/build-curl"
    xz -dc "$deps/src/curl-8.23.0.tar.xz" | tar -x -C "$deps/src"
    cmake -S "$deps/src/curl-8.23.0" -B "$deps/build-curl" -G Ninja \
        -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$prefix" -DCMAKE_INSTALL_LIBDIR=lib \
        -DCMAKE_C_FLAGS="$map" -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
        -DBUILD_SHARED_LIBS=OFF -DBUILD_STATIC_LIBS=ON -DBUILD_CURL_EXE=OFF \
        -DBUILD_TESTING=OFF -DBUILD_EXAMPLES=OFF -DBUILD_LIBCURL_DOCS=OFF \
        -DBUILD_MISC_DOCS=OFF -DENABLE_CURL_MANUAL=OFF \
        -DCURL_USE_OPENSSL=ON -DCURL_ZLIB=ON -DUSE_LIBIDN2=OFF -DCURL_USE_LIBPSL=OFF \
        -DCURL_USE_LIBSSH2=OFF -DCURL_USE_LIBSSH=OFF -DCURL_USE_GSSAPI=OFF \
        -DCURL_BROTLI=OFF -DCURL_ZSTD=OFF -DUSE_NGHTTP2=OFF -DUSE_NGTCP2=OFF \
        -DUSE_LIBRTMP=OFF -DCURL_DISABLE_LDAP=ON -DENABLE_ARES=OFF \
        -DCURL_CA_BUNDLE=none -DCURL_CA_PATH=none
    cmake --build "$deps/build-curl" --parallel "$jobs"
    cmake --install "$deps/build-curl"
fi

export PKG_CONFIG_PATH="$prefix/lib/pkgconfig"
BUILD_JOBS=$jobs "$root/scripts/build-sdk.sh" -DBUILD_SHARED_LIBS=OFF -DCMAKE_POSITION_INDEPENDENT_CODE=ON

rustup-init -y --profile minimal --default-toolchain "${RUST_VERSION:-1.99.1}" --no-modify-path
PATH="$HOME/.cargo/bin:$PATH"
# build.rs links the C++ runtime statically, so cc must not add it dynamically.
export CXXSTDLIB=
"$root/scripts/cargo.sh" build --release --locked
"$root/scripts/cargo.sh" test --locked
python3 "$root/scripts/package.py"

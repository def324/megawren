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
# CI caches these between runs. See the static-linux job in ci.yml.
cache="$root/target/static-cache"
export CCACHE_DIR="$cache/ccache" CARGO_HOME="$cache/cargo" RUSTUP_HOME="$cache/rustup"
export CARGO_INCREMENTAL=0

apk add --no-cache bash build-base cmake samurai pkgconf perl python3 git \
    linux-headers rustup unzip xz ccache \
    openssl-dev openssl-libs-static sqlite-dev sqlite-static icu icu-dev icu-static icu-data-en \
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
    mkdir -p "$prefix/share/licenses"
    cp "$deps/src/cryptopp/License.txt" "$prefix/share/licenses/Crypto++.txt"
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
    mkdir -p "$prefix/share/licenses"
    cp "$deps/src/curl-8.23.0/COPYING" "$prefix/share/licenses/curl.txt"
fi

# Alpine's libicudata.a is a stub that loads /usr/share/icu at run time, which a
# NAS does not have. genccode turns the English data subset into the real
# library, and the prefix comes before /usr/lib in the link. The SDK only uses
# an English collator and case mapping.
dat=$(ls /usr/share/icu/*/icudt*l.dat)
icu=$(basename "$dat" l.dat)
rm -rf "$deps/icudata" && mkdir "$deps/icudata"
# C output compiles for any CPU. genccode's direct object output misdetects ARM64.
genccode -e "$icu" -d "$deps/icudata" "$dat"
cc -c -o "$deps/icudata/icudata.o" "$deps/icudata"/*.c
rm -f "$prefix/lib/libicudata.a"
ar rcs "$prefix/lib/libicudata.a" "$deps/icudata/icudata.o"
echo "Embedding $(basename "$dat") ($(du -h "$dat" | cut -f1)) as ${icu}_dat"

export PKG_CONFIG_PATH="$prefix/lib/pkgconfig"
# native/musl_clockwait.h relies on this libstdc++ switch. Fail loudly if it moves.
grep -q '_GLIBCXX_USE_PTHREAD_COND_CLOCKWAIT' /usr/include/c++/*/bits/std_mutex.h
# -k reports every file that fails to compile, not only the first.
ccache --max-size=500M --zero-stats >/dev/null
CXXFLAGS="-include $root/native/musl_clockwait.h" MAKEFLAGS=-k BUILD_JOBS=$jobs \
    CMAKE_C_COMPILER_LAUNCHER=ccache CMAKE_CXX_COMPILER_LAUNCHER=ccache \
    "$root/scripts/build-sdk.sh" \
    -DBUILD_SHARED_LIBS=OFF -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
    -DCMAKE_PROJECT_INCLUDE="$root/scripts/static-linux-sdk.cmake"

ccache --show-stats | grep -E 'Hits|Misses' | head -2
rustup-init -y --profile minimal --default-toolchain "${RUST_VERSION:-1.99.1}" --no-modify-path
PATH="$CARGO_HOME/bin:$PATH"
# Embeds the dependency list in the binary so it can be audited after release.
cargo install --locked --version 0.7.7 cargo-auditable
# build.rs links the C++ runtime statically, so cc must not add it dynamically.
export CXXSTDLIB=
"$root/scripts/cargo.sh" auditable build --release --locked
# The 64-byte stub would mean ICU data is missing from the binary.
size=$(nm -S "$root/target/release/megawren" | awk -v s="${icu}_dat" '$4 == s {print $2}')
[ "$((0x${size:-0}))" -gt 1000000 ] || { echo "ICU data is not embedded (${icu}_dat size 0x${size:-0})" >&2; exit 1; }
# Tests must not find ICU data outside the binary.
rm -rf /usr/share/icu
timeout 900 "$root/scripts/cargo.sh" test --locked
python3 "$root/scripts/package.py"

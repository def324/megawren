#!/usr/bin/env sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
sdk=${MEGAWREN_SDK_SOURCE:-"$root/vendor/mega"}
# Package managers can supply their own checksum-verified SDK source archive.
if [ -z "${MEGAWREN_SDK_SOURCE:-}" ]; then
    sdk_commit=2c95a487b4b297dba1d792c88189d432c3452694
    if [ ! -e "$sdk/.git" ] || [ "$(git -C "$sdk" rev-parse HEAD)" != "$sdk_commit" ]; then
        echo 'SDK revision mismatch; restore the pinned submodule.' >&2
        exit 1
    fi
fi
export PKG_CONFIG_PATH="$root/target/native-prefix/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
if [ "$(uname -s)" = Darwin ]; then
    export MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-15.0}
fi
if ! pkg-config --exists libcrypto++ && { [ -n "${CRYPTOPP_PREFIX:-}" ] || command -v brew >/dev/null; }; then
    # Crypto++ uses a different pkg-config module name in Homebrew.
    crypto=${CRYPTOPP_PREFIX:-$(brew --prefix cryptopp)}
    test -f "$crypto/include/cryptopp/cryptlib.h" || { echo "Install cryptopp with Homebrew" >&2; exit 1; }
    mkdir -p "$root/target/brew-pkgconfig"
    version=$(awk '/^#define CRYPTOPP_(MAJOR|MINOR|REVISION) / {gsub(/\r/, "", $3); v=v sep $3; sep="."} END {print v}' "$crypto/include/cryptopp/config_ver.h")
    cat > "$root/target/brew-pkgconfig/libcrypto++.pc" <<EOF
prefix=$crypto
Name: Crypto++
Description: Homebrew Crypto++ library
Version: $version
Libs: -L\${prefix}/lib -lcryptopp
Cflags: -I\${prefix}/include
EOF
    export PKG_CONFIG_PATH="$root/target/brew-pkgconfig:$PKG_CONFIG_PATH"
fi
if [ "$(uname -s)" = Darwin ] && [ -z "${CMAKE_PREFIX_PATH:-}" ] && command -v brew >/dev/null; then
    export CMAKE_PREFIX_PATH="$(brew --prefix icu4c)"
fi
# All dependency downloads are explicit. This script only configures/builds.
cmake -S "$sdk" -B "$root/target/sdk-build" \
    -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=ON \
    "-DCMAKE_CXX_FLAGS=${CXXFLAGS-} \"-ffile-prefix-map=$HOME=/build\" \"-ffile-prefix-map=$root=/src/megawren\"" \
    "-DCMAKE_C_FLAGS=${CFLAGS-} \"-ffile-prefix-map=$HOME=/build\" \"-ffile-prefix-map=$root=/src/megawren\"" \
    -DCMAKE_INSTALL_PREFIX="$root/target/sdk" \
    -DENABLE_SYNC=OFF -DENABLE_CHAT=OFF \
    -DENABLE_SDKLIB_EXAMPLES=OFF -DENABLE_SDKLIB_TESTS=OFF \
    -DENABLE_SDKLIB_WERROR=OFF -DENABLE_QT_BINDINGS=OFF \
    -DENABLE_JAVA_BINDINGS=OFF -DENABLE_MEDIA_FILE_METADATA=OFF \
    -DENABLE_DRIVE_NOTIFICATIONS=OFF -DENABLE_ISOLATED_GFX=OFF \
    -DUSE_FREEIMAGE=OFF -DUSE_FFMPEG=OFF -DUSE_PDFIUM=OFF \
    -DUSE_LIBUV=OFF -DUSE_READLINE=OFF -DWITH_FUSE=OFF \
    -DMEGA_USE_WSUPLOAD=OFF "$@"
cmake --build "$root/target/sdk-build" --parallel "${BUILD_JOBS:-4}"
# This pinned SDK generates pkg-config metadata but has no install rules.
mkdir -p "$root/target/sdk/include" "$root/target/sdk/lib/pkgconfig"
cp "$sdk/include/megaapi.h" "$root/target/sdk/include/"
cp "$root/target/sdk-build/sdklib.pc" "$root/target/sdk/lib/pkgconfig/"
case $(uname -s) in
    Darwin) cp -P "$root"/target/sdk-build/libSDKlib*.dylib "$root/target/sdk/lib/" ;;
    Linux) cp -P "$root"/target/sdk-build/libSDKlib.so* "$root/target/sdk/lib/" ;;
    *) echo 'Native packaging script currently supports Unix hosts.' >&2; exit 1 ;;
esac

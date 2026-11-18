#!/usr/bin/env sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
export MEGAWREN_BUILD_ROOT="$root"
export PKG_CONFIG_PATH="$root/target/sdk/lib/pkgconfig:$root/target/native-prefix/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
if [ "$(uname -s)" = Darwin ]; then
    export MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-15.0}
fi
case " $* " in
    *" --release "*)
        # Cargo's unit separator preserves checkout paths containing spaces.
        separator=$(printf '\037')
        flags=${CARGO_ENCODED_RUSTFLAGS-}
        if [ -z "${CARGO_ENCODED_RUSTFLAGS+x}" ]; then
            for flag in ${RUSTFLAGS-}; do flags="${flags:+$flags$separator}$flag"; done
        fi
        export CARGO_ENCODED_RUSTFLAGS="${flags:+$flags$separator}--remap-path-prefix=$HOME=/build${separator}--remap-path-prefix=$root=/src/megawren"
        export CC_SHELL_ESCAPED_FLAGS=1
        export CFLAGS="${CFLAGS-} \"-ffile-prefix-map=$HOME=/build\" \"-ffile-prefix-map=$root=/src/megawren\""
        export CXXFLAGS="${CXXFLAGS-} \"-ffile-prefix-map=$HOME=/build\" \"-ffile-prefix-map=$root=/src/megawren\""
        ;;
esac
cd "$root"
exec cargo "$@"

#!/bin/sh
# Exercises install.sh against a freshly built release archive, in a
# throwaway home directory: install, upgrade, uninstall, purge, refusals.
#
#   sh scripts/test-install.sh target/dist/megawren-*.tar.gz
set -eu
root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
archive=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
HOME=$(mktemp -d)
export HOME
trap 'rm -rf "$HOME"' EXIT
bin="$HOME/.local/bin/megawren"
data="$HOME/.local/share/megawren"
fail() { echo "test-install: $*" >&2; exit 1; }

sh "$root/install.sh" --archive "$archive"
"$bin" --version
[ -x "$data/uninstall.sh" ] || fail "no uninstaller"
grep -q '^version=' "$data/install-receipt" || fail "no receipt"

sh "$root/install.sh" --archive "$archive" >/dev/null   # upgrade in place
"$bin" --version >/dev/null

mkdir -p "$HOME/.megawren"
touch "$HOME/.megawren/cache.enc"
"$data/uninstall.sh"
[ ! -e "$bin" ] && [ ! -e "$data" ] || fail "uninstall left files"
[ -f "$HOME/.megawren/cache.enc" ] || fail "uninstall removed ~/.megawren without --purge"

sh "$root/install.sh" --archive "$archive" >/dev/null
touch "$HOME/.megawren/session.enc"
sh "$root/install.sh" --uninstall --purge </dev/null
[ ! -e "$bin" ] && [ ! -e "$data" ] && [ ! -e "$HOME/.megawren" ] || fail "purge left files"

mkdir -p "$HOME/.local/bin"
echo foreign >"$bin"
if sh "$root/install.sh" --archive "$archive" 2>/dev/null; then fail "overwrote a foreign file"; fi
[ "$(cat "$bin")" = foreign ] || fail "changed a foreign file"
echo "install.sh: all checks passed"

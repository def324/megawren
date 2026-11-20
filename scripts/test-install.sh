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
step() { echo "== $*"; }

step "install"
sh "$root/install.sh" --archive "$archive" || fail "install exited with $?"
"$bin" --version
[ -x "$data/uninstall.sh" ] || fail "no uninstaller"
grep -q '^version=' "$data/install-receipt" || fail "no receipt"

step "upgrade in place"
sh "$root/install.sh" --archive "$archive" >/dev/null || fail "upgrade exited with $?"
"$bin" --version >/dev/null

step "uninstall keeps ~/.megawren"
mkdir -p "$HOME/.megawren"
touch "$HOME/.megawren/cache.enc"
"$data/uninstall.sh" || fail "uninstall.sh exited with $?"
if [ -e "$bin" ] || [ -e "$data" ]; then fail "uninstall left files"; fi
[ -f "$HOME/.megawren/cache.enc" ] || fail "uninstall removed ~/.megawren without --purge"

step "reinstall after uninstall"
sh "$root/install.sh" --archive "$archive" >/dev/null || fail "reinstall exited with $?"

step "purge"
touch "$HOME/.megawren/session.enc"
sh "$root/install.sh" --uninstall --purge </dev/null || fail "purge exited with $?"
if [ -e "$bin" ] || [ -e "$data" ] || [ -e "$HOME/.megawren" ]; then fail "purge left files"; fi

step "refuse to replace a foreign file"
mkdir -p "$HOME/.local/bin"
echo foreign >"$bin"
if sh "$root/install.sh" --archive "$archive" 2>/dev/null; then fail "overwrote a foreign file"; fi
[ "$(cat "$bin")" = foreign ] || fail "changed a foreign file"
echo "install.sh: all checks passed"

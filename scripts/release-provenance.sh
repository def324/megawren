#!/bin/sh
# Collects the signed build attestations of a release's files into
# megawren-VERSION.intoto.jsonl, one Sigstore bundle per line, then checks every
# file against that file alone. Publish it with the release: people can verify
# a download with it, and OpenSSF Scorecard looks for it.
#
# Usage: scripts/release-provenance.sh VERSION DIR
# DIR holds exactly the files to publish. Their .sha256 files are skipped.
set -eu

die() {
    echo "release-provenance: $*" >&2
    exit 1
}

[ $# -eq 2 ] || die "usage: $0 VERSION DIR"
version=$1
dir=$(cd "$2" && pwd) || die "$2 is not a directory."
repo=def324/megawren
out="$dir/megawren-$version.intoto.jsonl"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

count=0
for file in "$dir"/*; do
    name=$(basename "$file")
    case $name in
        *.sha256 | *.intoto.jsonl) continue ;;
        *"$version"*) ;;
        *) die "$name does not belong to $version." ;;
    esac
    # gh writes the bundles for each digest to the current directory.
    (cd "$tmp" && gh attestation download "$file" --repo "$repo" \
        --predicate-type https://slsa.dev/provenance/v1 >/dev/null) ||
        die "$name has no build attestation. Publish only files from CI on main or a tag."
    count=$((count + 1))
done
[ "$count" -gt 0 ] || die "$dir holds no release files."

cat "$tmp"/*.jsonl | sort -u >"$out"

for file in "$dir"/*; do
    case $file in *.sha256 | *.intoto.jsonl) continue ;; esac
    gh attestation verify "$file" --repo "$repo" --bundle "$out" >/dev/null 2>&1 ||
        die "$(basename "$file") does not verify against $(basename "$out")."
done

echo "Wrote $out: $(wc -l <"$out" | tr -d ' ') attestations covering $count files."

#!/bin/sh
# Scans CycloneDX SBOMs with Grype and fails on high or critical
# vulnerabilities that already have a fix.
#
#   sh scripts/scan-sboms.sh DIR
#
# It first scans .github/grype-canary.cdx.json, which lists known-vulnerable
# versions of an Alpine package, a CPE-identified C library, and a crate. A
# clean result counts only if all three of those are caught.
set -eu
root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
dir=$(cd "$1" && pwd)
grype() {
    docker run --rm -v "$dir:/sboms:ro" -v "$root/.github:/fixtures:ro" -v grype-db:/root/.cache/grype \
        anchore/grype:v0.121.2@sha256:6a36234da13f9faa327306ac0b3dab3405cfa2b4eb20c8f904125a8707863913 "$@"
}

found=$(grype sbom:/fixtures/grype-canary.cdx.json -o json | jq -r '[.matches[].artifact.name] | unique | join(" ")')
for name in openssl-libs-static curl smallvec; do
    case " $found " in
        *" $name "*) ;;
        *) echo "scan-sboms: Grype missed the known-vulnerable $name in the canary SBOM" >&2; exit 1 ;;
    esac
done
echo "Grype catches known vulnerabilities in Alpine packages, CPE-identified libraries, and crates."

status=0
for sbom in "$dir"/*.cdx.json; do
    [ -e "$sbom" ] || continue
    echo "== $(basename "$sbom")"
    grype "sbom:/sboms/$(basename "$sbom")" --only-fixed --fail-on high || status=1
done
exit $status

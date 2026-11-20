"""Package a release build for its host platform, then smoke-test the relocated binary."""
from pathlib import Path
import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import tarfile
import tempfile
import uuid

ROOT = Path(__file__).resolve().parents[1]
SDK = ROOT / "target/sdk/lib"
# C and C++ libraries inside the archive, for the SBOM. macho() and linux_static() fill it.
NATIVE = []
# CPE vendor:product, so scanners can match libraries that have no package-manager identity.
CPES = {"curl": "haxx:curl", "cryptopp": "cryptopp:crypto\\+\\+", "icu4c": "icu-project:international_components_for_unicode",
        "libsodium": "libsodium_project:libsodium", "openssl": "openssl:openssl", "sqlite": "sqlite:sqlite", "zlib": "zlib:zlib"}


def output(*args):
    return subprocess.check_output(args, text=True, cwd=ROOT).strip()


def notices(dest, packages):
    dest.mkdir()
    manifest = sorted((p["name"], p["version"], p["license"] or "", p["repository"] or "")
                      for p in packages if Path(p["manifest_path"]).parent != ROOT)
    (dest / "PACKAGES.tsv").write_text("name\tversion\tlicense\trepository\n" +
                                      "".join("\t".join(row) + "\n" for row in manifest))
    for p in packages:
        folder = Path(p["manifest_path"]).parent
        name = p["name"]
        if folder == ROOT:
            continue
        parts = [x for x in folder.iterdir() if x.name.lower().startswith(
            ("license", "licence", "copying", "copyright", "unlicense"))]
        if not parts:
            alternate = {"base64-simd": "simd", "vsimd": "simd", "block2": "objc2",
                         "dispatch2": "objc2", "objc2-encode": "objc2"}.get(name, name)
            parts = [ROOT / "docs/licenses" / (alternate + ".txt")]
        location = dest / f"{name}-{p['version']}"
        location.mkdir()
        for source in parts:
            if source.is_dir():
                shutil.copytree(source, location / source.name)
            else:
                shutil.copyfile(source, location / source.name)
    mega = ROOT / "vendor/mega"
    shutil.copyfile(mega / "LICENSE", dest / "MEGA-SDK.txt")
    for source in (mega / "third_party").rglob("*"):
        if source.is_file() and source.name.lower().startswith(("license", "copying", "copyright")):
            target = dest / "mega" / source.relative_to(mega)
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, target)


def native(name, version, purl=None, product=None):
    component = {"type": "library", "name": name, "version": version}
    if purl:
        component["purl"] = purl
    if product in CPES:
        component["cpe"] = f"cpe:2.3:a:{CPES[product]}:{version}:*:*:*:*:*:*:*"
    NATIVE.append(component)


def pkgconfig_version(path):
    match = re.search(r"^Version: *(\S+)", path.read_text(), re.M)
    return match[1] if match else "unknown"


def sbom(path, version, host, packages):
    """Write a CycloneDX SBOM listing everything compiled into the archive."""
    licenses = {(p["name"], p["version"]): p.get("license") for p in packages}
    crates = set()
    tree = output("cargo", "tree", "--locked", "--offline", "-e", "normal", "--target", host,
                  "--prefix", "none", "--format", "{p}")
    for line in tree.splitlines():
        match = re.match(r"(\S+) v(\S+)$", line.removesuffix(" (*)").strip())
        if match and match[1] != "megawren":
            crates.add((match[1], match[2]))
    components = []
    for crate, crate_version in sorted(crates):
        component = {"type": "library", "name": crate, "version": crate_version,
                     "purl": f"pkg:cargo/{crate}@{crate_version}"}
        if licenses.get((crate, crate_version)):
            component["licenses"] = [{"expression": licenses[(crate, crate_version)]}]
        components.append(component)
    commit = output("git", "rev-parse", "HEAD:vendor/mega")
    components.append({"type": "library", "name": "mega-sdk", "version": pkgconfig_version(SDK / "pkgconfig/sdklib.pc"),
                       "purl": f"pkg:github/meganz/sdk@{commit}"})
    components.extend(NATIVE)
    doc = {"bomFormat": "CycloneDX", "specVersion": "1.5", "version": 1,
           "serialNumber": "urn:uuid:" + str(uuid.uuid5(uuid.NAMESPACE_URL, f"megawren/{version}/{host}")),
           "metadata": {"component": {"type": "application", "name": "megawren", "version": version,
                                      "purl": f"pkg:github/def324/megawren@v{version}"},
                        "properties": [{"name": "target", "value": host}]},
           "components": components}
    path.write_text(json.dumps(doc, indent=2) + "\n")


def check_auditable(exe):
    """cargo auditable embeds the dependency list, so anyone can audit the shipped binary."""
    listing = output("otool", "-l", exe) if platform.system() == "Darwin" else output("readelf", "-S", exe)
    if ".dep-v0" not in listing:
        message = "The executable has no embedded dependency list. Build it with cargo auditable."
        if os.environ.get("CI"):
            raise RuntimeError(message)
        print("warning: " + message)


def check_macos_version(binary):
    # Inspect every slice. A single newer dependency raises the minimum for the whole CLI.
    versions = []
    for command in output("otool", "-arch", "all", "-l", binary).split("Load command")[1:]:
        key = "version" if "cmd LC_VERSION_MIN_MACOSX" in command else "minos"
        match = re.search(rf"^\s*{key} (\d+(?:\.\d+)*)$", command, re.M)
        if match:
            versions.append(match[1])
    if not versions or any(tuple(map(int, v.split(".")))[:2] > (15, 0) for v in versions):
        raise RuntimeError(f"{binary.name} requires macOS {', '.join(versions) or 'unknown'}. "
                           "Build the archive with macOS 15 dependencies.")


def public_owner(member):
    member.uid = member.gid = 0
    member.uname = member.gname = ""
    return member


def macho(root):
    """Rewrite every non-system edge, including the Homebrew ICU dependencies."""
    exe = root / "bin/megawren"
    libs = root / "lib"
    queue = [(exe, ROOT / "target/release/megawren")]
    while queue:
        binary, original = queue.pop()
        check_macos_version(original)
        changes = []
        for line in output("otool", "-L", original).splitlines()[1:]:
            dep = line.strip().split(" (compatibility", 1)[0]
            if dep.startswith(("/usr/lib/", "/System/Library/")):
                continue
            if dep.startswith("@rpath/"):
                src = SDK / Path(dep).name
            elif dep.startswith("@loader_path/"):
                src = original.parent / dep.removeprefix("@loader_path/")
            else:
                src = Path(dep)
            if src.resolve() == original.resolve():
                continue
            dst = libs / src.name
            if not dst.exists():
                shutil.copyfile(src, dst)
                dst.chmod(0o755)
                queue.append((dst, src))
                prefix = src.resolve().parent.parent
                cellar = re.search(r"/Cellar/([^/]+)/([^/]+)/", str(src.resolve()))
                if cellar:
                    formula, release = cellar[1], re.sub(r"_\d+$", "", cellar[2])
                    if not any(c["name"] == formula for c in NATIVE):
                        native(formula, release, f"pkg:brew/{formula.replace('@', '%40')}@{release}", formula.split("@")[0])
                if (prefix / "LICENSE").is_file():
                    shutil.copyfile(prefix / "LICENSE", root / "licenses" / (prefix.parent.name + ".txt"))
            changes.extend(["-change", dep, ("@executable_path/../lib/" if binary == exe else "@loader_path/") + dst.name])
        load = output("otool", "-l", original).splitlines()
        for i, line in enumerate(load):
            if line.strip() == "cmd LC_RPATH":
                changes.extend(["-delete_rpath", load[i + 2].strip()[5:].split(" (offset")[0]])
        if binary != exe:
            changes.extend(["-id", "@loader_path/" + binary.name])
        if changes:
            subprocess.run(["install_name_tool", *changes, binary], check=True)
        subprocess.run(["codesign", "--force", "--sign", "-", binary], check=True)
    # Crypto++ is statically linked by some SDK builds.
    licenses = [ROOT / "target/native-src/cryptopp/License.txt"]
    if shutil.which("brew"):
        crypto = subprocess.run(["brew", "--prefix", "cryptopp"], text=True, capture_output=True)
        if crypto.returncode == 0:
            licenses.extend(Path(crypto.stdout.strip()).glob("[Ll][Ii][Cc][Ee][Nn][Ss][Ee]*"))
    source = next((p for p in licenses if p.is_file()), None)
    if source is None:
        raise RuntimeError("Missing Crypto++ license for the native SDK")
    shutil.copyfile(source, root / "licenses/Crypto++.txt")


def linux_static(root):
    """A musl build carries every library inside the executable."""
    (root / "lib").rmdir()
    headers = output("readelf", "--program-headers", "--dynamic", root / "bin/megawren")
    if "(NEEDED)" in headers or "INTERP" in headers:
        raise RuntimeError("The static Linux executable still loads shared libraries")
    (root / "RUNTIME.txt").write_text("Built on " + platform.freedesktop_os_release()["PRETTY_NAME"] +
        " with musl\nStatically linked. Needs no system libraries.\n")
    # Notices for the C libraries inside the executable. SQLite is public domain.
    licenses = root / "licenses"
    for source in (ROOT / "docs/licenses/static-linux").glob("*.txt"):
        shutil.copyfile(source, licenses / source.name)
    for source in (ROOT / "target/static-deps/prefix/share/licenses").glob("*.txt"):
        shutil.copyfile(source, licenses / source.name)
    for name in ["curl.txt", "Crypto++.txt"]:
        if not (licenses / name).is_file():
            raise RuntimeError(f"Missing {name} for the static Linux archive")
    # Alpine packages carry Alpine's version, so scanners use Alpine's security data.
    alpine = ".".join(Path("/etc/alpine-release").read_text().split(".")[:2])
    for package, upstream in [("musl", "musl"), ("openssl-libs-static", "openssl"), ("icu-static", "icu"),
                              ("icu-data-en", "icu"), ("sqlite-static", "sqlite"), ("libsodium-static", "libsodium"),
                              ("zlib-static", "zlib"), ("libstdc++-dev", "gcc")]:
        line = output("apk", "list", "--installed", package)
        release = line.removeprefix(package + "-").split(" ")[0]
        arch = line.split(" ")[1]
        native(package, release, f"pkg:apk/alpine/{package}@{release}?arch={arch}&upstream={upstream}&distro=alpine-{alpine}")
    prefix = ROOT / "target/static-deps/prefix/lib/pkgconfig"
    curl = pkgconfig_version(prefix / "libcurl.pc")
    native("curl", curl, f"pkg:generic/curl@{curl}", "curl")
    cryptopp = pkgconfig_version(prefix / "libcrypto++.pc")
    native("cryptopp", cryptopp, f"pkg:generic/cryptopp@{cryptopp}", "cryptopp")


def linux(root):
    if not any(SDK.glob("libSDKlib.so*")):
        return linux_static(root)
    source, = [p for p in SDK.glob("libSDKlib.so*") if p.is_file() and not p.is_symlink()]
    soname = output("patchelf", "--print-soname", source)
    lib = root / "lib" / soname
    shutil.copyfile(source, lib)
    subprocess.run(["strip", "--strip-unneeded", lib], check=True)
    subprocess.run(["patchelf", "--remove-rpath", lib], check=True)
    exe = root / "bin/megawren"
    subprocess.run(["patchelf", "--set-rpath", "$ORIGIN/../lib", exe], check=True)
    deps = output("ldd", exe)
    sdk_line = next((s for s in deps.splitlines() if s.strip().startswith(soname + " => ")), "")
    resolved = sdk_line.partition(" => ")[2].split(" (", 1)[0]
    if "not found" in deps or not resolved or Path(resolved).resolve() != lib.resolve() or "/target/sdk/" in deps:
        raise RuntimeError("Unresolved or build-tree library in the Linux archive")
    (root / "RUNTIME.txt").write_text("Built on " + platform.freedesktop_os_release()["PRETTY_NAME"] +
        "\nRequired system libraries\n" + "\n".join(sorted(line.split("=>")[0].strip()
        for line in deps.splitlines() if "=>" in line and soname not in line)) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--licenses-only", type=Path, metavar="DIRECTORY",
                        help="collect the notices for a package manager's installation")
    args = parser.parse_args()
    host = output("rustc", "--print", "host-tuple")
    metadata = json.loads(output("cargo", "metadata", "--locked", "--offline", "--format-version", "1", "--filter-platform", host))
    if args.licenses_only:
        notices(args.licenses_only, metadata["packages"])
        return
    version = next(p["version"] for p in metadata["packages"] if Path(p["manifest_path"]).parent == ROOT)
    name = f"megawren-{version}-{host}"
    dist = ROOT / "target/dist"
    dist.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(dir=dist) as work:
        root = Path(work) / name
        (root / "bin").mkdir(parents=True)
        (root / "lib").mkdir()
        exe = root / "bin/megawren"
        shutil.copy2(ROOT / "target/release/megawren", exe)
        for file in ["LICENSE", "THIRD_PARTY_NOTICES.md"]:
            shutil.copyfile(ROOT / file, root / file)
        check_auditable(exe)
        (root / "README.md").write_text("# Megawren\n\nRun `bin/megawren` to see the files in your MEGA account, right in this terminal. "
            "Enter opens a folder or file actions. Space marks items. Press ? for help.\n\n"
            "Run `bin/megawren --help` for commands and `bin/megawren status` for local storage paths.\n")
        notices(root / "licenses", metadata["packages"])
        {"Darwin": macho, "Linux": linux}[platform.system()](root)
        for binary in [exe, *(root / "lib").glob("*")]:
            data = binary.read_bytes()
            if any(os.fsencode(str(path) + "/") in data for path in (ROOT, Path.home())):
                raise RuntimeError(f"{binary.name} contains a builder path. Rebuild with the release flags.")
        if output(exe, "--version") != "megawren " + version:
            raise RuntimeError("Packaged executable has the wrong version")
        archive = Path(work) / (name + ".tar.gz")
        with tarfile.open(archive, "w:gz", compresslevel=6) as tar:
            tar.add(root, arcname=name, filter=public_owner)
        artifact = dist / archive.name
        archive.replace(artifact)
    with artifact.open("rb") as stream:
        checksum = hashlib.file_digest(stream, "sha256").hexdigest()
    (dist / (artifact.name + ".sha256")).write_text(f"{checksum}  {artifact.name}\n")
    sbom(dist / f"{name}.cdx.json", version, host, metadata["packages"])
    print(artifact)


if __name__ == "__main__":
    main()

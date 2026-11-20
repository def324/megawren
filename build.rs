fn main() {
    // A shared Cargo target can otherwise reuse another checkout's native bridge.
    println!("cargo:rerun-if-env-changed=MEGAWREN_BUILD_ROOT");
    for file in [
        "src/bridge.rs",
        "native/client.hpp",
        "native/client.cpp",
        "native/cache.hpp",
        "native/cache.cpp",
    ] {
        println!("cargo:rerun-if-changed={file}");
    }
    let sdk = pkg_config::Config::new()
        .cargo_metadata(false)
        .probe("sdklib")
        .expect("Build SDK and set PKG_CONFIG_PATH as described in README.md");
    let sodium = pkg_config::Config::new()
        .cargo_metadata(false)
        .probe("libsodium")
        .expect("libsodium is required");
    let mut build = cxx_build::bridge("src/bridge.rs");
    build
        .file("native/client.cpp")
        .file("native/cache.cpp")
        .include(".")
        .std("c++20");
    for path in sdk.include_paths.iter().chain(&sodium.include_paths) {
        build.include(path);
    }
    build.compile("megawren-bridge");
    // musl builds link every native dependency into one self-contained binary.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("musl") {
        link_statically();
        return;
    }
    // GNU ld discards as-needed libraries that precede the static C++ bridge.
    for library in [&sdk, &sodium] {
        for path in &library.link_paths {
            println!("cargo:rustc-link-search=native={}", path.display());
        }
        for name in &library.libs {
            println!("cargo:rustc-link-lib={name}");
        }
    }
    if std::env::var("CARGO_CFG_TARGET_FAMILY").as_deref() == Ok("unix") {
        for path in sdk.link_paths {
            let target = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
                .join("target");
            let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
            if path == target.join("sdk/lib") && out.ancestors().nth(4) == Some(target.as_path()) {
                let loader = if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
                    "@loader_path"
                } else {
                    "$ORIGIN"
                };
                // Cargo places test harnesses one directory below the CLI.
                for relative in ["../sdk/lib", "../../sdk/lib"] {
                    println!("cargo:rustc-link-arg=-Wl,-rpath,{loader}/{relative}");
                }
            } else {
                println!("cargo:rustc-link-arg=-Wl,-rpath,{}", path.display());
            }
        }
    }
}

/// Links the static SDK and its dependency chain in the order pkg-config resolves.
/// The build sets CXXSTDLIB to an empty value so that cc does not also link the
/// C++ runtime dynamically. It is linked statically here, after everything that
/// needs it.
fn link_statically() {
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    let pkg_config = std::env::var("PKG_CONFIG").unwrap_or_else(|_| "pkg-config".into());
    let packages = [
        "sdklib",
        "libcurl",
        "sqlite3",
        "libsodium",
        "libcrypto++",
        "icu-i18n",
        "icu-uc",
        "openssl",
    ];
    let output = std::process::Command::new(pkg_config)
        .args(["--static", "--libs"])
        .args(packages)
        .output()
        .expect("pkg-config is required");
    assert!(
        output.status.success(),
        "pkg-config --static failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    for flag in String::from_utf8(output.stdout).unwrap().split_whitespace() {
        if let Some(path) = flag.strip_prefix("-L") {
            println!("cargo:rustc-link-search=native={path}");
        } else if let Some(name) = flag.strip_prefix("-l") {
            // musl's libc.a provides the system libraries, and Rust's standard
            // library links it. The C++ runtime is linked last, below.
            if !matches!(name, "c" | "m" | "dl" | "pthread" | "rt" | "stdc++") {
                println!("cargo:rustc-link-lib=static={name}");
            }
        }
    }
    println!("cargo:rustc-link-lib=static=stdc++");
}
